#![allow(unsafe_op_in_unsafe_fn)]

use crate::model::{AppIdentity, WindowEntry};
use std::{
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        Graphics::{
            Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute},
            Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow},
        },
        System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
        },
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            WindowsAndMessaging::{
                CHILDID_SELF, EVENT_OBJECT_CREATE, EVENT_OBJECT_DESTROY, EVENT_OBJECT_SHOW,
                EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MOVESIZEEND, EnumWindows, GW_OWNER,
                GWL_EXSTYLE, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW,
                GetWindowThreadProcessId, IsIconic, IsWindowVisible, OBJID_WINDOW,
                PostThreadMessageW, SW_HIDE, ShowWindow, WINEVENT_OUTOFCONTEXT,
                WINEVENT_SKIPOWNPROCESS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            },
        },
    },
    core::PWSTR,
};

static OWN_PID: OnceLock<u32> = OnceLock::new();
static UI_THREAD: AtomicU32 = AtomicU32::new(0);
static REFRESH_MESSAGE: AtomicU32 = AtomicU32::new(0);
static REFRESH_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct HookSet(Vec<HWINEVENTHOOK>);

impl HookSet {
    pub fn install(thread_id: u32, refresh_message: u32) -> Self {
        UI_THREAD.store(thread_id, Ordering::Relaxed);
        REFRESH_MESSAGE.store(refresh_message, Ordering::Relaxed);
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let mut hooks = Vec::new();
        unsafe {
            let foreground = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(win_event),
                0,
                0,
                flags,
            );
            if !foreground.is_invalid() {
                hooks.push(foreground);
            }
            for event in [
                EVENT_OBJECT_CREATE,
                EVENT_OBJECT_DESTROY,
                EVENT_OBJECT_SHOW,
                EVENT_SYSTEM_MOVESIZEEND,
            ] {
                let hook = SetWinEventHook(event, event, None, Some(win_event), 0, 0, flags);
                if !hook.is_invalid() {
                    hooks.push(hook);
                }
            }
        }
        Self(hooks)
    }
}

impl Drop for HookSet {
    fn drop(&mut self) {
        for hook in self.0.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(hook);
            }
        }
    }
}

unsafe extern "system" fn win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event == EVENT_OBJECT_SHOW
        && crate::shell::should_suppress_taskbar()
        && crate::shell::is_taskbar_window(hwnd)
    {
        let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        return;
    }
    if hwnd.is_invalid() || object != OBJID_WINDOW.0 || child != CHILDID_SELF as i32 {
        return;
    }
    let thread = UI_THREAD.load(Ordering::Relaxed);
    let message = REFRESH_MESSAGE.load(Ordering::Relaxed);
    if thread != 0
        && message != 0
        && REFRESH_PENDING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
    {
        let _ =
            unsafe { PostThreadMessageW(thread, message, Default::default(), Default::default()) };
    }
}

pub fn mark_refresh_handled() {
    REFRESH_PENDING.store(false, Ordering::Release);
}

unsafe extern "system" fn enum_window(hwnd: HWND, data: LPARAM) -> windows::core::BOOL {
    let list = &mut *(data.0 as *mut Vec<WindowEntry>);
    if !IsWindowVisible(hwnd).as_bool() || GetWindow(hwnd, GW_OWNER).is_ok_and(|h| !h.is_invalid())
    {
        return windows::core::BOOL(1);
    }
    let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if style & (WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) != 0 {
        return windows::core::BOOL(1);
    }
    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut _ as *mut _,
        std::mem::size_of_val(&cloaked) as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return windows::core::BOOL(1);
    }
    let title_len = GetWindowTextLengthW(hwnd);
    if title_len <= 0 {
        return windows::core::BOOL(1);
    }

    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == *OWN_PID.get_or_init(std::process::id) {
        return windows::core::BOOL(1);
    }
    let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return windows::core::BOOL(1);
    };
    let mut exe = vec![0u16; 32768];
    let mut exe_len = exe.len() as u32;
    let executable = if QueryFullProcessImageNameW(
        process,
        Default::default(),
        PWSTR(exe.as_mut_ptr()),
        &mut exe_len,
    )
    .is_ok()
    {
        PathBuf::from(String::from_utf16_lossy(&exe[..exe_len as usize]))
    } else {
        let _ = CloseHandle(process);
        return windows::core::BOOL(1);
    };
    let _ = CloseHandle(process);

    let mut title = vec![0u16; title_len as usize + 1];
    let read = GetWindowTextW(hwnd, &mut title);
    title.truncate(read.max(0) as usize);
    let display_name = executable
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let app_user_model_id = app_user_model_id(hwnd);
    let icon_key = app_user_model_id
        .clone()
        .unwrap_or_else(|| executable.to_string_lossy().to_lowercase());
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST).0 as isize;
    list.push(WindowEntry {
        hwnd,
        identity: AppIdentity {
            app_user_model_id,
            executable,
            display_name,
            icon_key,
        },
        monitor,
        title: String::from_utf16_lossy(&title),
        minimized: IsIconic(hwnd).as_bool(),
    });
    windows::core::BOOL(1)
}

unsafe fn app_user_model_id(hwnd: HWND) -> Option<String> {
    use windows::Win32::{
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::Com::StructuredStorage::{PropVariantClear, PropVariantToString},
        UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
    };
    let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
    let mut value = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
    let mut buffer = [0u16; 512];
    let result = PropVariantToString(&value, &mut buffer);
    let _ = PropVariantClear(&mut value);
    result.ok()?;
    let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    (len > 0).then(|| String::from_utf16_lossy(&buffer[..len]))
}

pub fn enumerate_windows() -> Vec<WindowEntry> {
    let mut result = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_window), LPARAM(&mut result as *mut _ as isize));
    }
    result
}
