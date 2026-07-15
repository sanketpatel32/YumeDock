#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    config::{ConfigV1, PinConfig, PinKind},
    model::{DockItem, MonitorInfo, WindowEntry, enumerate_monitors, group_for_monitor},
    render::{
        DockBounce, DockHover, DockRenderState, DockVisual, LauncherHit, Renderer, TopBarSegment,
        TopBarSegmentFlags, TopBarStatus,
    },
    shell::{AppBar, TaskbarState},
    status, tracker,
};
use anyhow::Result;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Dwm::{
                DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_VISIBLE, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
                DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
                DWMWCP_ROUND, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail,
                DwmSetWindowAttribute, DwmUnregisterThumbnail, DwmUpdateThumbnailProperties,
            },
            Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient},
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
            LibraryLoader::GetModuleHandleW,
            Threading::GetCurrentThreadId,
        },
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
            Controls::WM_MOUSELEAVE,
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, ReleaseCapture,
                SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_F12,
                VK_ESCAPE, VK_LWIN, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP, keybd_event,
            },
            Shell::{
                DragAcceptFiles, DragFinish, DragQueryFileW, HDROP, SHERB_NOCONFIRMATION,
                SHERB_NOPROGRESSUI, SHEmptyRecycleBinW, ShellExecuteW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW,
                DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, GA_ROOTOWNER,
                GetAncestor, GetClientRect, GetCursorPos, GetForegroundWindow, GetMessageW,
                GetWindowPlacement, HTCLIENT, HTTRANSPARENT, IDC_ARROW, IsIconic, KillTimer,
                LoadCursorW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
                PostQuitMessage, PostThreadMessageW, RegisterClassExW, RegisterWindowMessageW,
                SPI_GETHIGHCONTRAST, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOW, SW_SHOWNA,
                SetForegroundWindow, SetTimer, ShowWindow, SystemParametersInfoW, TPM_RETURNCMD,
                TrackPopupMenu, TranslateMessage, WINDOWPLACEMENT, WM_APP, WM_CAPTURECHANGED,
                WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_DROPFILES, WM_HOTKEY,
                WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONUP, WM_MOUSEMOVE,
                WM_MOUSEWHEEL, WM_NCHITTEST, WM_PAINT,
                WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SIZE, WM_TIMER, WNDCLASSEXW, WS_EX_NOACTIVATE,
                WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
    core::{BOOL, PCWSTR, w},
};

const CLASS_NAME: PCWSTR = w!("YumeDock.NativeWindow");
const HOTKEY_RESTORE: i32 = 0x5944;
const CMD_SETTINGS: usize = 100;
const CMD_QUIT: usize = 101;
const CMD_RESTORE_QUIT: usize = 102;
const CMD_REDUCE_MOTION: usize = 103;
const CMD_TASKBAR: usize = 104;
const CMD_ICON_LARGER: usize = 105;
const CMD_ICON_SMALLER: usize = 106;
const CMD_STARTUP: usize = 107;
const CMD_MINIMIZE_ACTIVE: usize = 108;
const CMD_CLOSE_ACTIVE: usize = 109;
const CMD_SHOW_DESKTOP: usize = 110;
const CMD_NETWORK_SETTINGS: usize = 111;
const CMD_SOUND_SETTINGS: usize = 112;
const CMD_VOLUME_DOWN: usize = 113;
const CMD_VOLUME_UP: usize = 114;
const CMD_VOLUME_MUTE: usize = 115;
const CMD_AUTO_HIDE: usize = 116;
const CMD_POWER_SETTINGS: usize = 117;
const CMD_DATE_MENU: usize = 118;
const CMD_START_MENU: usize = 119;
const CMD_QUICK_SETTINGS: usize = 120;
const CMD_NOTIFICATION_CENTER: usize = 121;
const CMD_CLOCK_24_HOUR: usize = 122;
const CMD_SHOW_NETWORK: usize = 123;
const CMD_SHOW_VOLUME: usize = 124;
const CMD_SHOW_BATTERY: usize = 125;
const CMD_OPEN_ITEM: usize = 200;
const CMD_NEW_INSTANCE: usize = 201;
const CMD_TOGGLE_PIN: usize = 202;
const CMD_MINIMIZE_ITEM: usize = 203;
const CMD_CLOSE_ITEM: usize = 204;
const CMD_EMPTY_RECYCLE: usize = 205;
const CMD_DEBUG_POPOVER: usize = 300;
const CMD_POWER_SLEEP: usize = 315;
const CMD_POWER_RESTART: usize = 316;
const CMD_POWER_SHUTDOWN: usize = 317;
const CMD_POWER_LOCK: usize = 318;
/// Fixed launcher quick-action labels in display order. The index matches the
/// `match` in `launcher_click`'s `LauncherHit::Action(i)` arm.
const LAUNCHER_ACTION_LABELS: &[&str] =
    &["Start", "Run…", "File Explorer", "Lock", "Sleep", "Restart", "Shut Down"];
pub const WM_REFRESH: u32 = WM_APP + 1;
const WM_REBUILD_MONITORS: u32 = WM_APP + 2;
const WM_FOREGROUND_CHANGED: u32 = WM_APP + 3;
const WM_NATIVE_MINIMIZE: u32 = WM_APP + 4;
const STACK_COLUMNS: usize = 5;
const STACK_CELL_WIDTH: i32 = 72;
const STACK_CELL_HEIGHT: i32 = 76;
const STACK_PADDING: i32 = 16;
const STACK_HEADER: i32 = 38;
const STACK_FOOTER: i32 = 38;
// Enough strips to keep the funnel curved without issuing hundreds of GPU draws per frame.
const GENIE_SLICES: usize = 72;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowKind {
    Top,
    Dock,
    Reserve,
    Preview,
    FolderStack,
    LaunchOverlay,
    DebugPopover,
    Launcher,
}

struct Preview {
    hwnd: HWND,
    thumbnail: isize,
    source: HWND,
    dock: HWND,
    title: String,
    close_hover: bool,
    pointer_x: f32,
}

struct FolderStack {
    hwnd: HWND,
    dock: HWND,
    folder: std::path::PathBuf,
    title: String,
    entries: Vec<std::path::PathBuf>,
    hover: Option<usize>,
    footer_hover: bool,
    pointer_x: f32,
}

/// State for the open launcher popover. `apps` is the full enumerated list;
/// `scroll` is the index of the first visible row; `hover` is the currently
/// highlighted target (action index or visible app index).
struct LauncherState {
    /// (label, .lnk path) for each Start Menu app, alphabetical.
    apps: Vec<(String, std::path::PathBuf)>,
    /// Index of the first visible app row.
    scroll: usize,
    /// Currently hovered target.
    hover: Option<LauncherHit>,
}

/// A single topbar popover window. Only one is ever open at a time
/// (`App::active_popover`), matching the macOS menu-bar invariant.
struct Popover {
    hwnd: HWND,
    /// The top-bar window that owns this popover (anchor + dismiss owner).
    owner: HWND,
}

struct LaunchBounce {
    item: usize,
    key: String,
    wait_for_window: bool,
    started: Instant,
    finish_after: Option<Duration>,
}

struct AutoHideState {
    progress: f32,
    target: f32,
    last_tick: Instant,
}

struct DockDrag {
    hwnd: HWND,
    pin: PinConfig,
    start_x: i32,
    active: bool,
}

struct ReorderAnimation {
    from: usize,
    to: usize,
    started: Instant,
}

struct MonitorShell {
    info: MonitorInfo,
    top: HWND,
    dock: HWND,
    reserve: HWND,
    top_appbar: Option<AppBar>,
    bottom_appbar: Option<AppBar>,
    icon_size: f32,
    scale: f32,
}

struct DockInteractionGeometry {
    scale: f32,
    width: f32,
    height: f32,
    count: usize,
    separator: Option<usize>,
    icon_size: f32,
    magnification: f32,
    hidden: bool,
    bouncing: bool,
    shell_left: f32,
    shell_top: f32,
    shell_right: f32,
    shell_bottom: f32,
}

pub struct App {
    config: ConfigV1,
    renderer: Renderer,
    shells: Vec<MonitorShell>,
    kinds: HashMap<isize, WindowKind>,
    hover: HashMap<isize, Option<usize>>,
    cursor_x: HashMap<isize, f32>,
    windows: Vec<WindowEntry>,
    dock_cache: HashMap<isize, Arc<[DockItem]>>,
    system_status: status::SystemStatus,
    taskbar: TaskbarState,
    safe_mode: bool,
    shutting_down: bool,
    _hooks: Option<tracker::HookSet>,
    preview: Option<Preview>,
    preview_candidate: Option<(HWND, HWND)>,
    folder_stack: Option<FolderStack>,
    menu_item: Option<DockItem>,
    animation: HashMap<isize, f32>,
    animation_clock: HashMap<isize, Instant>,
    launch_bounce: HashMap<isize, LaunchBounce>,
    pending_window_open: Option<PendingWindowOpen>,
    window_open_animation: Option<WindowOpenAnimation>,
    auto_hide: HashMap<isize, AutoHideState>,
    drag: Option<DockDrag>,
    reorder_animation: HashMap<isize, ReorderAnimation>,
    cycle_index: HashMap<String, usize>,
    monitor_rebuild_pending: bool,
    high_contrast: bool,
    last_genie_snapshot: Option<(isize, Instant)>,
    last_clock: String,
    active_popover: Option<Popover>,
    launcher: Option<LauncherState>,
}

impl App {
    pub fn run(mut config: ConfigV1, safe_mode: bool) -> Result<()> {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).ok();
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        if safe_mode {
            config.behavior.replace_taskbar = false;
        }
        register_window_class()?;
        TASKBAR_CREATED.store(
            unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            Ordering::Relaxed,
        );
        let high_contrast = high_contrast_enabled();
        let renderer = Renderer::new(high_contrast)?;
        let system_status = status::read_status();
        let last_clock = current_clock(config.top_bar.use_24_hour_clock);
        let mut app = Self {
            config,
            renderer,
            shells: Vec::new(),
            kinds: HashMap::new(),
            hover: HashMap::new(),
            cursor_x: HashMap::new(),
            windows: tracker::enumerate_windows(),
            dock_cache: HashMap::new(),
            system_status,
            taskbar: TaskbarState::capture(),
            safe_mode,
            shutting_down: false,
            _hooks: None,
            preview: None,
            preview_candidate: None,
            folder_stack: None,
            menu_item: None,
            animation: HashMap::new(),
            animation_clock: HashMap::new(),
            launch_bounce: HashMap::new(),
            pending_window_open: None,
            window_open_animation: None,
            auto_hide: HashMap::new(),
            drag: None,
            reorder_animation: HashMap::new(),
            cycle_index: HashMap::new(),
            monitor_rebuild_pending: false,
            high_contrast,
            last_genie_snapshot: None,
            last_clock,
            active_popover: None,
            launcher: None,
        };
        if app.config.behavior.replace_taskbar {
            app.taskbar.hide();
        }
        app.create_monitor_shells()?;
        let _ = crate::config::sync_startup(app.config.behavior.start_with_windows);
        unsafe {
            RegisterHotKey(
                None,
                HOTKEY_RESTORE,
                MOD_CONTROL | MOD_ALT | MOD_SHIFT,
                VK_F12.0 as u32,
            )?;
        }
        app._hooks = Some(tracker::HookSet::install(
            unsafe { GetCurrentThreadId() },
            WM_REFRESH,
            WM_FOREGROUND_CHANGED,
            WM_NATIVE_MINIMIZE,
        ));
        APP.with(|slot| *slot.borrow_mut() = Some(app));

        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).into() {
                if msg.hwnd.is_invalid() && msg.message == WM_REFRESH {
                    with_app(|app| app.schedule_window_refresh());
                    continue;
                }
                if msg.hwnd.is_invalid() && msg.message == WM_FOREGROUND_CHANGED {
                    with_app(|app| app.refresh_foreground_app());
                    continue;
                }
                if msg.hwnd.is_invalid() && msg.message == WM_NATIVE_MINIMIZE {
                    with_app(|app| app.handle_native_minimize(HWND(msg.wParam.0 as *mut _)));
                    continue;
                }
                if msg.hwnd.is_invalid() && msg.message == WM_REBUILD_MONITORS {
                    with_app(|app| {
                        app.monitor_rebuild_pending = false;
                        app.rebuild_monitors();
                    });
                    continue;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = UnregisterHotKey(None, HOTKEY_RESTORE);
        }
        APP.with(|slot| {
            if let Some(mut app) = slot.borrow_mut().take() {
                app.restore_shell();
            }
        });
        Ok(())
    }

    fn create_monitor_shells(&mut self) -> Result<()> {
        for info in enumerate_monitors() {
            let scale = info.scale();
            let top_height = scale_i32(self.config.top_bar.height, scale);
            let dock_height = scale_i32(self.config.dock.height, scale);
            let width = info.bounds.right - info.bounds.left;
            let bottom_edge = if self.config.behavior.replace_taskbar {
                info.bounds.bottom
            } else {
                info.work.bottom
            };
            let item_count =
                group_for_monitor(&self.config.pins, &self.windows, info.handle.0 as isize)
                    .len()
                    .max(1);
            let max_dock_width = width as f32 * 0.82;
            let desired_icon_px = self.config.dock.icon_size * scale;
            let icon_size_px = desired_icon_px.min(
                (((max_dock_width - 24.0 * scale) / item_count as f32) - 8.0 * scale)
                    .max(30.0 * scale),
            );
            let icon_size = icon_size_px / scale;
            let top = create_window(info.bounds.left, info.bounds.top, width, top_height, true)?;
            let reserve = create_window(
                info.bounds.left,
                bottom_edge - dock_height,
                width,
                dock_height,
                true,
            )?;
            let base_content = item_count as f32 * icon_size_px
                + item_count.saturating_sub(1) as f32 * 7.0 * scale
                + 13.0 * scale;
            let magnification_room = icon_size_px * (self.config.dock.magnification - 1.0) * 2.7;
            let dock_width = ((base_content + magnification_room + 212.0 * scale).ceil() as i32)
                .clamp(scale_i32(220, scale), (width as f32 * 0.96) as i32);
            let dock_visual_height = ((icon_size * self.config.dock.magnification + 62.0) * scale)
                .ceil()
                .max(dock_height as f32) as i32;
            let dock = create_window(
                info.bounds.left + (width - dock_width) / 2,
                bottom_edge - dock_visual_height,
                dock_width,
                dock_visual_height,
                false,
            )?;
            configure_window_backdrop(top, false, self.high_contrast);
            // The dock draws its own rounded panel inside a larger transparent host.
            // Rounding the host itself makes DWM outline the full host rectangle.
            configure_window_backdrop(dock, false, self.high_contrast);
            self.kinds.insert(top.0 as isize, WindowKind::Top);
            self.kinds.insert(dock.0 as isize, WindowKind::Dock);
            self.kinds.insert(reserve.0 as isize, WindowKind::Reserve);
            self.hover.insert(dock.0 as isize, None);
            self.cursor_x.insert(dock.0 as isize, 0.0);
            self.hover.insert(top.0 as isize, None);
            self.cursor_x.insert(top.0 as isize, 0.0);
            self.animation.insert(dock.0 as isize, 0.0);
            self.animation_clock.insert(dock.0 as isize, Instant::now());
            self.auto_hide.insert(
                dock.0 as isize,
                AutoHideState {
                    progress: 0.0,
                    target: 0.0,
                    last_tick: Instant::now(),
                },
            );

            let (top_appbar, bottom_appbar) = if self.config.behavior.reserve_edges {
                let top_rect = RECT {
                    left: info.bounds.left,
                    top: info.bounds.top,
                    right: info.bounds.right,
                    bottom: info.bounds.top + top_height,
                };
                let bottom_rect = RECT {
                    left: info.bounds.left,
                    top: bottom_edge - dock_height,
                    right: info.bounds.right,
                    bottom: bottom_edge,
                };
                let bottom_appbar = if self.config.dock.auto_hide {
                    None
                } else {
                    Some(AppBar::register(reserve, false, bottom_rect)?)
                };
                let top_appbar = if info.primary {
                    Some(AppBar::register(top, true, top_rect)?)
                } else {
                    None
                };
                (top_appbar, bottom_appbar)
            } else {
                (None, None)
            };
            self.shells.push(MonitorShell {
                info,
                top,
                dock,
                reserve,
                top_appbar,
                bottom_appbar,
                icon_size,
                scale,
            });
            self.dock_cache.insert(
                info.handle.0 as isize,
                Arc::from(group_for_monitor(
                    &self.config.pins,
                    &self.windows,
                    info.handle.0 as isize,
                )),
            );

            // Prime and commit each DirectComposition surface while its host is
            // still hidden. This prevents DWM from exposing an unpainted fallback
            // frame during cold startup.
            if info.primary {
                self.paint(top);
            }
            self.paint(dock);
            unsafe {
                let _ = ShowWindow(top, if info.primary { SW_SHOWNA } else { SW_HIDE });
                let _ = ShowWindow(dock, SW_SHOWNA);
                let _ = ShowWindow(reserve, SW_HIDE);
                if self.shells.len() == 1 {
                    SetTimer(Some(top), 1, 5000, None);
                    SetTimer(Some(top), 12, 1000, None);
                }
                if self.config.dock.auto_hide {
                    SetTimer(Some(dock), 8, 1000, None);
                }
            }
        }
        Ok(())
    }

    fn destroy_monitor_shells(&mut self) {
        self.close_window_open_animation(false);
        self.pending_window_open = None;
        self.close_popover();
        self.close_folder_stack();
        self.close_preview();
        self.preview_candidate = None;
        for shell in self.shells.drain(..) {
            drop(shell.top_appbar);
            drop(shell.bottom_appbar);
            for hwnd in [shell.dock, shell.top, shell.reserve] {
                self.renderer.forget(hwnd);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
        self.kinds.clear();
        self.hover.clear();
        self.cursor_x.clear();
        self.animation.clear();
        self.animation_clock.clear();
        self.launch_bounce.clear();
        self.auto_hide.clear();
        self.drag = None;
        self.reorder_animation.clear();
        self.dock_cache.clear();
    }

    fn rebuild_monitors(&mut self) {
        self.destroy_monitor_shells();
        if let Err(error) = self.create_monitor_shells() {
            self.taskbar.restore();
            unsafe {
                MessageBoxW(
                    None,
                    &windows::core::HSTRING::from(format!(
                        "YumeDock could not rebuild displays:\n{error:#}"
                    )),
                    w!("YumeDock"),
                    Default::default(),
                );
            }
            self.shutdown();
        }
    }

    fn schedule_monitor_rebuild(&mut self) {
        if self.monitor_rebuild_pending {
            return;
        }
        self.monitor_rebuild_pending = true;
        unsafe {
            if PostThreadMessageW(
                GetCurrentThreadId(),
                WM_REBUILD_MONITORS,
                Default::default(),
                Default::default(),
            )
            .is_err()
            {
                self.monitor_rebuild_pending = false;
            }
        }
    }

    fn refresh_windows(&mut self) {
        let previous_apps: HashSet<String> = self
            .windows
            .iter()
            .map(|window| window.identity.icon_key.to_ascii_lowercase())
            .collect();
        self.windows = tracker::enumerate_windows();
        self.rebuild_dock_cache();
        let opened_apps: HashSet<String> = self
            .windows
            .iter()
            .map(|window| window.identity.icon_key.to_ascii_lowercase())
            .filter(|key| !previous_apps.contains(key))
            .collect();
        self.stop_ready_launch_bounces();
        let mut new_bounces = Vec::new();
        for shell in &self.shells {
            if !opened_apps.is_empty() && !self.launch_bounce.contains_key(&(shell.dock.0 as isize))
            {
                let items = self.dock_items(shell.dock);
                if let Some((item, key)) = items.iter().enumerate().find_map(|(index, item)| {
                    let DockItem::Application { windows, .. } = item else {
                        return None;
                    };
                    let opened = windows.iter().any(|hwnd| {
                        self.windows
                            .iter()
                            .find(|window| window.hwnd == *hwnd)
                            .map(|window| window.identity.icon_key.to_ascii_lowercase())
                            .is_some_and(|key| opened_apps.contains(&key))
                    });
                    opened
                        .then(|| dock_identity_key(item))
                        .flatten()
                        .map(|key| (index, key))
                }) {
                    new_bounces.push((shell.dock, item, key));
                }
            }
            unsafe {
                let _ = InvalidateRect(Some(shell.dock), None, false);
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
        for (dock, item, key) in new_bounces {
            self.start_launch_bounce(dock, item, key, false);
        }
        self.refresh_launch_bounce_items();

        let pending_ready = self.pending_window_open.as_ref().and_then(|pending| {
            let items = self.dock_items(pending.dock);
            items.iter().find_map(|item| {
                if dock_identity_key(item).as_deref() != Some(pending.key.as_str()) {
                    return None;
                }
                let DockItem::Application { windows, .. } = item else {
                    return None;
                };
                windows
                    .first()
                    .copied()
                    .map(|source| (pending.dock, pending.origin, source))
            })
        });
        if let Some((dock, origin, source)) = pending_ready {
            self.pending_window_open = None;
            self.start_window_open_animation(dock, source, origin, true);
        } else if self
            .pending_window_open
            .as_ref()
            .is_some_and(|pending| pending.started.elapsed() >= Duration::from_secs(8))
        {
            self.pending_window_open = None;
        }
    }

    fn refresh_launch_bounce_items(&mut self) {
        let updates: Vec<_> = self
            .shells
            .iter()
            .filter_map(|shell| {
                let dock = shell.dock.0 as isize;
                let key = &self.launch_bounce.get(&dock)?.key;
                let items = self.dock_items(shell.dock);
                let item = items
                    .iter()
                    .position(|item| dock_identity_key(item).as_deref() == Some(key.as_str()))?;
                Some((dock, item))
            })
            .collect();
        for (dock, item) in updates {
            if let Some(state) = self.launch_bounce.get_mut(&dock) {
                state.item = item;
            }
        }
    }

    fn refresh_system_settings(&mut self) {
        self.system_status = status::read_status();
        let high_contrast = high_contrast_enabled();
        if self.high_contrast != high_contrast {
            self.high_contrast = high_contrast;
            self.renderer.set_high_contrast(high_contrast);
            self.rebuild_monitors();
        } else {
            self.refresh_windows();
        }
    }

    fn refresh_system_status(&mut self) {
        let next = status::read_status();
        let clock = current_clock(self.config.top_bar.use_24_hour_clock);
        let status_changed = next != self.system_status;
        let clock_changed = clock != self.last_clock;
        self.system_status = next;
        self.last_clock = clock;
        if !status_changed && !clock_changed {
            return;
        }
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
    }

    fn refresh_foreground_app(&mut self) {
        self.last_genie_snapshot = None;
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
    }

    fn cache_foreground_for_genie(&mut self) {
        // A full-window capture on this UI thread would steal a frame from the active transition.
        if self.window_open_animation.is_some() {
            return;
        }
        // Nothing to cache for transitions when the dock is fully hidden and
        // nothing is animating — skip the foreground probing entirely.
        if self.config.dock.auto_hide
            && self.window_open_animation.is_none()
            && self.launch_bounce.is_empty()
            && self.shells.iter().all(|shell| {
                self.auto_hide
                    .get(&(shell.dock.0 as isize))
                    .is_some_and(|state| state.target >= 1.0 && state.progress >= 0.999)
            })
        {
            return;
        }
        let source = unsafe { GetForegroundWindow() };
        if source.is_invalid()
            || unsafe { IsIconic(source).as_bool() }
            || self.kinds.contains_key(&(source.0 as isize))
        {
            return;
        }
        let mut rect = RECT::default();
        if unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(source, &mut rect).is_err()
        } || rect_width(rect) < 120
            || rect_height(rect) < 80
        {
            return;
        }
        let mut cursor = POINT::default();
        let near_caption = unsafe { GetCursorPos(&mut cursor).is_ok() }
            && cursor.x >= rect.right - 280
            && cursor.x <= rect.right
            && cursor.y >= rect.top
            && cursor.y <= rect.top + 80;
        let now = Instant::now();
        let minimum_age = if near_caption {
            Duration::from_millis(750)
        } else {
            Duration::from_secs(4)
        };
        if self.last_genie_snapshot.is_some_and(|(cached, at)| {
            cached == source.0 as isize && now.duration_since(at) < minimum_age
        }) {
            return;
        }
        let dock = self
            .windows
            .iter()
            .find(|entry| entry.hwnd == source)
            .and_then(|entry| {
                self.shells
                    .iter()
                    .find(|shell| shell.info.handle.0 as isize == entry.monitor)
            })
            .or_else(|| self.shells.first())
            .map(|shell| shell.dock);
        let Some(dock) = dock else {
            return;
        };
        if self
            .renderer
            .cache_genie_snapshot(dock, source, rect_width(rect), rect_height(rect))
        {
            self.last_genie_snapshot = Some((source.0 as isize, now));
        }
    }

    fn stop_ready_launch_bounces(&mut self) {
        let ready: Vec<isize> = self
            .shells
            .iter()
            .filter_map(|shell| {
                let key = shell.dock.0 as isize;
                let state = self.launch_bounce.get(&key)?;
                if !state.wait_for_window {
                    return None;
                }
                let ready = self.shells.iter().any(|candidate| {
                    self.dock_items(candidate.dock).iter().any(|item| {
                        dock_identity_key(item).is_some_and(|key| key == state.key)
                            && matches!(item, DockItem::Application { windows, .. } if !windows.is_empty())
                    })
                });
                ready.then_some(key)
            })
            .collect();
        for key in ready {
            if let Some(state) = self.launch_bounce.get_mut(&key) {
                let elapsed_ms = state.started.elapsed().as_millis() as u64;
                let next_landing = (elapsed_ms / 620 + 1) * 620;
                state.wait_for_window = false;
                state.finish_after = Some(Duration::from_millis(next_landing));
            }
        }
    }

    fn schedule_window_refresh(&self) {
        if let Some(shell) = self.shells.first() {
            unsafe {
                SetTimer(
                    Some(shell.top),
                    5,
                    if self.pending_window_open.is_some() {
                        60
                    } else {
                        250
                    },
                    None,
                );
            }
        } else {
            tracker::mark_refresh_handled();
        }
    }

    fn process_window_refresh(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), 5);
        }
        self.refresh_windows();
        tracker::mark_refresh_handled();
    }

    fn restore_shell(&mut self) {
        self.taskbar.restore();
        self.destroy_monitor_shells();
    }

    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.taskbar.restore();
        let _ = self.config.save();
        unsafe {
            PostQuitMessage(0);
        }
    }

    fn monitor_for(&self, hwnd: HWND) -> Option<&MonitorShell> {
        self.shells
            .iter()
            .find(|s| s.top == hwnd || s.dock == hwnd || s.reserve == hwnd)
    }

    fn rebuild_dock_cache(&mut self) {
        self.dock_cache.clear();
        for shell in &self.shells {
            let monitor = shell.info.handle.0 as isize;
            self.dock_cache.insert(
                monitor,
                Arc::from(group_for_monitor(&self.config.pins, &self.windows, monitor)),
            );
        }
    }

    fn dock_items(&self, hwnd: HWND) -> Arc<[DockItem]> {
        let monitor = self
            .monitor_for(hwnd)
            .map(|s| s.info.handle.0 as isize)
            .unwrap_or_default();
        self.dock_cache.get(&monitor).cloned().unwrap_or_else(|| {
            Arc::from(group_for_monitor(&self.config.pins, &self.windows, monitor))
        })
    }

    fn dock_hit_region(&self, hwnd: HWND, screen_x: i32, screen_y: i32) -> Option<bool> {
        if self.kinds.get(&(hwnd.0 as isize)) != Some(&WindowKind::Dock) {
            return None;
        }
        let mut point = POINT {
            x: screen_x,
            y: screen_y,
        };
        if !unsafe { ScreenToClient(hwnd, &mut point) }.as_bool() {
            return Some(false);
        }
        let geometry = self.dock_interaction_geometry(hwnd)?;
        let point_x = point.x as f32 / geometry.scale;
        let point_y = point.y as f32 / geometry.scale;
        if geometry.hidden {
            return Some(point_y >= geometry.height - 3.0 && point_y <= geometry.height);
        }
        if rounded_rect_contains(
            point_x,
            point_y,
            geometry.shell_left,
            geometry.shell_top,
            geometry.shell_right,
            geometry.shell_bottom,
            18.0,
        ) {
            return Some(true);
        }
        let over_icon = crate::render::dock_hit_test(
            point_x,
            geometry.width,
            geometry.count,
            geometry.icon_size,
            geometry.magnification,
            geometry.separator,
        )
        .is_some();
        let icon_top = geometry.shell_bottom
            - 9.0
            - geometry.icon_size * geometry.magnification
            - if geometry.bouncing {
                geometry.icon_size * 0.42
            } else {
                0.0
            };
        Some(over_icon && point_y >= icon_top && point_y <= geometry.shell_bottom)
    }

    fn dock_interaction_geometry(&self, hwnd: HWND) -> Option<DockInteractionGeometry> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect).ok()? };
        let scale = self
            .monitor_for(hwnd)
            .map(|shell| shell.scale)
            .unwrap_or_else(|| window_scale(hwnd));
        let width = (rect.right - rect.left) as f32 / scale;
        let height = (rect.bottom - rect.top) as f32 / scale;
        let icon_size = self
            .monitor_for(hwnd)
            .map(|shell| shell.icon_size)
            .unwrap_or(self.config.dock.icon_size);
        let items = self.dock_items(hwnd);
        let separator = items.iter().enumerate().find_map(|(index, item)| {
            (index > 0
                && matches!(item, DockItem::Folder(_) | DockItem::RecycleBin)
                && matches!(items[index - 1], DockItem::Application { .. }))
            .then_some(index)
        });
        let base_content = items.len() as f32 * icon_size
            + items.len().saturating_sub(1) as f32 * 7.0
            + separator.map_or(0.0, |_| 13.0);
        let hide_progress = self
            .auto_hide
            .get(&(hwnd.0 as isize))
            .map_or(0.0, |state| state.progress);
        let hide_offset = hide_progress * (icon_size + 24.0);
        let shell_bottom = height - 8.0 + hide_offset;
        let shell_top = shell_bottom - icon_size - 18.0;
        let hovered = self
            .hover
            .get(&(hwnd.0 as isize))
            .copied()
            .flatten()
            .is_some();
        let animation = self
            .animation
            .get(&(hwnd.0 as isize))
            .copied()
            .unwrap_or(if hovered { 1.0 } else { 0.0 });
        let magnification = if self.config.behavior.reduce_motion {
            1.0
        } else {
            1.0 + (self.config.dock.magnification - 1.0) * animation
        };
        let expansion = icon_size * (magnification - 1.0) * 2.7;
        let shell_width = base_content + expansion + 32.0;
        let bouncing = self.launch_bounce.contains_key(&(hwnd.0 as isize));
        Some(DockInteractionGeometry {
            scale,
            width,
            height,
            count: items.len(),
            separator,
            icon_size,
            magnification,
            hidden: hide_progress >= 0.98,
            bouncing,
            shell_left: (width - shell_width) / 2.0,
            shell_top,
            shell_right: (width + shell_width) / 2.0,
            shell_bottom,
        })
    }

    fn paint(&mut self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        unsafe {
            BeginPaint(hwnd, &mut ps);
        }
        let result = match self.kinds.get(&(hwnd.0 as isize)).copied() {
            Some(WindowKind::Top) => {
                let active = active_app_name(&self.windows);
                let status = self.system_status;
                let clock = current_clock(self.config.top_bar.use_24_hour_clock);
                let date = current_date();
                let hover = self
                    .hover
                    .get(&(hwnd.0 as isize))
                    .copied()
                    .flatten()
                    .and_then(TopBarSegment::decode);
                self.renderer.paint_top_bar(
                    hwnd,
                    &active,
                    &clock,
                    &date,
                    TopBarStatus {
                        battery_percent: status.battery_percent,
                        charging: status.charging,
                        network_online: status.network_online,
                        volume_percent: status.volume_percent,
                        muted: status.muted,
                    },
                    TopBarSegmentFlags {
                        show_network: self.config.top_bar.show_network,
                        show_volume: self.config.top_bar.show_volume,
                        show_battery: self.config.top_bar.show_battery,
                    },
                    hover,
                )
            }
            Some(WindowKind::Dock) => {
                let items = self.dock_items(hwnd);
                let visuals: Vec<_> = items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let utility = matches!(item, DockItem::Folder(_) | DockItem::RecycleBin);
                        let previous_is_app = index > 0
                            && matches!(items[index - 1], DockItem::Application { .. });
                        let (icon_path, fallback_icon_path) = dock_icon_paths(item);
                        DockVisual {
                            label: dock_label(item),
                            running: matches!(item, DockItem::Application { windows, .. } if !windows.is_empty()),
                            icon_path,
                            fallback_icon_path,
                            separator_before: utility && previous_is_app,
                            recycle_bin: matches!(item, DockItem::RecycleBin),
                            folder: matches!(item, DockItem::Folder(_)),
                        }
                    })
                    .collect();
                let hover = self.hover.get(&(hwnd.0 as isize)).copied().flatten();
                let hover_x = self.cursor_x.get(&(hwnd.0 as isize)).copied();
                let dock_hover = hover.zip(hover_x).map(|(index, x)| DockHover { index, x });
                let icon_size = self
                    .monitor_for(hwnd)
                    .map(|shell| shell.icon_size)
                    .unwrap_or(self.config.dock.icon_size);
                let progress = self
                    .animation
                    .get(&(hwnd.0 as isize))
                    .copied()
                    .unwrap_or(1.0);
                let magnification = if self.config.behavior.reduce_motion {
                    1.0
                } else {
                    1.0 + (self.config.dock.magnification - 1.0) * progress
                };
                let bounce = self.launch_bounce.get(&(hwnd.0 as isize)).map(|state| {
                    let frame = launch_bounce_frame(state.started.elapsed(), icon_size);
                    DockBounce {
                        item: state.item,
                        offset: frame.offset,
                        scale_x: frame.scale_x,
                        scale_y: frame.scale_y,
                    }
                });
                let hide_progress = self
                    .auto_hide
                    .get(&(hwnd.0 as isize))
                    .map_or(0.0, |state| state.progress);
                let drag_index = self
                    .drag
                    .as_ref()
                    .filter(|drag| drag.hwnd == hwnd)
                    .and_then(|drag| {
                        items
                            .iter()
                            .position(|item| dock_pin(item).is_some_and(|pin| pin == &drag.pin))
                    });
                let dragging = self
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.hwnd == hwnd && drag.active)
                    .then_some(drag_index)
                    .flatten();
                let pressed = self
                    .drag
                    .as_ref()
                    .is_some_and(|drag| drag.hwnd == hwnd && !drag.active)
                    .then_some(drag_index)
                    .flatten();
                let reorder = self
                    .reorder_animation
                    .get(&(hwnd.0 as isize))
                    .map(|animation| {
                        let progress =
                            (animation.started.elapsed().as_secs_f32() / 0.18).clamp(0.0, 1.0);
                        (animation.from, animation.to, ease_out_quart(progress))
                    });
                self.renderer.paint_dock(
                    hwnd,
                    &visuals,
                    icon_size,
                    DockRenderState {
                        hover: dock_hover,
                        magnification,
                        bounce,
                        hide_progress,
                        dragging,
                        pressed,
                        reorder,
                    },
                )
            }
            Some(WindowKind::Preview) => {
                if let Some(preview) = self.preview.as_ref().filter(|preview| preview.hwnd == hwnd)
                {
                    self.renderer.paint_preview(
                        hwnd,
                        &preview.title,
                        preview.close_hover,
                        preview.pointer_x,
                    )
                } else {
                    Ok(())
                }
            }
            Some(WindowKind::FolderStack) => {
                if let Some(stack) = self
                    .folder_stack
                    .as_ref()
                    .filter(|stack| stack.hwnd == hwnd)
                {
                    let entries: Vec<_> = stack
                        .entries
                        .iter()
                        .map(|path| {
                            (
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                                path.clone(),
                            )
                        })
                        .collect();
                    self.renderer.paint_folder_stack(
                        hwnd,
                        &stack.title,
                        &entries,
                        stack.hover,
                        stack.footer_hover,
                        stack.pointer_x,
                    )
                } else {
                    Ok(())
                }
            }
            // Animation frames are painted directly by the timer. Repainting here would
            // clear the captured window between frames and cause a visible flash.
            Some(WindowKind::LaunchOverlay) => Ok(()),
            Some(WindowKind::DebugPopover) => self.renderer.paint_debug_popover(hwnd),
            Some(WindowKind::Launcher) => match self.launcher.as_ref() {
                Some(launcher) => {
                    let visible = launcher
                        .apps
                        .len()
                        .min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
                    let apps_window: Vec<(String, std::path::PathBuf)> = launcher
                        .apps
                        .iter()
                        .skip(launcher.scroll)
                        .take(visible)
                        .cloned()
                        .collect();
                    self.renderer.paint_launcher(
                        hwnd,
                        LAUNCHER_ACTION_LABELS,
                        &apps_window,
                        launcher.hover,
                    )
                }
                None => Ok(()),
            },
            _ => Ok(()),
        };
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        if let Err(error) = result {
            crate::yume_err!("paint failed: {error:#}");
        }
    }

    fn mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let x = (x as f32 / scale).round() as i32;
        let y = (y as f32 / scale).round() as i32;
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::FolderStack) {
            self.folder_stack_mouse_move(hwnd, x, y);
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Preview) {
            if let Some(preview) = self.preview.as_mut() {
                unsafe {
                    let _ = KillTimer(Some(preview.dock), 3);
                }
                let close_hover = (8..=30).contains(&x) && (8..=30).contains(&y);
                if preview.close_hover != close_hover {
                    preview.close_hover = close_hover;
                    unsafe {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                ..Default::default()
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Top) {
            // Menu-bar segment hover: hit-test against the laid-out segments.
            let status = self.system_status;
            let active = active_app_name(&self.windows);
            let (width, height) = client_size_dips(hwnd, scale);
            let geo = crate::render::top_bar_geometry(
                width,
                height,
                &active,
                TopBarStatus {
                    battery_percent: status.battery_percent,
                    charging: status.charging,
                    network_online: status.network_online,
                    volume_percent: status.volume_percent,
                    muted: status.muted,
                },
                TopBarSegmentFlags {
                    show_network: self.config.top_bar.show_network,
                    show_volume: self.config.top_bar.show_volume,
                    show_battery: self.config.top_bar.show_battery,
                },
            );
            let hit = crate::render::top_bar_hit_test(&geo, x as f32).map(TopBarSegment::encode);
            let previous = self.hover.get(&(hwnd.0 as isize)).copied().flatten();
            self.cursor_x.insert(hwnd.0 as isize, x as f32);
            if previous != hit {
                self.hover.insert(hwnd.0 as isize, hit);
                unsafe {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                ..Default::default()
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) != Some(&WindowKind::Dock) {
            return;
        }
        if let Some(stack) = self.folder_stack.as_ref() {
            unsafe {
                let _ = KillTimer(Some(stack.hwnd), 10);
            }
        }
        self.reveal_dock(hwnd);
        let items = self.dock_items(hwnd);
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let icon_size = self
            .monitor_for(hwnd)
            .map(|shell| shell.icon_size)
            .unwrap_or(self.config.dock.icon_size);
        let previous = self.hover.get(&(hwnd.0 as isize)).copied().flatten();
        self.cursor_x.insert(hwnd.0 as isize, x as f32);
        let progress = self
            .animation
            .get(&(hwnd.0 as isize))
            .copied()
            .unwrap_or(1.0);
        let magnification = if self.config.behavior.reduce_motion {
            1.0
        } else {
            1.0 + (self.config.dock.magnification - 1.0) * progress
        };
        let separator = items.iter().enumerate().find_map(|(index, item)| {
            (index > 0
                && matches!(item, DockItem::Folder(_) | DockItem::RecycleBin)
                && matches!(items[index - 1], DockItem::Application { .. }))
            .then_some(index)
        });
        let next = crate::render::dock_hit_test(
            x as f32,
            (rect.right - rect.left) as f32 / scale,
            items.len(),
            icon_size,
            magnification,
            separator,
        );
        self.update_drag(hwnd, x, next, &items);
        let changed = previous != next;
        self.hover.insert(hwnd.0 as isize, next);
        if changed {
            // macOS Dock hover is icon magnification and a label, not a large
            // persistent taskbar-style window thumbnail. The preview also
            // obscured the app and could remain open after pointer transitions.
            self.cancel_preview_candidate(hwnd);
            self.close_preview();
            if previous.is_none() {
                self.animation.insert(
                    hwnd.0 as isize,
                    if self.config.behavior.reduce_motion {
                        1.0
                    } else {
                        0.0
                    },
                );
                self.animation_clock.insert(hwnd.0 as isize, Instant::now());
            }
            unsafe { SetTimer(Some(hwnd), 2, 16, None) };
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        let mut track = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            ..Default::default()
        };
        unsafe {
            let _ = TrackMouseEvent(&mut track);
        }
    }

    fn mouse_leave(&mut self, hwnd: HWND) {
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::FolderStack) {
            unsafe {
                SetTimer(Some(hwnd), 10, 250, None);
            }
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Preview) {
            self.close_preview();
            return;
        }
        self.cancel_preview_candidate(hwnd);
        self.cursor_x.remove(&(hwnd.0 as isize));
        self.hover.insert(hwnd.0 as isize, None);
        if self.preview.is_some() {
            unsafe {
                SetTimer(Some(hwnd), 3, 250, None);
            }
        }
        if self.config.behavior.reduce_motion {
            self.animation.insert(hwnd.0 as isize, 0.0);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        } else {
            self.animation_clock.insert(hwnd.0 as isize, Instant::now());
            unsafe {
                SetTimer(Some(hwnd), 2, 16, None);
            }
        }
        if self.config.dock.auto_hide {
            unsafe {
                SetTimer(Some(hwnd), 8, self.config.dock.auto_hide_delay_ms, None);
            }
        }
    }

    fn animate(&mut self, hwnd: HWND) {
        let target = if self
            .hover
            .get(&(hwnd.0 as isize))
            .copied()
            .flatten()
            .is_some()
        {
            1.0
        } else {
            0.0
        };
        let value = self.animation.entry(hwnd.0 as isize).or_default();
        let now = Instant::now();
        let last = self.animation_clock.entry(hwnd.0 as isize).or_insert(now);
        let elapsed = now.saturating_duration_since(*last);
        *last = now;
        *value = approach_value(
            *value,
            target,
            elapsed,
            Duration::from_millis(self.config.dock.animation_ms.max(60) as u64),
        );
        if (*value - target).abs() < 0.015 {
            *value = target;
            unsafe {
                let _ = KillTimer(Some(hwnd), 2);
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn reveal_dock(&mut self, hwnd: HWND) {
        if !self.config.dock.auto_hide {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(hwnd), 8);
        }
        let state = self
            .auto_hide
            .entry(hwnd.0 as isize)
            .or_insert(AutoHideState {
                progress: 0.0,
                target: 0.0,
                last_tick: Instant::now(),
            });
        state.target = 0.0;
        state.last_tick = Instant::now();
        if self.config.behavior.reduce_motion {
            state.progress = 0.0;
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        } else {
            unsafe {
                SetTimer(Some(hwnd), 7, 16, None);
            }
        }
    }

    fn begin_auto_hide(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), 8);
        }
        if !self.config.dock.auto_hide
            || self
                .hover
                .get(&(hwnd.0 as isize))
                .copied()
                .flatten()
                .is_some()
            || self.preview.is_some()
            || self.launch_bounce.contains_key(&(hwnd.0 as isize))
            || self.drag.is_some()
            || self.reorder_animation.contains_key(&(hwnd.0 as isize))
            || self.folder_stack.is_some()
        {
            return;
        }
        let state = self
            .auto_hide
            .entry(hwnd.0 as isize)
            .or_insert(AutoHideState {
                progress: 0.0,
                target: 1.0,
                last_tick: Instant::now(),
            });
        state.target = 1.0;
        state.last_tick = Instant::now();
        if self.config.behavior.reduce_motion {
            state.progress = 1.0;
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        } else {
            unsafe {
                SetTimer(Some(hwnd), 7, 16, None);
            }
        }
    }

    fn animate_auto_hide(&mut self, hwnd: HWND) {
        let configured_duration = self.config.dock.animation_ms.max(80) as f32;
        let Some(state) = self.auto_hide.get_mut(&(hwnd.0 as isize)) else {
            return;
        };
        let now = Instant::now();
        let elapsed = now.saturating_duration_since(state.last_tick);
        state.last_tick = now;
        let duration = Duration::from_millis(
            (configured_duration
                * if state.target < state.progress {
                    0.85
                } else {
                    1.15
                })
            .round() as u64,
        );
        state.progress = approach_value(state.progress, state.target, elapsed, duration);
        if (state.progress - state.target).abs() < 0.01 {
            state.progress = state.target;
            unsafe {
                let _ = KillTimer(Some(hwnd), 7);
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn animate_reorder(&mut self, hwnd: HWND) {
        let finished = self
            .reorder_animation
            .get(&(hwnd.0 as isize))
            .is_none_or(|animation| animation.started.elapsed() >= Duration::from_millis(180));
        if finished {
            self.reorder_animation.remove(&(hwnd.0 as isize));
            unsafe {
                let _ = KillTimer(Some(hwnd), 9);
                if self.config.dock.auto_hide {
                    SetTimer(Some(hwnd), 8, self.config.dock.auto_hide_delay_ms, None);
                }
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn start_launch_bounce(&mut self, hwnd: HWND, item: usize, key: String, wait_for_window: bool) {
        if self.config.behavior.reduce_motion {
            return;
        }
        self.reveal_dock(hwnd);
        self.launch_bounce.insert(
            hwnd.0 as isize,
            LaunchBounce {
                item,
                key,
                wait_for_window,
                started: Instant::now(),
                finish_after: (!wait_for_window).then_some(Duration::from_millis(620)),
            },
        );
        unsafe {
            SetTimer(Some(hwnd), 4, 16, None);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn cancel_launch_feedback(&mut self) {
        self.close_window_open_animation(true);
        self.pending_window_open = None;
        self.launch_bounce.clear();
        for shell in &self.shells {
            if let Some(state) = self.auto_hide.get_mut(&(shell.dock.0 as isize)) {
                state.progress = state.target;
                state.last_tick = Instant::now();
            }
            unsafe {
                let _ = KillTimer(Some(shell.dock), 4);
                let _ = KillTimer(Some(shell.dock), 7);
                let _ = InvalidateRect(Some(shell.dock), None, false);
                if self.config.dock.auto_hide {
                    SetTimer(
                        Some(shell.dock),
                        8,
                        self.config.dock.auto_hide_delay_ms,
                        None,
                    );
                }
            }
        }
    }

    fn animate_launch_bounce(&mut self, hwnd: HWND) {
        let finished = self
            .launch_bounce
            .get(&(hwnd.0 as isize))
            .is_none_or(|state| {
                state.started.elapsed() >= Duration::from_secs(8)
                    || state
                        .finish_after
                        .is_some_and(|finish| state.started.elapsed() >= finish)
            });
        if finished {
            self.launch_bounce.remove(&(hwnd.0 as isize));
            unsafe {
                let _ = KillTimer(Some(hwnd), 4);
                if self.config.dock.auto_hide {
                    SetTimer(Some(hwnd), 8, self.config.dock.auto_hide_delay_ms, None);
                }
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn dock_icon_screen_rect(&self, dock: HWND, index: usize) -> Option<RECT> {
        let hide_progress = self
            .auto_hide
            .get(&(dock.0 as isize))
            .map_or(0.0, |state| state.progress);
        self.dock_icon_screen_rect_at_hide_progress(dock, index, hide_progress)
    }

    fn dock_icon_screen_rect_at_hide_progress(
        &self,
        dock: HWND,
        index: usize,
        hide_progress: f32,
    ) -> Option<RECT> {
        let geometry = self.dock_interaction_geometry(dock)?;
        let icon = crate::render::dock_icon_rect(
            geometry.width,
            geometry.height,
            geometry.count,
            geometry.icon_size,
            self.cursor_x.get(&(dock.0 as isize)).copied(),
            geometry.magnification,
            geometry.separator,
            hide_progress,
            index,
        )?;
        let mut dock_rect = RECT::default();
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(dock, &mut dock_rect).ok()?;
        }
        Some(RECT {
            left: dock_rect.left + (icon.left * geometry.scale).round() as i32,
            top: dock_rect.top + (icon.top * geometry.scale).round() as i32,
            right: dock_rect.left + (icon.right * geometry.scale).round() as i32,
            bottom: dock_rect.top + (icon.bottom * geometry.scale).round() as i32,
        })
    }

    fn start_window_open_animation(
        &mut self,
        dock: HWND,
        source: HWND,
        origin_screen: RECT,
        minimize_source: bool,
    ) {
        self.start_window_transition(dock, source, origin_screen, minimize_source, true);
    }

    fn start_window_minimize_animation(&mut self, dock: HWND, source: HWND, origin_screen: RECT) {
        self.start_window_transition(dock, source, origin_screen, true, false);
    }

    fn handle_native_minimize(&mut self, source: HWND) {
        if source.is_invalid()
            || self
                .window_open_animation
                .as_ref()
                .is_some_and(|animation| animation.source == source)
        {
            return;
        }
        let docks: Vec<_> = self.shells.iter().map(|shell| shell.dock).collect();
        let target = docks.into_iter().find_map(|dock| {
            let items = self.dock_items(dock);
            items
                .iter()
                .position(|item| {
                    matches!(item, DockItem::Application { windows, .. } if windows.contains(&source))
                })
                .map(|index| (dock, index))
        });
        let Some((dock, index)) = target else {
            return;
        };
        let Some(origin) = self.dock_icon_screen_rect_at_hide_progress(dock, index, 0.0) else {
            return;
        };
        self.start_window_minimize_animation(dock, source, origin);
    }

    fn reveal_dock_for_transition(&mut self, dock: HWND) {
        if !self.config.dock.auto_hide {
            return;
        }
        if let Some(state) = self.auto_hide.get_mut(&(dock.0 as isize)) {
            state.progress = 0.0;
            state.target = 0.0;
            state.last_tick = Instant::now();
        }
        unsafe {
            let _ = KillTimer(Some(dock), 7);
            let _ = KillTimer(Some(dock), 8);
            let _ = InvalidateRect(Some(dock), None, false);
        }
        self.paint(dock);
    }

    fn start_window_transition(
        &mut self,
        dock: HWND,
        source: HWND,
        origin_screen: RECT,
        _minimize_source: bool,
        opening: bool,
    ) {
        self.close_window_open_animation(false);
        let was_minimized = unsafe { IsIconic(source).as_bool() };
        if self.config.behavior.reduce_motion {
            settle_window_transition(source, opening, was_minimized);
            return;
        }

        let mut target_screen = RECT::default();
        let got_target = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowRect(source, &mut target_screen)
                .is_ok()
        };
        if was_minimized
            || !got_target
            || rect_width(target_screen) < 120
            || rect_height(target_screen) < 80
        {
            let mut placement = WINDOWPLACEMENT {
                length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
                ..Default::default()
            };
            if unsafe { GetWindowPlacement(source, &mut placement).is_ok() } {
                target_screen = placement.rcNormalPosition;
            }
        }
        if rect_width(target_screen) < 120 || rect_height(target_screen) < 80 {
            settle_window_transition(source, opening, was_minimized);
            return;
        }

        let source_size = SIZE {
            cx: rect_width(target_screen),
            cy: rect_height(target_screen),
        };
        let icon_rect = launch_origin_rect(origin_screen, source_size);
        let overlay_bounds = RECT {
            left: target_screen.left.min(icon_rect.left) - 2,
            top: target_screen.top.min(icon_rect.top) - 2,
            right: target_screen.right.max(icon_rect.right) + 2,
            bottom: target_screen.bottom.max(icon_rect.bottom) + 2,
        };
        let Ok(overlay) = create_window(
            overlay_bounds.left,
            overlay_bounds.top,
            rect_width(overlay_bounds),
            rect_height(overlay_bounds),
            false,
        ) else {
            settle_window_transition(source, opening, was_minimized);
            return;
        };
        configure_window_backdrop(overlay, false, self.high_contrast);
        self.kinds
            .insert(overlay.0 as isize, WindowKind::LaunchOverlay);

        if self
            .renderer
            .prepare_genie_snapshot(
                overlay,
                source,
                source_size.cx,
                source_size.cy,
                was_minimized,
            )
            .is_err()
        {
            self.kinds.remove(&(overlay.0 as isize));
            self.renderer.forget(overlay);
            unsafe {
                let _ = DestroyWindow(overlay);
            }
            settle_window_transition(source, opening, was_minimized);
            return;
        }

        // Capture while an auto-hidden Dock is still off-screen, then reveal it.
        // Otherwise the Dock itself would be baked into the app snapshot.
        self.reveal_dock_for_transition(dock);

        let destinations =
            genie_frame_rects(target_screen, icon_rect, overlay_bounds, 0.0, opening);
        if self
            .renderer
            .paint_genie(overlay, &destinations, 1.0)
            .is_err()
        {
            self.kinds.remove(&(overlay.0 as isize));
            self.renderer.forget(overlay);
            unsafe {
                let _ = DestroyWindow(overlay);
            }
            settle_window_transition(source, opening, was_minimized);
            return;
        }

        unsafe {
            let _ = ShowWindow(overlay, SW_SHOWNA);
            if !was_minimized {
                let _ = ShowWindow(source, SW_MINIMIZE);
            }
            self.window_open_animation = Some(WindowOpenAnimation {
                dock,
                overlay,
                source,
                window_rect: target_screen,
                icon_rect,
                overlay_bounds,
                started: Instant::now(),
                restore_at_end: opening,
                opening,
            });
            // Submit faster than the compositor refresh so DWM always has a fresh frame.
            SetTimer(Some(overlay), 11, 8, None);
        }
    }

    fn animate_window_open(&mut self, hwnd: HWND) {
        let Some(animation) = self
            .window_open_animation
            .as_ref()
            .filter(|animation| animation.overlay == hwnd)
        else {
            unsafe {
                let _ = KillTimer(Some(hwnd), 11);
            }
            return;
        };
        let duration = if animation.opening { 0.20 } else { 0.16 };
        let progress = (animation.started.elapsed().as_secs_f32() / duration).clamp(0.0, 1.0);
        let destinations = genie_frame_rects(
            animation.window_rect,
            animation.icon_rect,
            animation.overlay_bounds,
            progress,
            animation.opening,
        );
        let painted = self.renderer.paint_genie(hwnd, &destinations, 1.0).is_ok();
        if progress >= 1.0 || !painted {
            self.close_window_open_animation(animation.opening);
        }
    }

    fn close_window_open_animation(&mut self, focus_source: bool) {
        let Some(animation) = self.window_open_animation.take() else {
            return;
        };
        unsafe {
            if animation.restore_at_end {
                let _ = ShowWindow(animation.source, SW_RESTORE);
            }
            if focus_source && animation.opening {
                let _ = SetForegroundWindow(animation.source);
            }
            let _ = KillTimer(Some(animation.overlay), 11);
            if self.config.dock.auto_hide {
                SetTimer(
                    Some(animation.dock),
                    8,
                    self.config.dock.auto_hide_delay_ms,
                    None,
                );
            }
        }
        self.kinds.remove(&(animation.overlay.0 as isize));
        self.renderer.forget(animation.overlay);
        unsafe {
            let _ = DestroyWindow(animation.overlay);
        }
    }

    fn show_preview(&mut self, dock: HWND, source: Option<HWND>) {
        self.close_preview();
        let Some(source) = source else { return };
        let mut dock_rect = RECT::default();
        unsafe {
            if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(dock, &mut dock_rect).is_err()
            {
                return;
            }
        }
        let title = self
            .windows
            .iter()
            .find(|entry| entry.hwnd == source)
            .map(|entry| entry.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Window Preview".to_string());
        let mut source_rect = RECT::default();
        unsafe {
            let _ =
                windows::Win32::UI::WindowsAndMessaging::GetWindowRect(source, &mut source_rect);
        }
        let source_width = (source_rect.right - source_rect.left).max(1) as f32;
        let source_height = (source_rect.bottom - source_rect.top).max(1) as f32;
        let scale = self
            .monitor_for(dock)
            .map(|shell| shell.scale)
            .unwrap_or_else(|| window_scale(dock));
        let content_height = scale_i32(180, scale);
        let content_width = ((content_height as f32 * source_width / source_height).round() as i32)
            .clamp(scale_i32(220, scale), scale_i32(360, scale));
        let width = content_width + scale_i32(16, scale);
        let height = scale_i32(232, scale);
        let anchor_x = dock_rect.left
            + (self
                .cursor_x
                .get(&(dock.0 as isize))
                .copied()
                .unwrap_or((dock_rect.right - dock_rect.left) as f32 / (2.0 * scale))
                * scale)
                .round() as i32;
        let monitor_bounds = self
            .monitor_for(dock)
            .map(|shell| shell.info.bounds)
            .unwrap_or(dock_rect);
        let margin = scale_i32(8, scale);
        let min_x = monitor_bounds.left + margin;
        let max_x = (monitor_bounds.right - width - margin).max(min_x);
        let x = (anchor_x - width / 2).clamp(min_x, max_x);
        let icon_size = self
            .monitor_for(dock)
            .map(|shell| shell.icon_size)
            .unwrap_or(self.config.dock.icon_size);
        let preview_bottom =
            dock_rect.bottom - (icon_size * scale).ceil() as i32 - scale_i32(36, scale);
        let min_y =
            monitor_bounds.top + scale_i32(self.config.top_bar.height, scale) + scale_i32(8, scale);
        let y = (preview_bottom - height).max(min_y);
        let pointer_x = (anchor_x - x) as f32 / scale;
        let Ok(hwnd) = create_window(x, y, width, height, false) else {
            return;
        };
        configure_window_backdrop(hwnd, true, self.high_contrast);
        unsafe {
            let Ok(thumbnail) = DwmRegisterThumbnail(hwnd, source) else {
                let _ = DestroyWindow(hwnd);
                return;
            };
            let source_size = DwmQueryThumbnailSourceSize(thumbnail).unwrap_or(SIZE {
                cx: source_width.round() as i32,
                cy: source_height.round() as i32,
            });
            let properties = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_VISIBLE | DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY,
                rcDestination: preview_content_rect(source_size, width, height, scale),
                opacity: 255,
                fVisible: true.into(),
                ..Default::default()
            };
            if DwmUpdateThumbnailProperties(thumbnail, &properties).is_err() {
                let _ = DwmUnregisterThumbnail(thumbnail);
                let _ = DestroyWindow(hwnd);
                return;
            }
            self.kinds.insert(hwnd.0 as isize, WindowKind::Preview);
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            self.preview = Some(Preview {
                hwnd,
                thumbnail,
                source,
                dock,
                title,
                close_hover: false,
                pointer_x,
            });
        }
    }

    fn cancel_preview_candidate(&mut self, dock: HWND) {
        self.preview_candidate = None;
        unsafe {
            let _ = KillTimer(Some(dock), 6);
        }
    }

    fn show_pending_preview(&mut self, dock: HWND) {
        unsafe {
            let _ = KillTimer(Some(dock), 6);
        }
        let source = self
            .preview_candidate
            .take()
            .and_then(|(candidate_dock, source)| (candidate_dock == dock).then_some(source));
        self.show_preview(dock, source);
    }

    fn close_preview(&mut self) {
        if let Some(preview) = self.preview.take() {
            self.kinds.remove(&(preview.hwnd.0 as isize));
            self.renderer.forget(preview.hwnd);
            unsafe {
                let _ = DwmUnregisterThumbnail(preview.thumbnail);
                let _ = DestroyWindow(preview.hwnd);
            }
            if self.config.dock.auto_hide {
                unsafe {
                    SetTimer(
                        Some(preview.dock),
                        8,
                        self.config.dock.auto_hide_delay_ms,
                        None,
                    );
                }
            }
        }
    }

    fn activate_dock_item(&mut self, hwnd: HWND, new_instance: bool) {
        let Some(index) = self.hover.get(&(hwnd.0 as isize)).copied().flatten() else {
            return;
        };
        let items = self.dock_items(hwnd);
        let Some(item) = items.get(index) else { return };
        let keep_stack = matches!(item, DockItem::Folder(pin) if self.folder_stack.as_ref().is_some_and(|stack| stack.folder == pin.path));
        if !keep_stack {
            self.close_folder_stack();
        }
        match item {
            DockItem::Application { windows, .. } if !new_instance && !windows.is_empty() => {
                let foreground = unsafe { GetForegroundWindow() };
                let foreground_root = unsafe { GetAncestor(foreground, GA_ROOTOWNER) };
                let active_window = windows
                    .iter()
                    .copied()
                    .find(|window| *window == foreground || *window == foreground_root);
                if let Some(active_window) = active_window {
                    self.close_window_open_animation(false);
                    if let Some(origin) = self.dock_icon_screen_rect(hwnd, index) {
                        self.start_window_minimize_animation(hwnd, active_window, origin);
                    } else {
                        unsafe {
                            let _ = ShowWindow(active_window, SW_MINIMIZE);
                        }
                    }
                    return;
                }
                let key = dock_label(item).to_lowercase();
                let cycle = self.cycle_index.entry(key).or_default();
                let target = windows[*cycle % windows.len()];
                *cycle = (*cycle + 1) % windows.len();
                if let Some(key) = dock_identity_key(item) {
                    self.start_launch_bounce(hwnd, index, key, false);
                }
                if let Some(origin) = self.dock_icon_screen_rect(hwnd, index) {
                    self.start_window_open_animation(hwnd, target, origin, false);
                } else {
                    unsafe {
                        let _ = ShowWindow(target, SW_RESTORE);
                        let _ = SetForegroundWindow(target);
                    }
                }
            }
            DockItem::Application {
                pin: Some(pin),
                windows,
                ..
            } => {
                if let Some(key) = dock_identity_key(item) {
                    let wait_for_window = windows.is_empty();
                    self.start_launch_bounce(hwnd, index, key.clone(), wait_for_window);
                    if wait_for_window && let Some(origin) = self.dock_icon_screen_rect(hwnd, index)
                    {
                        self.pending_window_open = Some(PendingWindowOpen {
                            dock: hwnd,
                            key,
                            origin,
                            started: Instant::now(),
                        });
                    }
                }
                launch_path(&pin.path);
            }
            DockItem::Folder(pin) => self.show_folder_stack(hwnd, pin),
            DockItem::Application {
                identity: Some(identity),
                windows,
                ..
            } => {
                if let Some(key) = dock_identity_key(item) {
                    let wait_for_window = windows.is_empty();
                    self.start_launch_bounce(hwnd, index, key.clone(), wait_for_window);
                    if wait_for_window && let Some(origin) = self.dock_icon_screen_rect(hwnd, index)
                    {
                        self.pending_window_open = Some(PendingWindowOpen {
                            dock: hwnd,
                            key,
                            origin,
                            started: Instant::now(),
                        });
                    }
                }
                launch_path(&identity.executable);
            }
            DockItem::RecycleBin => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("shell:RecycleBinFolder"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            _ => {}
        }
    }

    fn mouse_down(&mut self, hwnd: HWND, x: i32) {
        if self.active_popover.is_some() {
            self.close_popover();
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) != Some(&WindowKind::Dock) {
            return;
        }
        let x = (x as f32 / window_scale(hwnd)).round() as i32;
        let pin = self
            .hover
            .get(&(hwnd.0 as isize))
            .copied()
            .flatten()
            .and_then(|index| self.dock_items(hwnd).get(index).and_then(dock_pin).cloned());
        let Some(pin) = pin else { return };
        self.drag = Some(DockDrag {
            hwnd,
            pin,
            start_x: x,
            active: false,
        });
        unsafe {
            SetCapture(hwnd);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    fn update_drag(&mut self, hwnd: HWND, x: i32, target: Option<usize>, items: &[DockItem]) {
        let Some(mut drag) = self.drag.take() else {
            return;
        };
        if drag.hwnd != hwnd {
            self.drag = Some(drag);
            return;
        }
        let became_active = !drag.active && (x - drag.start_x).abs() >= 6;
        drag.active |= became_active;
        let source_index = items
            .iter()
            .position(|item| dock_pin(item).is_some_and(|pin| pin == &drag.pin));
        let mut reordered = false;
        if drag.active
            && let Some(target_pin) = target
                .and_then(|index| items.get(index))
                .and_then(dock_pin)
                .cloned()
        {
            reordered = swap_compatible_pins(&mut self.config.pins, &drag.pin, &target_pin);
        }
        if reordered {
            self.rebuild_dock_cache();
        }
        if reordered
            && !self.config.behavior.reduce_motion
            && let (Some(from), Some(to)) = (source_index, target)
        {
            self.reorder_animation.insert(
                hwnd.0 as isize,
                ReorderAnimation {
                    from,
                    to,
                    started: Instant::now(),
                },
            );
            unsafe {
                SetTimer(Some(hwnd), 9, 16, None);
            }
        }
        self.drag = Some(drag);
        if became_active {
            self.cancel_preview_candidate(hwnd);
            self.close_preview();
        }
        if became_active || reordered {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn mouse_up(&mut self, hwnd: HWND) {
        let active = self
            .drag
            .take()
            .filter(|drag| drag.hwnd == hwnd)
            .is_some_and(|drag| drag.active);
        unsafe {
            let _ = ReleaseCapture();
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
        if active {
            let _ = self.config.save();
            if self.config.dock.auto_hide {
                unsafe {
                    SetTimer(Some(hwnd), 8, self.config.dock.auto_hide_delay_ms, None);
                }
            }
        } else {
            self.left_click(hwnd);
        }
    }

    fn cancel_drag(&mut self, hwnd: HWND) {
        if self.drag.as_ref().is_some_and(|drag| drag.hwnd == hwnd) {
            self.drag = None;
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn left_click(&mut self, hwnd: HWND) {
        match self.kinds.get(&(hwnd.0 as isize)).copied() {
            Some(WindowKind::Top) => {
                // Dispatch by hovered segment; fall back to the full menu when
                // the click lands in a gap or on the logo/app cluster.
                let segment = self
                    .hover
                    .get(&(hwnd.0 as isize))
                    .copied()
                    .flatten()
                    .and_then(TopBarSegment::decode);
                match segment {
                    Some(TopBarSegment::Logo) => self.open_launcher(hwnd),
                    Some(TopBarSegment::App) => self.show_top_menu(hwnd),
                    Some(
                        TopBarSegment::Network | TopBarSegment::Volume | TopBarSegment::Battery,
                    ) => self.handle_command(CMD_QUICK_SETTINGS),
                    Some(TopBarSegment::Clock) => self.handle_command(CMD_NOTIFICATION_CENTER),
                    _ => self.show_top_menu(hwnd),
                }
            }
            Some(WindowKind::Dock) => self.activate_dock_item(hwnd, false),
            Some(WindowKind::Preview) => {
                if let Some(preview) = self.preview.as_ref() {
                    let source = preview.source;
                    let close = preview.close_hover;
                    unsafe {
                        if close {
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(source),
                                WM_CLOSE,
                                Default::default(),
                                Default::default(),
                            );
                        } else {
                            let _ = ShowWindow(source, SW_RESTORE);
                            let _ = SetForegroundWindow(source);
                        }
                    }
                    self.close_preview();
                }
            }
            Some(WindowKind::FolderStack) => self.folder_stack_click(hwnd),
            _ => {}
        }
    }

    fn folder_stack_click(&mut self, hwnd: HWND) {
        let target = self
            .folder_stack
            .as_ref()
            .filter(|stack| stack.hwnd == hwnd)
            .and_then(|stack| {
                if stack.footer_hover {
                    Some(stack.folder.clone())
                } else {
                    stack
                        .hover
                        .and_then(|index| stack.entries.get(index).cloned())
                }
            });
        if let Some(path) = target {
            launch_path(&path);
            self.close_folder_stack();
        }
    }

    fn show_menu(&mut self, hwnd: HWND) {
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::FolderStack) {
            self.close_folder_stack();
            return;
        }
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Dock)
            && let Some(index) = self.hover.get(&(hwnd.0 as isize)).copied().flatten()
            && let Some(item) = self.dock_items(hwnd).get(index).cloned()
        {
            self.show_item_menu(hwnd, item);
            return;
        }
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("Open settings file"));
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.behavior.reduce_motion {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_REDUCE_MOTION,
                w!("Reduce motion"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.dock.auto_hide {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_AUTO_HIDE,
                w!("Automatically Hide and Show the Dock"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.behavior.replace_taskbar {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_TASKBAR,
                w!("Replace Windows taskbar"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.behavior.start_with_windows {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_STARTUP,
                w!("Start with Windows"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.top_bar.show_network {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_SHOW_NETWORK,
                w!("Show network"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.top_bar.show_volume {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_SHOW_VOLUME,
                w!("Show volume"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.top_bar.show_battery {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_SHOW_BATTERY,
                w!("Show battery"),
            );
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.top_bar.use_24_hour_clock {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_CLOCK_24_HOUR,
                w!("Use 24-hour clock"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_ICON_LARGER, w!("Larger icons"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_ICON_SMALLER, w!("Smaller icons"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_RESTORE_QUIT,
                w!("Restore taskbar and quit"),
            );
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let command =
                TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, Some(0), hwnd, None);
            let _ = DestroyMenu(menu);
            self.handle_command(command.0 as usize);
        }
    }

    fn show_top_menu(&mut self, hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, CMD_START_MENU, w!("Start"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUICK_SETTINGS, w!("Quick Settings"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_NOTIFICATION_CENTER,
                w!("Notifications & calendar"),
            );
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_MINIMIZE_ACTIVE,
                w!("Minimize active app"),
            );
            let _ = AppendMenuW(menu, MF_STRING, CMD_CLOSE_ACTIVE, w!("Close active app"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW_DESKTOP, w!("Show desktop"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_NETWORK_SETTINGS,
                w!("Network settings"),
            );
            let _ = AppendMenuW(menu, MF_STRING, CMD_SOUND_SETTINGS, w!("Sound settings"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_POWER_SETTINGS, w!("Power & battery"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_DATE_MENU, w!("Date & time"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_DOWN, w!("Volume down"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_UP, w!("Volume up"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_MUTE, w!("Mute / unmute"));
            let _ = AppendMenuW(
                menu,
                MF_STRING
                    | if self.config.dock.auto_hide {
                        MF_CHECKED
                    } else {
                        Default::default()
                    },
                CMD_AUTO_HIDE,
                w!("Automatically Hide and Show the Dock"),
            );
            let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("YumeDock settings"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_DEBUG_POPOVER, w!("Debug popover"));
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                CMD_RESTORE_QUIT,
                w!("Restore taskbar and quit"),
            );
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let command =
                TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, Some(0), hwnd, None);
            let _ = DestroyMenu(menu);
            self.handle_command(command.0 as usize);
        }
    }

    fn show_item_menu(&mut self, hwnd: HWND, item: DockItem) {
        self.menu_item = Some(item.clone());
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let _ = AppendMenuW(menu, MF_STRING, CMD_OPEN_ITEM, w!("Open"));
            if matches!(item, DockItem::Application { .. }) {
                let _ = AppendMenuW(menu, MF_STRING, CMD_NEW_INSTANCE, w!("New instance"));
                let pinned = matches!(item, DockItem::Application { pin: Some(_), .. });
                let _ = AppendMenuW(
                    menu,
                    MF_STRING,
                    CMD_TOGGLE_PIN,
                    if pinned {
                        w!("Unpin from YumeDock")
                    } else {
                        w!("Pin to YumeDock")
                    },
                );
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                let _ = AppendMenuW(menu, MF_STRING, CMD_MINIMIZE_ITEM, w!("Minimize"));
                let _ = AppendMenuW(menu, MF_STRING, CMD_CLOSE_ITEM, w!("Close"));
            }
            if matches!(item, DockItem::RecycleBin) {
                let _ = AppendMenuW(menu, MF_STRING, CMD_EMPTY_RECYCLE, w!("Empty Recycle Bin"));
            }
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let command =
                TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, Some(0), hwnd, None);
            let _ = DestroyMenu(menu);
            self.handle_command(command.0 as usize);
        }
        self.menu_item = None;
    }

    fn handle_command(&mut self, command: usize) {
        match command {
            CMD_DEBUG_POPOVER => {
                if let Some(owner) = self.shells.first().map(|shell| shell.top) {
                    self.toggle_debug_popover(owner);
                }
                return;
            }
            CMD_START_MENU => open_start_menu(),
            CMD_QUICK_SETTINGS => send_windows_shortcut(b'A'),
            CMD_NOTIFICATION_CENTER => send_windows_shortcut(b'N'),
            CMD_CLOCK_24_HOUR => {
                self.config.top_bar.use_24_hour_clock = !self.config.top_bar.use_24_hour_clock
            }
            CMD_SHOW_NETWORK => {
                self.config.top_bar.show_network = !self.config.top_bar.show_network
            }
            CMD_SHOW_VOLUME => self.config.top_bar.show_volume = !self.config.top_bar.show_volume,
            CMD_SHOW_BATTERY => {
                self.config.top_bar.show_battery = !self.config.top_bar.show_battery
            }
            CMD_SETTINGS => {
                if let Ok(path) = crate::config::app_data_dir().map(|p| p.join("config.json")) {
                    launch_path(&path);
                }
            }
            CMD_REDUCE_MOTION => {
                self.config.behavior.reduce_motion = !self.config.behavior.reduce_motion;
                if self.config.behavior.reduce_motion {
                    self.cancel_launch_feedback();
                }
            }
            CMD_AUTO_HIDE => {
                self.config.dock.auto_hide = !self.config.dock.auto_hide;
                self.rebuild_monitors();
            }
            CMD_TASKBAR if !self.safe_mode => {
                self.config.behavior.replace_taskbar = !self.config.behavior.replace_taskbar;
                if self.config.behavior.replace_taskbar {
                    self.taskbar.refresh_and_hide();
                } else {
                    self.taskbar.restore();
                }
            }
            CMD_STARTUP => {
                self.config.behavior.start_with_windows = !self.config.behavior.start_with_windows;
                let _ = crate::config::sync_startup(self.config.behavior.start_with_windows);
            }
            CMD_MINIMIZE_ACTIVE => unsafe {
                let active = GetForegroundWindow();
                let _ = ShowWindow(active, windows::Win32::UI::WindowsAndMessaging::SW_MINIMIZE);
            },
            CMD_CLOSE_ACTIVE => unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(GetForegroundWindow()),
                    WM_CLOSE,
                    Default::default(),
                    Default::default(),
                );
            },
            CMD_SHOW_DESKTOP => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("shell:::{3080F90D-D7AD-11D9-BD98-0000947B0257}"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            CMD_NETWORK_SETTINGS => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("ms-settings:network-status"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            CMD_SOUND_SETTINGS => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("ms-settings:sound"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            CMD_VOLUME_DOWN => send_media_key(VK_VOLUME_DOWN.0 as u8),
            CMD_VOLUME_UP => send_media_key(VK_VOLUME_UP.0 as u8),
            CMD_VOLUME_MUTE => send_media_key(VK_VOLUME_MUTE.0 as u8),
            CMD_POWER_SETTINGS => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("ms-settings:powersleep"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            CMD_POWER_SLEEP => power_action(PowerAction::Sleep),
            CMD_POWER_RESTART => power_action(PowerAction::Restart),
            CMD_POWER_SHUTDOWN => power_action(PowerAction::Shutdown),
            CMD_POWER_LOCK => power_action(PowerAction::Lock),
            CMD_DATE_MENU => unsafe {
                ShellExecuteW(
                    None,
                    w!("open"),
                    w!("ms-settings:datetimelanguage"),
                    None,
                    None,
                    SW_SHOW,
                );
            },
            CMD_OPEN_ITEM | CMD_NEW_INSTANCE => {
                if let Some(item) = self.menu_item.clone() {
                    open_item(&item, command == CMD_NEW_INSTANCE);
                }
            }
            CMD_TOGGLE_PIN => {
                if let Some(item) = self.menu_item.clone() {
                    self.toggle_pin(&item);
                }
            }
            CMD_MINIMIZE_ITEM => {
                if let Some(DockItem::Application { windows, .. }) = self.menu_item.as_ref() {
                    for hwnd in windows {
                        unsafe {
                            let _ = ShowWindow(
                                *hwnd,
                                windows::Win32::UI::WindowsAndMessaging::SW_MINIMIZE,
                            );
                        }
                    }
                }
            }
            CMD_CLOSE_ITEM => {
                if let Some(DockItem::Application { windows, .. }) = self.menu_item.as_ref() {
                    for hwnd in windows {
                        unsafe {
                            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                                Some(*hwnd),
                                WM_CLOSE,
                                Default::default(),
                                Default::default(),
                            );
                        }
                    }
                }
            }
            CMD_EMPTY_RECYCLE => unsafe {
                let _ = SHEmptyRecycleBinW(
                    None,
                    PCWSTR::null(),
                    SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI,
                );
            },
            CMD_ICON_LARGER => {
                self.config.dock.icon_size = (self.config.dock.icon_size + 4.0).min(80.0);
                self.rebuild_monitors();
            }
            CMD_ICON_SMALLER => {
                self.config.dock.icon_size = (self.config.dock.icon_size - 4.0).max(32.0);
                self.rebuild_monitors();
            }
            CMD_QUIT | CMD_RESTORE_QUIT => self.shutdown(),
            _ => {}
        }
        let _ = self.config.save();
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.dock), None, false);
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
    }

    fn toggle_pin(&mut self, item: &DockItem) {
        match item {
            DockItem::Application { pin: Some(pin), .. } => {
                self.config.pins.retain(|p| p.path != pin.path)
            }
            DockItem::Application {
                identity: Some(identity),
                ..
            } => self.config.pins.push(PinConfig {
                label: identity.display_name.clone(),
                path: identity.executable.clone(),
                identity_path: Some(identity.executable.clone()),
                kind: PinKind::Application,
            }),
            _ => {}
        }
        self.rebuild_monitors();
    }

    fn drop_files(&mut self, hdrop: HDROP) {
        unsafe {
            let count = DragQueryFileW(hdrop, u32::MAX, None);
            for index in 0..count {
                let len = DragQueryFileW(hdrop, index, None);
                let mut buffer = vec![0u16; len as usize + 1];
                DragQueryFileW(hdrop, index, Some(&mut buffer));
                let path =
                    std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
                if self.config.pins.iter().any(|pin| pin.path == path) {
                    continue;
                }
                let kind = if path.is_dir() {
                    PinKind::Folder
                } else {
                    PinKind::Application
                };
                let label = path
                    .file_stem()
                    .or_else(|| path.file_name())
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let identity_path = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
                    .then(|| crate::config::resolve_shortcut(&path))
                    .flatten()
                    .or_else(|| Some(path.clone()));
                self.config.pins.push(PinConfig {
                    label,
                    path,
                    identity_path,
                    kind,
                });
            }
            DragFinish(hdrop);
        }
        let _ = self.config.save();
        self.rebuild_monitors();
    }

    fn show_folder_stack(&mut self, hwnd: HWND, pin: &PinConfig) {
        if self
            .folder_stack
            .as_ref()
            .is_some_and(|stack| stack.folder == pin.path)
        {
            self.close_folder_stack();
            return;
        }
        self.close_folder_stack();
        self.close_preview();
        let mut entries: Vec<_> = std::fs::read_dir(&pin.path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort_by_key(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
        });
        entries.truncate(20);
        let scale = self
            .monitor_for(hwnd)
            .map(|shell| shell.scale)
            .unwrap_or_else(|| window_scale(hwnd));
        let rows = entries.len().max(1).div_ceil(STACK_COLUMNS) as i32;
        let width = scale_i32(
            STACK_PADDING * 2 + STACK_CELL_WIDTH * STACK_COLUMNS as i32,
            scale,
        );
        let height = scale_i32(
            STACK_HEADER + STACK_CELL_HEIGHT * rows + STACK_FOOTER + 16,
            scale,
        );
        let mut dock_rect = RECT::default();
        unsafe {
            if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut dock_rect).is_err()
            {
                return;
            }
        }
        let anchor_x = dock_rect.left
            + (self
                .cursor_x
                .get(&(hwnd.0 as isize))
                .copied()
                .unwrap_or((dock_rect.right - dock_rect.left) as f32 / (2.0 * scale))
                * scale)
                .round() as i32;
        let bounds = self
            .monitor_for(hwnd)
            .map(|shell| shell.info.bounds)
            .unwrap_or(dock_rect);
        let margin = scale_i32(8, scale);
        let min_x = bounds.left + margin;
        let max_x = (bounds.right - width - margin).max(min_x);
        let x = (anchor_x - width / 2).clamp(min_x, max_x);
        let icon_size = self
            .monitor_for(hwnd)
            .map(|shell| shell.icon_size)
            .unwrap_or(self.config.dock.icon_size);
        let y =
            dock_rect.bottom - (icon_size * scale).round() as i32 - scale_i32(36, scale) - height;
        let Ok(stack_hwnd) = create_window(x, y, width, height, false) else {
            return;
        };
        configure_window_backdrop(stack_hwnd, true, self.high_contrast);
        self.kinds
            .insert(stack_hwnd.0 as isize, WindowKind::FolderStack);
        self.folder_stack = Some(FolderStack {
            hwnd: stack_hwnd,
            dock: hwnd,
            folder: pin.path.clone(),
            title: pin.label.clone(),
            entries,
            hover: None,
            footer_hover: false,
            pointer_x: (anchor_x - x) as f32 / scale,
        });
        unsafe {
            let _ = ShowWindow(stack_hwnd, SW_SHOWNA);
            let _ = InvalidateRect(Some(stack_hwnd), None, false);
        }
    }

    fn close_folder_stack(&mut self) {
        if let Some(stack) = self.folder_stack.take() {
            self.kinds.remove(&(stack.hwnd.0 as isize));
            self.renderer.forget(stack.hwnd);
            unsafe {
                let _ = DestroyWindow(stack.hwnd);
                if self.config.dock.auto_hide {
                    SetTimer(
                        Some(stack.dock),
                        8,
                        self.config.dock.auto_hide_delay_ms,
                        None,
                    );
                }
            }
        }
    }

    fn close_popover(&mut self) {
        if let Some(popover) = self.active_popover.take() {
            self.launcher = None;
            self.kinds.remove(&(popover.hwnd.0 as isize));
            self.renderer.forget(popover.hwnd);
            unsafe {
                let _ = DestroyWindow(popover.hwnd);
                // Repaint the owner so any open-state affordance clears.
                let _ = InvalidateRect(Some(popover.owner), None, false);
            }
        }
    }

    fn launcher_click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let x = x as f32 / scale;
        let y = y as f32 / scale;
        let Some(launcher) = self.launcher.as_ref() else {
            return;
        };
        let visible = launcher
            .apps
            .len()
            .min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
        let layout = crate::render::launcher_geometry(
            crate::render::LAUNCHER_WIDTH,
            crate::render::launcher_height(LAUNCHER_ACTION_LABELS.len(), launcher.apps.len()),
            LAUNCHER_ACTION_LABELS.len(),
            visible,
        );
        match crate::render::launcher_hit_test(&layout, x, y) {
            LauncherHit::Action(i) => {
                match i {
                    0 => open_start_menu(),
                    1 => send_windows_shortcut(b'R'),
                    2 => launch_path(std::path::Path::new("explorer.exe")),
                    3 => power_action(PowerAction::Lock),
                    4 => power_action(PowerAction::Sleep),
                    5 => power_action(PowerAction::Restart),
                    6 => power_action(PowerAction::Shutdown),
                    _ => {}
                }
                self.close_popover();
            }
            LauncherHit::App(visible_i) => {
                let path = self
                    .launcher
                    .as_ref()
                    .and_then(|l| l.apps.get(l.scroll + visible_i))
                    .map(|(_, p)| p.clone());
                self.close_popover();
                if let Some(path) = path {
                    launch_path(&path);
                }
            }
            LauncherHit::None => {}
        }
    }

    fn launcher_mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let x = x as f32 / scale;
        let y = y as f32 / scale;
        let Some(launcher) = self.launcher.as_mut() else {
            return;
        };
        let visible = launcher
            .apps
            .len()
            .min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
        let layout = crate::render::launcher_geometry(
            crate::render::LAUNCHER_WIDTH,
            crate::render::launcher_height(LAUNCHER_ACTION_LABELS.len(), launcher.apps.len()),
            LAUNCHER_ACTION_LABELS.len(),
            visible,
        );
        let new_hover = crate::render::launcher_hit_test(&layout, x, y);
        if Some(new_hover) != launcher.hover {
            launcher.hover = Some(new_hover);
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn launcher_scroll(&mut self, hwnd: HWND, delta: i32) {
        let Some(launcher) = self.launcher.as_mut() else {
            return;
        };
        let visible = crate::render::LAUNCHER_MAX_VISIBLE_ROWS;
        if launcher.apps.len() <= visible {
            return;
        }
        let step = 3;
        let new_scroll = if delta > 0 {
            launcher.scroll.saturating_sub(step)
        } else {
            (launcher.scroll + step).min(launcher.apps.len().saturating_sub(visible))
        };
        if new_scroll != launcher.scroll {
            launcher.scroll = new_scroll;
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    /// Create a popover window anchored under `cursor_x` on the given top
    /// window. The caller passes the specific `WindowKind` to register.
    /// Reuses the folder_stack anchor math.
    fn open_popover(&mut self, owner: HWND, kind: WindowKind, width: i32, height: i32) {
        self.close_popover();
        let scale = self
            .monitor_for(owner)
            .map(|shell| shell.scale)
            .unwrap_or_else(|| window_scale(owner));
        let owner_rect = {
            let mut r = RECT::default();
            // If we can't read the owner rect, bail out (can't anchor).
            if unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindowRect(owner, &mut r).is_err()
            } {
                crate::yume_warn!("popover: could not read owner window rect");
                return;
            }
            r
        };
        let cursor = self
            .cursor_x
            .get(&(owner.0 as isize))
            .copied()
            .unwrap_or((owner_rect.right - owner_rect.left) as f32 / (2.0 * scale));
        let bounds = self
            .monitor_for(owner)
            .map(|shell| shell.info.bounds)
            .unwrap_or(owner_rect);
        let margin = scale_i32(8, scale);
        let anchor_x = owner_rect.left + (cursor * scale).round() as i32;
        let min_x = bounds.left + margin;
        let max_x = (bounds.right - scale_i32(width, scale) - margin).max(min_x);
        let x = (anchor_x - scale_i32(width, scale) / 2).clamp(min_x, max_x);
        let y = owner_rect.bottom;
        let w = scale_i32(width, scale);
        let h = scale_i32(height, scale);
        let Ok(hwnd) = create_window(x, y, w, h, false) else {
            crate::yume_warn!("popover window creation failed");
            return;
        };
        configure_window_backdrop(hwnd, true, self.high_contrast);
        self.kinds.insert(hwnd.0 as isize, kind);
        self.active_popover = Some(Popover { hwnd, owner });
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    /// Open the Phase-1c debug popover — a labelled rectangle that proves the
    /// popover system works end to end (create, render, dismiss). Phases 2–4
    /// replace this with real popovers.
    fn toggle_debug_popover(&mut self, owner: HWND) {
        if self.active_popover.is_some() {
            self.close_popover();
            return;
        }
        self.open_popover(owner, WindowKind::DebugPopover, 220, 120);
    }

    fn open_launcher(&mut self, owner: HWND) {
        let apps = enumerate_start_menu();
        let height = crate::render::launcher_height(LAUNCHER_ACTION_LABELS.len(), apps.len());
        self.open_popover(
            owner,
            WindowKind::Launcher,
            crate::render::LAUNCHER_WIDTH as i32,
            height.round() as i32,
        );
        if let Some(popover) = &self.active_popover {
            self.launcher = Some(LauncherState {
                apps,
                scroll: 0,
                hover: None,
            });
            unsafe {
                let _ = InvalidateRect(Some(popover.hwnd), None, false);
            }
        }
    }

    fn folder_stack_mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        unsafe {
            let _ = KillTimer(Some(hwnd), 10);
        }
        let mut rect = RECT::default();
        if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let height = ((rect.bottom - rect.top) as f32 / window_scale(hwnd)).round() as i32;
        let footer_top = height - 10 - STACK_FOOTER;
        let hover = if x >= STACK_PADDING
            && x < STACK_PADDING + STACK_CELL_WIDTH * STACK_COLUMNS as i32
            && y >= STACK_HEADER
            && y < footer_top
        {
            let column = ((x - STACK_PADDING) / STACK_CELL_WIDTH) as usize;
            let row = ((y - STACK_HEADER) / STACK_CELL_HEIGHT) as usize;
            Some(row * STACK_COLUMNS + column)
        } else {
            None
        };
        let footer_hover = x >= 9 && x <= rect.right - 9 && y >= footer_top && y < rect.bottom - 10;
        let Some(stack) = self
            .folder_stack
            .as_mut()
            .filter(|stack| stack.hwnd == hwnd)
        else {
            return;
        };
        let hover = hover.filter(|index| *index < stack.entries.len());
        let changed = stack.hover != hover || stack.footer_hover != footer_hover;
        stack.hover = hover;
        stack.footer_hover = footer_hover;
        if changed {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
        let mut track = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            ..Default::default()
        };
        unsafe {
            let _ = TrackMouseEvent(&mut track);
        }
    }
}

fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|slot| {
        if let Ok(mut state) = slot.try_borrow_mut()
            && let Some(app) = state.as_mut()
        {
            f(app);
        }
    });
}

fn with_app_value<T>(default: T, f: impl FnOnce(&mut App) -> T) -> T {
    APP.with(|slot| {
        if let Ok(mut state) = slot.try_borrow_mut()
            && let Some(app) = state.as_mut()
        {
            return f(app);
        }
        default
    })
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == TASKBAR_CREATED.load(Ordering::Relaxed) {
        with_app(|app| {
            if app.config.behavior.replace_taskbar {
                app.taskbar.refresh_and_hide();
            }
        });
        return LRESULT(0);
    }
    match msg {
        WM_NCHITTEST => {
            if with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::LaunchOverlay)
            }) {
                return LRESULT(HTTRANSPARENT as isize);
            }
            // Popover windows are fully client-hittable so they receive clicks.
            if with_app_value(false, |app| {
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover) | Some(WindowKind::Launcher)
                )
            }) {
                return LRESULT(HTCLIENT as isize);
            }
            let screen_x = (lparam.0 as i16) as i32;
            let screen_y = ((lparam.0 >> 16) as i16) as i32;
            if let Some(hit) =
                with_app_value(None, |app| app.dock_hit_region(hwnd, screen_x, screen_y))
            {
                return LRESULT(if hit {
                    HTCLIENT as isize
                } else {
                    HTTRANSPARENT as isize
                });
            }
        }
        WM_PAINT => {
            with_app(|app| app.paint(hwnd));
            return LRESULT(0);
        }
        WM_SIZE => {
            with_app(|app| {
                app.renderer.resize(
                    hwnd,
                    lparam.0 as u32 & 0xffff,
                    (lparam.0 as u32 >> 16) & 0xffff,
                )
            });
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                with_app(|app| app.launcher_mouse_move(hwnd, x, y));
                return LRESULT(0);
            }
            with_app(|app| {
                app.mouse_move(
                    hwnd,
                    (lparam.0 as i16) as i32,
                    ((lparam.0 >> 16) as i16) as i32,
                )
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.mouse_leave(hwnd));
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                with_app(|app| app.launcher_scroll(hwnd, delta));
                return LRESULT(0);
            }
        }
        WM_LBUTTONDOWN => {
            let is_popover = with_app_value(false, |app| {
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover)
                )
            });
            if is_popover {
                with_app(|app| app.close_popover());
                return LRESULT(0);
            }
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                with_app(|app| app.launcher_click(hwnd, x, y));
                return LRESULT(0);
            }
            with_app(|app| app.mouse_down(hwnd, (lparam.0 as i16) as i32));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_app(|app| app.mouse_up(hwnd));
            return LRESULT(0);
        }
        WM_KEYDOWN if wparam.0 as i32 == VK_ESCAPE.0 as i32 => {
            with_app(|app| app.close_popover());
            return LRESULT(0);
        }
        WM_CAPTURECHANGED => {
            with_app(|app| app.cancel_drag(hwnd));
            return LRESULT(0);
        }
        WM_MBUTTONUP => {
            with_app(|app| app.activate_dock_item(hwnd, true));
            return LRESULT(0);
        }
        WM_RBUTTONUP => {
            with_app(|app| app.show_menu(hwnd));
            return LRESULT(0);
        }
        WM_DROPFILES => {
            with_app(|app| app.drop_files(HDROP(wparam.0 as *mut _)));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 1 => {
            with_app(|app| app.refresh_system_status());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 2 => {
            with_app(|app| app.animate(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 3 => {
            with_app(|app| app.close_preview());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 4 => {
            with_app(|app| app.animate_launch_bounce(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 5 => {
            with_app(|app| app.process_window_refresh(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 6 => {
            with_app(|app| app.show_pending_preview(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 7 => {
            with_app(|app| app.animate_auto_hide(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 8 => {
            with_app(|app| app.begin_auto_hide(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 9 => {
            with_app(|app| app.animate_reorder(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 10 => {
            with_app(|app| app.close_folder_stack());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 11 => {
            with_app(|app| app.animate_window_open(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 12 => {
            with_app(|app| app.cache_foreground_for_genie());
            return LRESULT(0);
        }
        WM_TIMER => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            with_app(|app| app.schedule_monitor_rebuild());
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            with_app(|app| app.refresh_system_settings());
            return LRESULT(0);
        }
        WM_HOTKEY if wparam.0 as i32 == HOTKEY_RESTORE => {
            with_app(|app| app.shutdown());
            return LRESULT(0);
        }
        WM_CLOSE => {
            with_app(|app| app.shutdown());
            return LRESULT(0);
        }
        WM_DESTROY => return LRESULT(0),
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn register_window_class() -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            anyhow::bail!("RegisterClassExW failed: {:?}", GetLastError())
        }
    }
    Ok(())
}

fn create_window(x: i32, y: i32, width: i32, height: i32, bar: bool) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP;
        let hwnd = CreateWindowExW(
            ex,
            CLASS_NAME,
            w!("YumeDock"),
            WS_POPUP,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let _ = bar;
        DragAcceptFiles(hwnd, true);
        Ok(hwnd)
    }
}

fn launch_path(path: &Path) {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        ShellExecuteW(None, w!("open"), PCWSTR(wide.as_ptr()), None, None, SW_SHOW);
    }
}

fn dock_label(item: &DockItem) -> String {
    match item {
        DockItem::Application { pin: Some(pin), .. } | DockItem::Folder(pin) => pin.label.clone(),
        DockItem::Application {
            identity: Some(identity),
            ..
        } => friendly_app_name(&identity.display_name),
        DockItem::Application { .. } => "App".into(),
        DockItem::RecycleBin => "Recycle Bin".into(),
    }
}

#[derive(Clone)]
struct PendingWindowOpen {
    dock: HWND,
    key: String,
    origin: RECT,
    started: Instant,
}

struct WindowOpenAnimation {
    dock: HWND,
    overlay: HWND,
    source: HWND,
    window_rect: RECT,
    icon_rect: RECT,
    overlay_bounds: RECT,
    started: Instant,
    restore_at_end: bool,
    opening: bool,
}

fn dock_identity_key(item: &DockItem) -> Option<String> {
    match item {
        DockItem::Application { pin: Some(pin), .. } => Some(
            pin.identity_path
                .as_ref()
                .unwrap_or(&pin.path)
                .to_string_lossy()
                .to_ascii_lowercase(),
        ),
        DockItem::Application {
            identity: Some(identity),
            ..
        } => Some(identity.icon_key.to_ascii_lowercase()),
        _ => None,
    }
}

fn dock_pin(item: &DockItem) -> Option<&PinConfig> {
    match item {
        DockItem::Application { pin: Some(pin), .. } | DockItem::Folder(pin) => Some(pin),
        _ => None,
    }
}

fn swap_compatible_pins(pins: &mut [PinConfig], source: &PinConfig, target: &PinConfig) -> bool {
    if source == target
        || source.kind == PinKind::RecycleBin
        || target.kind == PinKind::RecycleBin
        || source.kind != target.kind
    {
        return false;
    }
    let Some(from) = pins.iter().position(|pin| pin == source) else {
        return false;
    };
    let Some(to) = pins.iter().position(|pin| pin == target) else {
        return false;
    };
    pins.swap(from, to);
    true
}

fn dock_icon_paths(item: &DockItem) -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>) {
    match item {
        DockItem::Application {
            pin: Some(pin),
            identity,
            ..
        } => (
            Some(preferred_pin_icon(pin)),
            identity
                .as_ref()
                .map(|identity| identity.executable.clone()),
        ),
        DockItem::Folder(pin) => (Some(pin.path.clone()), None),
        DockItem::Application {
            identity: Some(identity),
            ..
        } => {
            let app_id_icon = identity
                .app_user_model_id
                .as_ref()
                .map(|id| std::path::PathBuf::from(format!(r"shell:AppsFolder\{id}")));
            let executable = identity.executable.clone();
            let packaged = executable
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("\\windowsapps\\");
            if packaged {
                (
                    app_id_icon.or_else(|| Some(executable.clone())),
                    Some(executable),
                )
            } else {
                (Some(executable), app_id_icon)
            }
        }
        DockItem::RecycleBin | DockItem::Application { .. } => (None, None),
    }
}

fn preferred_pin_icon(pin: &crate::config::PinConfig) -> std::path::PathBuf {
    if pin.label.eq_ignore_ascii_case("File Explorer")
        && let Some(windows) = std::env::var_os("WINDIR")
    {
        return std::path::PathBuf::from(windows).join("explorer.exe");
    }
    let Some(path) = pin.identity_path.as_ref() else {
        return pin.path.clone();
    };
    if path
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("Update.exe"))
    {
        return pin.path.clone();
    }
    let raw = path.to_string_lossy();
    let expanded = if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("%windir%"))
    {
        std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .map(|root| {
                root.join(
                    raw.get(8..)
                        .unwrap_or_default()
                        .trim_start_matches(['\\', '/']),
                )
            })
            .unwrap_or_else(|| path.clone())
    } else {
        path.clone()
    };
    if expanded.exists() {
        expanded
    } else {
        pin.path.clone()
    }
}

fn send_media_key(key: u8) {
    unsafe {
        keybd_event(key, 0, Default::default(), 0);
        keybd_event(key, 0, KEYEVENTF_KEYUP, 0);
    }
}

fn open_start_menu() {
    unsafe {
        keybd_event(VK_LWIN.0 as u8, 0, Default::default(), 0);
        keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

/// Enumerate Start Menu apps (system + user), alphabetical, as (label, .lnk).
/// Caps at 64 entries.
fn enumerate_start_menu() -> Vec<(String, std::path::PathBuf)> {
    let mut entries = Vec::new();
    let dirs: [Option<std::path::PathBuf>; 2] = [
        std::env::var_os("ProgramData")
            .map(|p| std::path::PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
        std::env::var_os("APPDATA")
            .map(|p| std::path::PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
    ];
    for dir in dirs.into_iter().flatten() {
        collect_lnk(&dir, &mut entries);
    }
    entries.sort_by_key(|a| a.0.to_lowercase());
    entries.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    entries.truncate(64);
    entries
}

fn collect_lnk(dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lnk(&path, out);
            continue;
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            && let Some(name) = path.file_stem()
        {
            out.push((name.to_string_lossy().into_owned(), path));
        }
    }
}

#[derive(Clone, Copy)]
enum PowerAction {
    Sleep,
    Restart,
    Shutdown,
    Lock,
}

/// Best-effort power action via the shell. Uses shutdown.exe / rundll32 so we
/// avoid linking Win32_System_Shutdown and the SE_SHUTDOWN_NAME privilege
/// dance — these verbs prompt or act as the current user.
fn power_action(action: PowerAction) {
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    unsafe {
        match action {
            PowerAction::Sleep => {
                let _ = ShellExecuteW(
                    None,
                    w!("open"),
                    w!("rundll32.exe"),
                    w!("powrprof.dll,SetSuspendState 0,1,0"),
                    None,
                    SW_SHOW,
                );
            }
            PowerAction::Restart => {
                let _ = ShellExecuteW(
                    None,
                    w!("open"),
                    w!("shutdown.exe"),
                    w!("/r /t 0"),
                    None,
                    SW_SHOW,
                );
            }
            PowerAction::Shutdown => {
                let _ = ShellExecuteW(
                    None,
                    w!("open"),
                    w!("shutdown.exe"),
                    w!("/s /t 0"),
                    None,
                    SW_SHOW,
                );
            }
            PowerAction::Lock => {
                let _ = ShellExecuteW(
                    None,
                    w!("open"),
                    w!("rundll32.exe"),
                    w!("user32.dll,LockWorkStation"),
                    None,
                    SW_SHOW,
                );
            }
        }
    }
}

fn send_windows_shortcut(key: u8) {
    unsafe {
        keybd_event(VK_LWIN.0 as u8, 0, Default::default(), 0);
        keybd_event(key, 0, Default::default(), 0);
        keybd_event(key, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_LWIN.0 as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

fn open_item(item: &DockItem, new_instance: bool) {
    match item {
        DockItem::Application { windows, .. } if !new_instance && !windows.is_empty() => unsafe {
            let _ = ShowWindow(windows[0], SW_RESTORE);
            let _ = SetForegroundWindow(windows[0]);
        },
        DockItem::Application { pin: Some(pin), .. } | DockItem::Folder(pin) => {
            launch_path(&pin.path)
        }
        DockItem::Application {
            identity: Some(identity),
            ..
        } => launch_path(&identity.executable),
        DockItem::RecycleBin => unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                w!("shell:RecycleBinFolder"),
                None,
                None,
                SW_SHOW,
            );
        },
        _ => {}
    }
}

fn active_app_name(windows: &[WindowEntry]) -> String {
    let active = unsafe { GetForegroundWindow() };
    windows
        .iter()
        .find(|w| w.hwnd == active)
        .map(|w| friendly_app_name(&w.identity.display_name))
        .unwrap_or_else(|| "Desktop".into())
}

fn friendly_app_name(process_name: &str) -> String {
    match process_name.to_ascii_lowercase().as_str() {
        "msedge" => "Microsoft Edge".into(),
        "explorer" => "File Explorer".into(),
        "steamwebhelper" => "Steam".into(),
        "applicationframehost" => "Windows App".into(),
        "valorant-win64-shipping" => "VALORANT".into(),
        _ => process_name.to_string(),
    }
}

fn current_clock(use_24: bool) -> String {
    let now = chrono_like_local_time();
    if use_24 {
        format!("{:02}:{:02}", now.3, now.4)
    } else {
        let hour = if now.3.is_multiple_of(12) {
            12
        } else {
            now.3 % 12
        };
        format!(
            "{hour}:{:02} {}",
            now.4,
            if now.3 < 12 { "AM" } else { "PM" }
        )
    }
}

/// Compact "Wed 14 Jul" style date for the menu-bar clock segment. Uses the
/// raw `GetLocalTime` directly (rather than `chrono_like_local_time`) because
/// it also needs the day-of-week, which that helper discards.
fn current_date() -> String {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = WEEKDAYS
        .get(time.wDayOfWeek as usize)
        .copied()
        .unwrap_or("???");
    let month = MONTHS
        .get((time.wMonth as usize).saturating_sub(1))
        .copied()
        .unwrap_or("???");
    format!("{weekday} {day} {month}", day = time.wDay)
}

fn rounded_rect_contains(
    x: f32,
    y: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    radius: f32,
) -> bool {
    if x < left || x > right || y < top || y > bottom {
        return false;
    }
    let nearest_x = x.clamp(left + radius, right - radius);
    let nearest_y = y.clamp(top + radius, bottom - radius);
    let dx = x - nearest_x;
    let dy = y - nearest_y;
    dx * dx + dy * dy <= radius * radius
}

#[derive(Clone, Copy)]
struct LaunchBounceFrame {
    offset: f32,
    scale_x: f32,
    scale_y: f32,
}

fn rect_width(rect: RECT) -> i32 {
    rect.right - rect.left
}

fn rect_height(rect: RECT) -> i32 {
    rect.bottom - rect.top
}

fn launch_origin_rect(icon: RECT, source: SIZE) -> RECT {
    let icon_width = rect_width(icon).max(1) as f32;
    let icon_height = rect_height(icon).max(1) as f32;
    let aspect = source.cx.max(1) as f32 / source.cy.max(1) as f32;
    let (width, height) = if aspect >= 1.0 {
        (icon_width, icon_width / aspect)
    } else {
        (icon_height * aspect, icon_height)
    };
    let center_x = (icon.left + icon.right) as f32 / 2.0;
    let center_y = (icon.top + icon.bottom) as f32 / 2.0;
    RECT {
        left: (center_x - width / 2.0).round() as i32,
        top: (center_y - height / 2.0).round() as i32,
        right: (center_x + width / 2.0).round() as i32,
        bottom: (center_y + height / 2.0).round() as i32,
    }
}

fn settle_window_transition(source: HWND, opening: bool, was_minimized: bool) {
    unsafe {
        if opening {
            if was_minimized {
                let _ = ShowWindow(source, SW_RESTORE);
            }
            let _ = SetForegroundWindow(source);
        } else {
            let _ = ShowWindow(source, SW_MINIMIZE);
        }
    }
}

fn genie_frame_rects(
    window: RECT,
    icon: RECT,
    overlay: RECT,
    progress: f32,
    opening: bool,
) -> Vec<RECT> {
    (0..GENIE_SLICES)
        .map(|index| {
            let screen = genie_slice_rect(window, icon, progress, index, opening);
            RECT {
                left: screen.left - overlay.left,
                top: screen.top - overlay.top,
                right: screen.right - overlay.left,
                bottom: screen.bottom - overlay.top,
            }
        })
        .collect()
}

fn genie_slice_rect(window: RECT, icon: RECT, progress: f32, index: usize, opening: bool) -> RECT {
    let y0 = index as f32 / GENIE_SLICES as f32;
    let y1 = (index + 1) as f32 / GENIE_SLICES as f32;
    let ym = (y0 + y1) / 2.0;
    let local = |y: f32| {
        let stagger = 0.22;
        let raw = if opening {
            progress * (1.0 + stagger) - y * stagger
        } else {
            progress * (1.0 + stagger) - (1.0 - y) * stagger
        }
        .clamp(0.0, 1.0);
        1.0 - (1.0 - raw).powi(3)
    };
    let interpolate = |from: f32, to: f32, amount: f32| from + (to - from) * amount;
    let icon_y = |y: f32| icon.top as f32 + rect_height(icon) as f32 * y;
    let window_y = |y: f32| window.top as f32 + rect_height(window) as f32 * y;
    let boundary_y = |y: f32| {
        if opening {
            interpolate(icon_y(y), window_y(y), local(y))
        } else {
            interpolate(window_y(y), icon_y(y), local(y))
        }
    };
    let horizontal_progress = local(ym);
    let icon_center = (icon.left + icon.right) as f32 / 2.0;
    let window_center = (window.left + window.right) as f32 / 2.0;
    let (center, width) = if opening {
        (
            interpolate(icon_center, window_center, horizontal_progress),
            interpolate(
                rect_width(icon) as f32,
                rect_width(window) as f32,
                horizontal_progress,
            ),
        )
    } else {
        (
            interpolate(window_center, icon_center, horizontal_progress),
            interpolate(
                rect_width(window) as f32,
                rect_width(icon) as f32,
                horizontal_progress,
            ),
        )
    };
    let top = boundary_y(y0).round() as i32;
    let bottom = boundary_y(y1).round() as i32;
    RECT {
        left: (center - width / 2.0).round() as i32,
        top,
        right: (center + width / 2.0).round() as i32,
        bottom,
    }
}

fn launch_bounce_frame(elapsed: Duration, icon_size: f32) -> LaunchBounceFrame {
    let cycle = (elapsed.as_secs_f32() % 0.62) / 0.62;
    if cycle < 0.82 {
        let airborne = cycle / 0.82;
        let offset = (airborne * std::f32::consts::PI).sin().max(0.0).powf(0.84) * icon_size * 0.42;
        let stretch = if airborne < 0.18 {
            (airborne / 0.18 * std::f32::consts::PI).sin().max(0.0)
        } else {
            0.0
        };
        LaunchBounceFrame {
            offset,
            scale_x: 1.0 - 0.018 * stretch,
            scale_y: 1.0 + 0.035 * stretch,
        }
    } else {
        let landing = (cycle - 0.82) / 0.18;
        let squash = (landing * std::f32::consts::PI).sin().max(0.0);
        LaunchBounceFrame {
            offset: 0.0,
            scale_x: 1.0 + 0.055 * squash,
            scale_y: 1.0 - 0.075 * squash,
        }
    }
}

fn approach_value(value: f32, target: f32, elapsed: Duration, duration: Duration) -> f32 {
    if duration.is_zero() {
        return target;
    }
    let blend = 1.0 - (-4.605 * elapsed.as_secs_f32() / duration.as_secs_f32()).exp();
    value + (target - value) * blend.clamp(0.0, 1.0)
}

fn scale_i32(value: i32, scale: f32) -> i32 {
    (value as f32 * scale.max(1.0)).round() as i32
}

fn window_scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd).max(96) as f32 / 96.0 }
}

/// Client size of `hwnd` in device-independent pixels (DIPs), accounting for
/// the per-window DPI scale.
fn client_size_dips(hwnd: HWND, scale: f32) -> (f32, f32) {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut rect);
    }
    let width = ((rect.right - rect.left) as f32 / scale).max(1.0);
    let height = ((rect.bottom - rect.top) as f32 / scale).max(1.0);
    (width, height)
}

fn high_contrast_enabled() -> bool {
    let mut high_contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            high_contrast.cbSize,
            Some((&mut high_contrast as *mut HIGHCONTRASTW).cast()),
            Default::default(),
        )
        .is_ok()
            && high_contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
    }
}

fn configure_window_backdrop(hwnd: HWND, rounded: bool, high_contrast: bool) {
    if high_contrast {
        return;
    }
    let dark_mode = BOOL(1);
    let corner = if rounded {
        DWMWCP_ROUND
    } else {
        DWMWCP_DONOTROUND
    };
    let border_color = DWMWA_COLOR_NONE;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&dark_mode as *const BOOL).cast(),
            std::mem::size_of_val(&dark_mode) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&corner as *const windows::Win32::Graphics::Dwm::DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            (&border_color as *const u32).cast(),
            std::mem::size_of_val(&border_color) as u32,
        );
    }
}

fn preview_content_rect(source: SIZE, width: i32, height: i32, scale: f32) -> RECT {
    let padding = scale_i32(8, scale);
    let header = scale_i32(36, scale);
    let pointer_and_bottom = scale_i32(18, scale);
    let available_width = (width - padding * 2).max(1);
    let available_height = (height - header - pointer_and_bottom).max(1);
    let source_width = source.cx.max(1) as f32;
    let source_height = source.cy.max(1) as f32;
    let scale =
        (available_width as f32 / source_width).min(available_height as f32 / source_height);
    let fitted_width = (source_width * scale).round().max(1.0) as i32;
    let fitted_height = (source_height * scale).round().max(1.0) as i32;
    let left = (width - fitted_width) / 2;
    let top = header + (available_height - fitted_height) / 2;
    RECT {
        left,
        top,
        right: left + fitted_width,
        bottom: top + fitted_height,
    }
}

fn ease_out_quart(progress: f32) -> f32 {
    1.0 - (1.0 - progress.clamp(0.0, 1.0)).powi(4)
}

fn chrono_like_local_time() -> (i32, u32, u32, u32, u32) {
    let time = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        time.wYear as i32,
        time.wMonth as u32,
        time.wDay as u32,
        time.wHour as u32,
        time.wMinute as u32,
    )
}

use std::os::windows::ffi::OsStrExt;

#[cfg(test)]
mod label_tests {
    use super::{
        DockItem, GENIE_SLICES, PinConfig, PinKind, RECT, SIZE, approach_value, dock_identity_key,
        ease_out_quart, friendly_app_name, genie_frame_rects, genie_slice_rect,
        launch_bounce_frame, launch_origin_rect, preview_content_rect, rect_width,
        rounded_rect_contains, scale_i32, swap_compatible_pins,
    };
    use crate::model::AppIdentity;
    use std::{path::PathBuf, time::Duration};

    fn pin(label: &str, kind: PinKind) -> PinConfig {
        PinConfig {
            label: label.into(),
            path: PathBuf::from(label),
            identity_path: None,
            kind,
        }
    }

    #[test]
    fn replaces_raw_windows_process_names() {
        assert_eq!(friendly_app_name("msedge"), "Microsoft Edge");
        assert_eq!(friendly_app_name("explorer"), "File Explorer");
        assert_eq!(friendly_app_name("YumePlayer"), "YumePlayer");
    }

    #[test]
    fn rounded_dock_hit_region_excludes_transparent_corners() {
        assert!(rounded_rect_contains(
            20.0, 10.0, 0.0, 0.0, 100.0, 50.0, 18.0
        ));
        assert!(!rounded_rect_contains(
            1.0, 1.0, 0.0, 0.0, 100.0, 50.0, 18.0
        ));
    }

    #[test]
    fn launch_bounce_has_a_clear_mac_style_peak() {
        assert_eq!(launch_bounce_frame(Duration::ZERO, 48.0).offset, 0.0);
        assert!(launch_bounce_frame(Duration::from_millis(250), 48.0).offset > 19.0);
        let landing = launch_bounce_frame(Duration::from_millis(565), 48.0);
        assert!(landing.scale_y < 0.95);
        assert!(landing.scale_x > 1.03);
    }

    #[test]
    fn genie_open_starts_at_icon_and_unfurls_to_window() {
        let icon_bounds = RECT {
            left: 400,
            top: 900,
            right: 464,
            bottom: 964,
        };
        let icon = launch_origin_rect(icon_bounds, SIZE { cx: 1600, cy: 900 });
        let window = RECT {
            left: 120,
            top: 80,
            right: 1720,
            bottom: 980,
        };
        let start_top = genie_slice_rect(window, icon, 0.0, 0, true);
        let start_bottom = genie_slice_rect(window, icon, 0.0, GENIE_SLICES - 1, true);
        assert_eq!(start_top.top, icon.top);
        assert_eq!(start_bottom.bottom, icon.bottom);
        let end_top = genie_slice_rect(window, icon, 1.0, 0, true);
        let end_bottom = genie_slice_rect(window, icon, 1.0, GENIE_SLICES - 1, true);
        assert_eq!(end_top.top, window.top);
        assert_eq!(end_bottom.bottom, window.bottom);
        let middle_top = genie_slice_rect(window, icon, 0.35, 0, true);
        let middle_bottom = genie_slice_rect(window, icon, 0.35, GENIE_SLICES - 1, true);
        assert!(rect_width(middle_top) > rect_width(middle_bottom));
    }

    #[test]
    fn genie_minimize_funnels_window_back_to_icon() {
        let window = RECT {
            left: 120,
            top: 80,
            right: 1720,
            bottom: 980,
        };
        let icon = RECT {
            left: 400,
            top: 925,
            right: 464,
            bottom: 961,
        };
        let start_top = genie_slice_rect(window, icon, 0.0, 0, false);
        let start_bottom = genie_slice_rect(window, icon, 0.0, GENIE_SLICES - 1, false);
        assert_eq!(start_top.top, window.top);
        assert_eq!(start_bottom.bottom, window.bottom);
        let end_top = genie_slice_rect(window, icon, 1.0, 0, false);
        let end_bottom = genie_slice_rect(window, icon, 1.0, GENIE_SLICES - 1, false);
        assert_eq!(end_top.top, icon.top);
        assert_eq!(end_bottom.bottom, icon.bottom);
        let middle_top = genie_slice_rect(window, icon, 0.35, 0, false);
        let middle_bottom = genie_slice_rect(window, icon, 0.35, GENIE_SLICES - 1, false);
        assert!(rect_width(middle_bottom) < rect_width(middle_top));
    }

    #[test]
    fn genie_frame_is_contiguous_and_local_to_one_overlay() {
        let window = RECT {
            left: 100,
            top: 50,
            right: 1700,
            bottom: 950,
        };
        let icon = RECT {
            left: 760,
            top: 930,
            right: 824,
            bottom: 966,
        };
        let overlay = RECT {
            left: 98,
            top: 48,
            right: 1702,
            bottom: 968,
        };
        let slices = genie_frame_rects(window, icon, overlay, 0.42, false);
        assert_eq!(slices.len(), GENIE_SLICES);
        assert!(slices.iter().all(|slice| {
            slice.left >= 0
                && slice.top >= 0
                && slice.right <= rect_width(overlay)
                && slice.bottom <= super::rect_height(overlay)
        }));
        for pair in slices.windows(2) {
            assert_eq!(pair[0].bottom, pair[1].top);
        }
    }

    #[test]
    fn launch_identity_stays_stable_when_a_pinned_app_gets_a_window() {
        let mut app_pin = pin("Example", PinKind::Application);
        app_pin.identity_path = Some(PathBuf::from(r"C:\Apps\Example.exe"));
        let before = DockItem::Application {
            pin: Some(app_pin.clone()),
            identity: None,
            windows: Vec::new(),
        };
        let after = DockItem::Application {
            pin: Some(app_pin),
            identity: Some(AppIdentity {
                app_user_model_id: None,
                executable: PathBuf::from(r"C:\Apps\Example.exe"),
                display_name: "Different window title".into(),
                icon_key: "example.exe".into(),
            }),
            windows: Vec::new(),
        };
        assert_eq!(dock_identity_key(&before), dock_identity_key(&after));
    }

    #[test]
    fn time_based_motion_reaches_same_point_at_different_frame_rates() {
        let duration = Duration::from_millis(160);
        let one_step = approach_value(0.0, 1.0, duration, duration);
        let mut many_steps = 0.0;
        for _ in 0..10 {
            many_steps = approach_value(many_steps, 1.0, Duration::from_millis(16), duration);
        }
        assert!((one_step - many_steps).abs() < 0.001);
    }

    #[test]
    fn preview_preserves_wide_window_aspect_ratio() {
        let rect = preview_content_rect(SIZE { cx: 1920, cy: 1080 }, 336, 232, 1.0);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        assert!(((width as f32 / height as f32) - (16.0 / 9.0)).abs() < 0.02);
        assert!(rect.top >= 36);
        assert!(rect.bottom <= 214);
    }

    #[test]
    fn dpi_scaling_keeps_logical_geometry_proportional() {
        assert_eq!(scale_i32(32, 1.0), 32);
        assert_eq!(scale_i32(32, 1.25), 40);
        assert_eq!(scale_i32(48, 2.0), 96);
        let preview = preview_content_rect(SIZE { cx: 1920, cy: 1080 }, 672, 464, 2.0);
        assert!(preview.top >= 72);
        assert!(preview.bottom <= 428);
    }

    #[test]
    fn drag_reorders_apps_but_never_recycle_bin() {
        let first = pin("First", PinKind::Application);
        let second = pin("Second", PinKind::Application);
        let recycle = pin("Recycle Bin", PinKind::RecycleBin);
        let mut pins = vec![first.clone(), second.clone(), recycle.clone()];
        assert!(swap_compatible_pins(&mut pins, &first, &second));
        assert_eq!(pins[0], second);
        assert!(!swap_compatible_pins(&mut pins, &first, &recycle));
        assert_eq!(pins.last(), Some(&recycle));
    }

    #[test]
    fn reorder_settling_uses_fast_ease_out() {
        assert_eq!(ease_out_quart(0.0), 0.0);
        assert!(ease_out_quart(0.5) > 0.9);
        assert_eq!(ease_out_quart(1.0), 1.0);
    }
}
