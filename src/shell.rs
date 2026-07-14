#![allow(unsafe_op_in_unsafe_fn)]

use anyhow::{Context, Result};
use std::{env, process::Command, time::Duration};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND, LPARAM},
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        UI::{
            Shell::{
                ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA,
                SHAppBarMessage,
            },
            WindowsAndMessaging::{
                EnumWindows, FindWindowW, IsWindowVisible, SW_HIDE, SW_SHOW, ShowWindow,
            },
        },
    },
    core::PCWSTR,
};

#[derive(Debug, Default)]
pub struct TaskbarState {
    windows: Vec<(HWND, bool)>,
    hidden: bool,
}

impl TaskbarState {
    pub fn capture() -> Self {
        let windows = taskbar_windows()
            .into_iter()
            .map(|hwnd| {
                let visible = unsafe { IsWindowVisible(hwnd).as_bool() };
                (hwnd, visible)
            })
            .collect();
        Self {
            windows,
            hidden: false,
        }
    }

    pub fn hide(&mut self) {
        for (hwnd, _) in &self.windows {
            unsafe {
                let _ = ShowWindow(*hwnd, SW_HIDE);
            }
        }
        self.hidden = true;
    }

    pub fn refresh_and_hide(&mut self) {
        *self = Self::capture();
        self.hide();
    }

    pub fn restore(&mut self) {
        if !self.hidden {
            return;
        }
        for (hwnd, was_visible) in &self.windows {
            if *was_visible {
                unsafe {
                    let _ = ShowWindow(*hwnd, SW_SHOW);
                }
            }
        }
        self.hidden = false;
    }
}

impl Drop for TaskbarState {
    fn drop(&mut self) {
        self.restore();
    }
}

unsafe extern "system" fn collect_secondary(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let mut class = [0u16; 64];
    let len = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(hwnd, &mut class);
    if len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "Shell_SecondaryTrayWnd" {
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

pub fn spawn_watchdog() -> Result<()> {
    let exe = env::current_exe().context("locate YumeDock executable")?;
    Command::new(exe)
        .arg("--watchdog")
        .arg(std::process::id().to_string())
        .spawn()
        .context("start taskbar watchdog")?;
    Ok(())
}

pub fn run_watchdog(pid: u32) -> Result<()> {
    unsafe {
        let process =
            OpenProcess(PROCESS_SYNCHRONIZE, false, pid).context("open YumeDock process")?;
        let _ = WaitForSingleObject(process, u32::MAX);
        let _ = CloseHandle(process);
    }
    std::thread::sleep(Duration::from_millis(100));
    force_restore_taskbars();
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
