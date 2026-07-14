#![allow(unsafe_op_in_unsafe_fn)]

use crate::config::{PinConfig, PinKind};
use std::{collections::BTreeMap, path::PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HMONITOR, MONITORINFO};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppIdentity {
    pub app_user_model_id: Option<String>,
    pub executable: PathBuf,
    pub display_name: String,
    pub icon_key: String,
}

#[derive(Debug, Clone)]
pub struct WindowEntry {
    pub hwnd: HWND,
    pub identity: AppIdentity,
    pub monitor: isize,
    #[allow(dead_code)]
    pub title: String,
    #[allow(dead_code)]
    pub minimized: bool,
}

#[derive(Debug, Clone)]
pub enum DockItem {
    Application {
        pin: Option<PinConfig>,
        identity: Option<AppIdentity>,
        windows: Vec<HWND>,
    },
    Folder(PinConfig),
    RecycleBin,
}

#[derive(Debug, Clone, Copy)]
pub struct MonitorInfo {
    pub handle: HMONITOR,
    pub bounds: RECT,
    #[allow(dead_code)]
    pub work: RECT,
    pub primary: bool,
}

unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _dc: windows::Win32::Graphics::Gdi::HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let monitors = &mut *(data.0 as *mut Vec<MonitorInfo>);
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        monitors.push(MonitorInfo {
            handle: monitor,
            bounds: info.rcMonitor,
            work: info.rcWork,
            primary: info.dwFlags & 1 != 0,
        });
    }
    windows::core::BOOL(1)
}

pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_callback),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }
    monitors.sort_by_key(|m| (!m.primary, m.bounds.left, m.bounds.top));
    monitors
}

pub fn group_for_monitor(
    pins: &[PinConfig],
    windows: &[WindowEntry],
    monitor: isize,
) -> Vec<DockItem> {
    let mut by_exe: BTreeMap<String, Vec<&WindowEntry>> = BTreeMap::new();
    for window in windows.iter().filter(|w| w.monitor == monitor) {
        by_exe
            .entry(window.identity.icon_key.to_lowercase())
            .or_default()
            .push(window);
    }

    let mut items = Vec::new();
    for pin in pins {
        match pin.kind {
            PinKind::Folder => items.push(DockItem::Folder(pin.clone())),
            PinKind::RecycleBin => items.push(DockItem::RecycleBin),
            PinKind::Application => {
                let target = pin.identity_path.as_ref().unwrap_or(&pin.path);
                let key = by_exe
                    .iter()
                    .find_map(|(key, group)| {
                        group.first().and_then(|window| {
                            window
                                .identity
                                .executable
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&target.to_string_lossy())
                                .then(|| key.clone())
                        })
                    })
                    .unwrap_or_else(|| target.to_string_lossy().to_lowercase());
                let matching = by_exe.remove(&key).unwrap_or_default();
                items.push(DockItem::Application {
                    pin: Some(pin.clone()),
                    identity: matching.first().map(|w| w.identity.clone()),
                    windows: matching.iter().map(|w| w.hwnd).collect(),
                });
            }
        }
    }
    for group in by_exe.into_values() {
        let Some(first) = group.first() else { continue };
        items.push(DockItem::Application {
            pin: None,
            identity: Some(first.identity.clone()),
            windows: group.iter().map(|w| w.hwnd).collect(),
        });
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_only_monitor_local_windows() {
        let identity = AppIdentity {
            app_user_model_id: None,
            executable: "test.exe".into(),
            display_name: "Test".into(),
            icon_key: "test.exe".into(),
        };
        let windows = vec![WindowEntry {
            hwnd: HWND(1 as _),
            identity,
            monitor: 7,
            title: "A".into(),
            minimized: false,
        }];
        assert_eq!(group_for_monitor(&[], &windows, 7).len(), 1);
        assert!(group_for_monitor(&[], &windows, 8).is_empty());
    }
}
