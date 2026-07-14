#![allow(unsafe_op_in_unsafe_fn)]

use crate::config::{PinConfig, PinKind};
use std::{collections::BTreeMap, path::PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

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
    pub dpi: u32,
}

impl MonitorInfo {
    pub fn scale(self) -> f32 {
        self.dpi.max(96) as f32 / 96.0
    }
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
        let mut dpi_x = 96;
        let mut dpi_y = 96;
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        monitors.push(MonitorInfo {
            handle: monitor,
            bounds: info.rcMonitor,
            work: info.rcWork,
            primary: info.dwFlags & 1 != 0,
            dpi: dpi_x.max(96),
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
    let mut folders = Vec::new();
    let mut has_recycle_bin = false;
    for pin in pins {
        match pin.kind {
            PinKind::Folder => folders.push(DockItem::Folder(pin.clone())),
            PinKind::RecycleBin => has_recycle_bin = true,
            PinKind::Application => {
                let target = pin.identity_path.as_ref().unwrap_or(&pin.path);
                let key = by_exe
                    .iter()
                    .find_map(|(key, group)| {
                        group.first().and_then(|window| {
                            (window
                                .identity
                                .executable
                                .to_string_lossy()
                                .eq_ignore_ascii_case(&target.to_string_lossy())
                                || app_label_key(&window.identity.display_name)
                                    == app_label_key(&pin.label))
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
    items.extend(folders);
    if has_recycle_bin {
        items.push(DockItem::RecycleBin);
    }
    items
}

fn app_label_key(label: &str) -> String {
    let key: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    match key.as_str() {
        "explorer" => "fileexplorer".into(),
        "steamwebhelper" => "steam".into(),
        _ => key,
    }
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

    #[test]
    fn merges_running_app_with_matching_pin_label() {
        let pin = PinConfig {
            label: "Discord".into(),
            path: "Discord.lnk".into(),
            identity_path: Some("Update.exe".into()),
            kind: PinKind::Application,
        };
        let identity = AppIdentity {
            app_user_model_id: Some("com.squirrel.Discord.Discord".into()),
            executable: "Discord.exe".into(),
            display_name: "Discord".into(),
            icon_key: "com.squirrel.Discord.Discord".into(),
        };
        let items = group_for_monitor(
            &[pin],
            &[WindowEntry {
                hwnd: HWND(1 as _),
                identity,
                monitor: 7,
                title: "Discord".into(),
                minimized: false,
            }],
            7,
        );
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0],
            DockItem::Application { windows, .. } if windows.len() == 1
        ));
    }

    #[test]
    fn keeps_recycle_bin_last() {
        let pins = vec![
            PinConfig {
                label: "Recycle Bin".into(),
                path: PathBuf::new(),
                identity_path: None,
                kind: PinKind::RecycleBin,
            },
            PinConfig {
                label: "Folder".into(),
                path: "Folder".into(),
                identity_path: None,
                kind: PinKind::Folder,
            },
        ];
        let items = group_for_monitor(&pins, &[], 7);
        assert!(matches!(items.last(), Some(DockItem::RecycleBin)));
    }
}
