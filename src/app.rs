#![allow(unsafe_op_in_unsafe_fn)]

use crate::{
    config::{ConfigV1, PinConfig, PinKind},
    model::{DockItem, MonitorInfo, WindowEntry, enumerate_monitors, group_for_monitor},
    render::{DockVisual, Renderer},
    shell::{AppBar, TaskbarState},
    status, tracker,
};
use anyhow::Result;
use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicU32, Ordering},
};
use windows::{
    Win32::{
        Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Dwm::{
                DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_VISIBLE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
                DwmRegisterThumbnail, DwmSetWindowAttribute, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties,
            },
            Gdi::{
                BeginPaint, CreateRoundRectRgn, EndPaint, InvalidateRect, PAINTSTRUCT, SetWindowRgn,
            },
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx},
            LibraryLoader::GetModuleHandleW,
            Threading::GetCurrentThreadId,
        },
        UI::{
            Controls::WM_MOUSELEAVE,
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            Input::KeyboardAndMouse::{
                KEYEVENTF_KEYUP, MOD_ALT, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, TME_LEAVE,
                TRACKMOUSEEVENT, TrackMouseEvent, UnregisterHotKey, VK_F12, VK_VOLUME_DOWN,
                VK_VOLUME_MUTE, VK_VOLUME_UP, keybd_event,
            },
            Shell::{
                DragAcceptFiles, DragFinish, DragQueryFileW, HDROP, SHERB_NOCONFIRMATION,
                SHERB_NOPROGRESSUI, SHEmptyRecycleBinW, ShellExecuteW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CS_HREDRAW, CS_VREDRAW, CreatePopupMenu, CreateWindowExW,
                DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, GetClientRect,
                GetCursorPos, GetForegroundWindow, GetMessageW, IDC_ARROW, KillTimer, LWA_ALPHA,
                LoadCursorW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW,
                PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SW_HIDE, SW_RESTORE,
                SW_SHOW, SW_SHOWNA, SetForegroundWindow, SetLayeredWindowAttributes, SetTimer,
                ShowWindow, TPM_RETURNCMD, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE,
                WM_DESTROY, WM_DISPLAYCHANGE, WM_DROPFILES, WM_HOTKEY, WM_LBUTTONUP, WM_MBUTTONUP,
                WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SIZE, WM_TIMER,
                WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_POPUP,
            },
        },
    },
    core::{PCWSTR, w},
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
const CMD_OPEN_ITEM: usize = 200;
const CMD_NEW_INSTANCE: usize = 201;
const CMD_TOGGLE_PIN: usize = 202;
const CMD_MINIMIZE_ITEM: usize = 203;
const CMD_CLOSE_ITEM: usize = 204;
const CMD_EMPTY_RECYCLE: usize = 205;
pub const WM_REFRESH: u32 = WM_APP + 1;

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
}

struct Preview {
    hwnd: HWND,
    thumbnail: isize,
    source: HWND,
    dock: HWND,
}

struct MonitorShell {
    info: MonitorInfo,
    top: HWND,
    dock: HWND,
    reserve: HWND,
    top_appbar: Option<AppBar>,
    bottom_appbar: Option<AppBar>,
    icon_size: f32,
}

pub struct App {
    config: ConfigV1,
    renderer: Renderer,
    shells: Vec<MonitorShell>,
    kinds: HashMap<isize, WindowKind>,
    hover: HashMap<isize, Option<usize>>,
    windows: Vec<WindowEntry>,
    taskbar: TaskbarState,
    safe_mode: bool,
    shutting_down: bool,
    _hooks: Option<tracker::HookSet>,
    preview: Option<Preview>,
    menu_item: Option<DockItem>,
    animation: HashMap<isize, f32>,
    cycle_index: HashMap<String, usize>,
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
        let renderer = Renderer::new()?;
        let mut app = Self {
            config,
            renderer,
            shells: Vec::new(),
            kinds: HashMap::new(),
            hover: HashMap::new(),
            windows: tracker::enumerate_windows(),
            taskbar: TaskbarState::capture(),
            safe_mode,
            shutting_down: false,
            _hooks: None,
            preview: None,
            menu_item: None,
            animation: HashMap::new(),
            cycle_index: HashMap::new(),
        };
        app.create_monitor_shells()?;
        let _ = crate::config::sync_startup(app.config.behavior.start_with_windows);
        if app.config.behavior.replace_taskbar {
            app.taskbar.hide();
        }
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
        ));
        APP.with(|slot| *slot.borrow_mut() = Some(app));

        let mut msg = MSG::default();
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).into() {
                if msg.hwnd.is_invalid() && msg.message == WM_REFRESH {
                    with_app(|app| app.refresh_windows());
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
            let top_height = self.config.top_bar.height;
            let dock_height = self.config.dock.height;
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
            let icon_size = self
                .config
                .dock
                .icon_size
                .min((((max_dock_width - 24.0) / item_count as f32) - 8.0).max(30.0));
            let top = create_window(info.bounds.left, info.bounds.top, width, top_height, true)?;
            let reserve = create_window(
                info.bounds.left,
                bottom_edge - dock_height,
                width,
                dock_height,
                true,
            )?;
            let dock_width = ((item_count as f32 * (icon_size + 8.0) + 24.0).ceil() as i32)
                .clamp(180, (width as f32 * 0.84) as i32);
            let dock_visual_height = (icon_size * self.config.dock.magnification + 28.0)
                .ceil()
                .max(dock_height as f32) as i32;
            let dock = create_window(
                info.bounds.left + (width - dock_width) / 2,
                bottom_edge - dock_visual_height - 8,
                dock_width,
                dock_visual_height,
                false,
            )?;
            self.kinds.insert(top.0 as isize, WindowKind::Top);
            self.kinds.insert(dock.0 as isize, WindowKind::Dock);
            self.kinds.insert(reserve.0 as isize, WindowKind::Reserve);
            self.hover.insert(dock.0 as isize, None);
            self.animation.insert(dock.0 as isize, 0.0);

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
                (
                    Some(AppBar::register(top, true, top_rect)?),
                    Some(AppBar::register(reserve, false, bottom_rect)?),
                )
            } else {
                (None, None)
            };
            unsafe {
                let _ = ShowWindow(top, SW_SHOWNA);
                let _ = ShowWindow(dock, SW_SHOWNA);
                let _ = ShowWindow(reserve, SW_HIDE);
                SetTimer(Some(top), 1, 1000, None);
                let backdrop = DWMSBT_TRANSIENTWINDOW;
                let _ = DwmSetWindowAttribute(
                    dock,
                    DWMWA_SYSTEMBACKDROP_TYPE,
                    &backdrop as *const _ as _,
                    std::mem::size_of_val(&backdrop) as u32,
                );
            }
            self.shells.push(MonitorShell {
                info,
                top,
                dock,
                reserve,
                top_appbar,
                bottom_appbar,
                icon_size,
            });
        }
        Ok(())
    }

    fn destroy_monitor_shells(&mut self) {
        self.close_preview();
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
        self.animation.clear();
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

    fn refresh_windows(&mut self) {
        self.windows = tracker::enumerate_windows();
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.dock), None, false);
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
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

    fn dock_items(&self, hwnd: HWND) -> Vec<DockItem> {
        let monitor = self
            .monitor_for(hwnd)
            .map(|s| s.info.handle.0 as isize)
            .unwrap_or_default();
        group_for_monitor(&self.config.pins, &self.windows, monitor)
    }

    fn paint(&mut self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        unsafe {
            BeginPaint(hwnd, &mut ps);
        }
        let result = match self.kinds.get(&(hwnd.0 as isize)).copied() {
            Some(WindowKind::Top) => {
                let active = active_app_name(&self.windows);
                let status = status::read_status();
                let battery = status
                    .battery_percent
                    .map(|p| format!("{}{}%", if status.charging { "⚡" } else { "" }, p))
                    .unwrap_or_default();
                let network = if status.network_online { "⌁" } else { "×" };
                let volume = if status.muted {
                    "🔇".into()
                } else {
                    status
                        .volume_percent
                        .map(|v| format!("◕ {v}%"))
                        .unwrap_or_else(|| "◕".into())
                };
                let clock = current_clock(self.config.top_bar.use_24_hour_clock);
                self.renderer.paint_top_bar(
                    hwnd,
                    &active,
                    &format!("{network}   {volume}   {battery}   {clock}"),
                )
            }
            Some(WindowKind::Dock) => {
                let items = self.dock_items(hwnd);
                let visuals: Vec<_> = items.iter().map(|item| DockVisual {
                    label: dock_label(item),
                    running: matches!(item, DockItem::Application { windows, .. } if !windows.is_empty()),
                    icon_path: dock_icon_path(item),
                }).collect();
                let hover = self.hover.get(&(hwnd.0 as isize)).copied().flatten();
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
                self.renderer
                    .paint_dock(hwnd, &visuals, hover, icon_size, magnification)
            }
            Some(WindowKind::Preview) => self.renderer.paint_dock(hwnd, &[], None, 0.0, 1.0),
            _ => Ok(()),
        };
        unsafe {
            let _ = EndPaint(hwnd, &ps);
        }
        if let Err(error) = result {
            eprintln!("paint failed: {error:#}");
        }
    }

    fn mouse_move(&mut self, hwnd: HWND, x: i32) {
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Preview) {
            if let Some(preview) = self.preview.as_ref() {
                unsafe {
                    let _ = KillTimer(Some(preview.dock), 3);
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
        let items = self.dock_items(hwnd);
        let mut rect = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rect);
        }
        let icon_size = self
            .monitor_for(hwnd)
            .map(|shell| shell.icon_size)
            .unwrap_or(self.config.dock.icon_size);
        let step = icon_size + 8.0;
        let total = items.len() as f32 * step + 24.0;
        let start = (((rect.right - rect.left) as f32 - total) / 2.0).max(8.0) + 12.0;
        let index = ((x as f32 - start) / step).floor() as isize;
        let next = (index >= 0 && index < items.len() as isize).then_some(index as usize);
        let previous = self.hover.get(&(hwnd.0 as isize)).copied().flatten();
        let changed = previous != next;
        self.hover.insert(hwnd.0 as isize, next);
        if changed {
            self.show_preview(hwnd, next.and_then(|i| items.get(i)).and_then(first_window));
            if previous.is_none() {
                self.animation.insert(
                    hwnd.0 as isize,
                    if self.config.behavior.reduce_motion {
                        1.0
                    } else {
                        0.0
                    },
                );
            }
            unsafe {
                SetTimer(Some(hwnd), 2, 16, None);
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

    fn mouse_leave(&mut self, hwnd: HWND) {
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Preview) {
            self.close_preview();
            return;
        }
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
            unsafe {
                SetTimer(Some(hwnd), 2, 16, None);
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
        *value += (target - *value) * 0.24;
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
        let width = 320;
        let height = 190;
        let x = (dock_rect.left + dock_rect.right - width) / 2;
        let y = dock_rect.top - height - 10;
        let Ok(hwnd) = create_window(x, y, width, height, false) else {
            return;
        };
        unsafe {
            let Ok(thumbnail) = DwmRegisterThumbnail(hwnd, source) else {
                let _ = DestroyWindow(hwnd);
                return;
            };
            let properties = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_VISIBLE | DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY,
                rcDestination: RECT {
                    left: 8,
                    top: 8,
                    right: width - 8,
                    bottom: height - 8,
                },
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
            });
        }
    }

    fn close_preview(&mut self) {
        if let Some(preview) = self.preview.take() {
            self.kinds.remove(&(preview.hwnd.0 as isize));
            self.renderer.forget(preview.hwnd);
            unsafe {
                let _ = DwmUnregisterThumbnail(preview.thumbnail);
                let _ = DestroyWindow(preview.hwnd);
            }
        }
    }

    fn activate_dock_item(&mut self, hwnd: HWND, new_instance: bool) {
        let Some(index) = self.hover.get(&(hwnd.0 as isize)).copied().flatten() else {
            return;
        };
        let items = self.dock_items(hwnd);
        let Some(item) = items.get(index) else { return };
        match item {
            DockItem::Application { windows, .. } if !new_instance && !windows.is_empty() => {
                let key = dock_label(item).to_lowercase();
                let index = self.cycle_index.entry(key).or_default();
                let target = windows[*index % windows.len()];
                *index = (*index + 1) % windows.len();
                unsafe {
                    let _ = ShowWindow(target, SW_RESTORE);
                    let _ = SetForegroundWindow(target);
                }
            }
            DockItem::Application { pin: Some(pin), .. } => launch_path(&pin.path),
            DockItem::Folder(pin) => self.show_folder_stack(hwnd, pin),
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

    fn left_click(&mut self, hwnd: HWND) {
        match self.kinds.get(&(hwnd.0 as isize)).copied() {
            Some(WindowKind::Top) => self.show_top_menu(hwnd),
            Some(WindowKind::Dock) => self.activate_dock_item(hwnd, false),
            Some(WindowKind::Preview) => {
                if let Some(preview) = self.preview.as_ref() {
                    unsafe {
                        let _ = SetForegroundWindow(preview.source);
                    }
                }
            }
            _ => {}
        }
    }

    fn show_menu(&mut self, hwnd: HWND) {
        if self.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Dock)
            && let Some(index) = self.hover.get(&(hwnd.0 as isize)).copied().flatten()
            && let Some(item) = self.dock_items(hwnd).get(index).cloned()
        {
            self.show_item_menu(hwnd, item);
            return;
        }
        unsafe {
            let menu = CreatePopupMenu().expect("menu");
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
            let menu = CreatePopupMenu().expect("menu");
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
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_DOWN, w!("Volume down"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_UP, w!("Volume up"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_VOLUME_MUTE, w!("Mute / unmute"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_SETTINGS, w!("YumeDock settings"));
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

    fn show_item_menu(&mut self, hwnd: HWND, item: DockItem) {
        self.menu_item = Some(item.clone());
        unsafe {
            let menu = CreatePopupMenu().expect("menu");
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
            CMD_SETTINGS => {
                if let Ok(path) = crate::config::app_data_dir().map(|p| p.join("config.json")) {
                    launch_path(&path);
                }
            }
            CMD_REDUCE_MOTION => {
                self.config.behavior.reduce_motion = !self.config.behavior.reduce_motion
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

    fn show_folder_stack(&self, hwnd: HWND, pin: &PinConfig) {
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
        entries.truncate(24);
        unsafe {
            let menu = CreatePopupMenu().expect("folder menu");
            let _ = AppendMenuW(menu, MF_STRING, 499, w!("Open folder"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            for (index, path) in entries.iter().enumerate() {
                let label = windows::core::HSTRING::from(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref(),
                );
                let _ = AppendMenuW(menu, MF_STRING, 500 + index, &label);
            }
            let mut point = POINT::default();
            let _ = GetCursorPos(&mut point);
            let command = TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, Some(0), hwnd, None)
                .0 as usize;
            let _ = DestroyMenu(menu);
            if command == 499 {
                launch_path(&pin.path);
            } else if let Some(path) = command
                .checked_sub(500)
                .and_then(|index| entries.get(index))
            {
                launch_path(path);
            }
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
            with_app(|app| app.mouse_move(hwnd, (lparam.0 as i16) as i32));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_app(|app| app.mouse_leave(hwnd));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_app(|app| app.left_click(hwnd));
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
        WM_TIMER if wparam.0 == 2 => {
            with_app(|app| app.animate(hwnd));
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == 3 => {
            with_app(|app| app.close_preview());
            return LRESULT(0);
        }
        WM_TIMER => {
            let _ = InvalidateRect(Some(hwnd), None, false);
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => {
            with_app(|app| app.rebuild_monitors());
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            with_app(|app| app.refresh_windows());
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
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED;
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
        SetLayeredWindowAttributes(
            hwnd,
            Default::default(),
            if bar { 236 } else { 248 },
            LWA_ALPHA,
        )?;
        if !bar {
            let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, 32, 32);
            if !region.is_invalid() {
                SetWindowRgn(hwnd, Some(region), true);
            }
        }
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
        } => identity.display_name.clone(),
        DockItem::Application { .. } => "App".into(),
        DockItem::RecycleBin => "Recycle Bin".into(),
    }
}

fn dock_icon_path(item: &DockItem) -> Option<std::path::PathBuf> {
    match item {
        DockItem::Application { pin: Some(pin), .. } => Some(preferred_pin_icon(pin)),
        DockItem::Folder(pin) => Some(pin.path.clone()),
        DockItem::Application {
            identity: Some(identity),
            ..
        } => Some(identity.executable.clone()),
        DockItem::RecycleBin => Some(std::path::PathBuf::from(r"C:\$Recycle.Bin")),
        _ => None,
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
    let raw = path.to_string_lossy();
    let expanded = if raw
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("%windir%"))
    {
        std::env::var_os("WINDIR")
            .map(std::path::PathBuf::from)
            .map(|root| {
                root.join(
                    raw.get(9..)
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

fn first_window(item: &DockItem) -> Option<HWND> {
    match item {
        DockItem::Application { windows, .. } => windows.first().copied(),
        _ => None,
    }
}

fn send_media_key(key: u8) {
    unsafe {
        keybd_event(key, 0, Default::default(), 0);
        keybd_event(key, 0, KEYEVENTF_KEYUP, 0);
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
    use super::friendly_app_name;

    #[test]
    fn replaces_raw_windows_process_names() {
        assert_eq!(friendly_app_name("msedge"), "Microsoft Edge");
        assert_eq!(friendly_app_name("explorer"), "File Explorer");
        assert_eq!(friendly_app_name("YumePlayer"), "YumePlayer");
    }
}
