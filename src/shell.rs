#![allow(unsafe_op_in_unsafe_fn)]

use anyhow::{Context, Result};
use std::{
    env,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        UI::{
            Shell::{
                ABE_BOTTOM, ABE_TOP, ABM_GETSTATE, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS,
                ABM_SETSTATE, ABS_AUTOHIDE, APPBARDATA, SHAppBarMessage,
            },
            WindowsAndMessaging::{
                EnumWindows, FindWindowW, IsWindowVisible, SW_HIDE, SW_SHOW, ShowWindow,
            },
        },
    },
    core::PCWSTR,
};

static SUPPRESS_TASKBAR: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Default)]
pub struct TaskbarState {
    windows: Vec<(HWND, bool)>,
    appbar_state: u32,
    hidden: bool,
}

impl TaskbarState {
    pub fn capture() -> Self {
        let appbar_state = get_taskbar_appbar_state();
        let windows = taskbar_windows()
            .into_iter()
            .map(|hwnd| {
                let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
                (hwnd, visible)
            })
            .collect();
        Self {
            windows,
            appbar_state,
            hidden: false,
        }
    }

    pub fn hide(&mut self) {
        SUPPRESS_TASKBAR.store(true, Ordering::Release);
        set_taskbar_appbar_state(self.appbar_state | ABS_AUTOHIDE);
        for (hwnd, _) in &self.windows {
            unsafe {
                let _ = ShowWindow(*hwnd, SW_HIDE);
            }
        }
        self.hidden = true;
    }

    pub fn refresh_and_hide(&mut self) {
        self.windows = taskbar_windows()
            .into_iter()
            .map(|hwnd| {
                let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
                (hwnd, visible)
            })
            .collect();
        self.hidden = false;
        self.hide();
    }

    pub fn restore(&mut self) {
        if !self.hidden {
            return;
        }
        SUPPRESS_TASKBAR.store(false, Ordering::Release);
        for (hwnd, was_visible) in &self.windows {
            if *was_visible {
                unsafe {
                    let _ = ShowWindow(*hwnd, SW_SHOW);
                }
            }
        }
        set_taskbar_appbar_state(self.appbar_state);
        self.hidden = false;
    }
}

pub fn should_suppress_taskbar() -> bool {
    SUPPRESS_TASKBAR.load(Ordering::Acquire)
}

pub fn is_taskbar_window(hwnd: HWND) -> bool {
    let mut class = [0u16; 64];
    let len = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class) };
    if len <= 0 {
        return false;
    }
    matches!(
        &String::from_utf16_lossy(&class[..len as usize])[..],
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

impl Drop for TaskbarState {
    fn drop(&mut self) {
        self.restore();
    }
}

unsafe extern "system" fn collect_secondary(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    if is_taskbar_window(hwnd) {
        let mut class = [0u16; 64];
        let len = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class);
        if len <= 0 || String::from_utf16_lossy(&class[..len as usize]) != "Shell_SecondaryTrayWnd"
        {
            return windows::core::BOOL(1);
        }
        (*(lparam.0 as *mut Vec<HWND>)).push(hwnd);
    }
    windows::core::BOOL(1)
}

pub fn taskbar_windows() -> Vec<HWND> {
    let mut result = Vec::new();
    unsafe {
        let main = FindWindowW(windows::core::w!("Shell_TrayWnd"), PCWSTR::null());
        if let Ok(hwnd) = main
            && !hwnd.is_invalid()
        {
            result.push(hwnd);
        }
        let _ = EnumWindows(
            Some(collect_secondary),
            LPARAM(&mut result as *mut _ as isize),
        );
    }
    result
}

pub fn force_restore_taskbars() {
    for hwnd in taskbar_windows() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
    }
}

fn get_taskbar_appbar_state() -> u32 {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        ..Default::default()
    };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 }
}

fn set_taskbar_appbar_state(state: u32) {
    let mut data = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        lParam: windows::Win32::Foundation::LPARAM(state as isize),
        ..Default::default()
    };
    unsafe {
        SHAppBarMessage(ABM_SETSTATE, &mut data);
    }
}

pub fn spawn_watchdog() -> Result<()> {
    let exe = env::current_exe().context("locate YumeDock executable")?;
    let appbar_state = get_taskbar_appbar_state();
    Command::new(exe)
        .arg("--watchdog")
        .arg(std::process::id().to_string())
        .arg(appbar_state.to_string())
        .spawn()
        .context("start taskbar watchdog")?;
    Ok(())
}

pub fn run_watchdog(pid: u32, appbar_state: u32) -> Result<()> {
    unsafe {
        let process =
            OpenProcess(PROCESS_SYNCHRONIZE, false, pid).context("open YumeDock process")?;
        let _ = WaitForSingleObject(process, u32::MAX);
        let _ = CloseHandle(process);
    }
    std::thread::sleep(Duration::from_millis(100));
    force_restore_taskbars();
    set_taskbar_appbar_state(appbar_state);
    Ok(())
}

pub struct AppBar {
    hwnd: HWND,
    registered: bool,
}

impl AppBar {
    pub fn register(
        hwnd: HWND,
        top: bool,
        mut rect: windows::Win32::Foundation::RECT,
    ) -> Result<Self> {
        let edge = if top { ABE_TOP } else { ABE_BOTTOM };
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd,
            uCallbackMessage: windows::Win32::UI::WindowsAndMessaging::WM_APP + 8,
            uEdge: edge,
            rc: rect,
            ..Default::default()
        };
        unsafe {
            if SHAppBarMessage(ABM_NEW, &mut data) == 0 {
                anyhow::bail!("ABM_NEW failed")
            }
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
            data.rc = rect;
            if top {
                data.rc.bottom = data.rc.top + (rect.bottom - rect.top);
            } else {
                data.rc.top = data.rc.bottom - (rect.bottom - rect.top);
            }
            if SHAppBarMessage(ABM_SETPOS, &mut data) == 0 {
                SHAppBarMessage(ABM_REMOVE, &mut data);
                anyhow::bail!("ABM_SETPOS failed")
            }
            rect = data.rc;
            windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            )?;
        }
        Ok(Self {
            hwnd,
            registered: true,
        })
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        if self.registered {
            let mut data = APPBARDATA {
                cbSize: std::mem::size_of::<APPBARDATA>() as u32,
                hWnd: self.hwnd,
                ..Default::default()
            };
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut data);
            }
        }
    }
}
