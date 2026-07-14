use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConfigV1 {
    pub version: u32,
    pub dock: DockConfig,
    pub top_bar: TopBarConfig,
    pub behavior: BehaviorConfig,
    pub pins: Vec<PinConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DockConfig {
    pub icon_size: f32,
    pub magnification: f32,
    pub animation_ms: u32,
    pub opacity: f32,
    pub height: i32,
    pub auto_hide: bool,
    pub auto_hide_delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TopBarConfig {
    pub height: i32,
    pub opacity: f32,
    pub use_24_hour_clock: bool,
    pub show_network: bool,
    pub show_volume: bool,
    pub show_battery: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BehaviorConfig {
    pub replace_taskbar: bool,
    pub start_with_windows: bool,
    pub reduce_motion: bool,
    pub reserve_edges: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinConfig {
    pub label: String,
    pub path: PathBuf,
    #[serde(default)]
    pub identity_path: Option<PathBuf>,
    #[serde(default)]
    pub kind: PinKind,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    #[default]
    Application,
    Folder,
    RecycleBin,
}

impl Default for ConfigV1 {
    fn default() -> Self {
        Self {
            version: 1,
            dock: DockConfig::default(),
            top_bar: TopBarConfig::default(),
            behavior: BehaviorConfig::default(),
            pins: Vec::new(),
        }
    }
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            icon_size: 48.0,
            magnification: 1.42,
            animation_ms: 160,
            opacity: 0.82,
            height: 76,
            auto_hide: true,
            auto_hide_delay_ms: 650,
        }
    }
}

impl Default for TopBarConfig {
    fn default() -> Self {
        Self {
            height: 32,
            opacity: 0.88,
            use_24_hour_clock: false,
            show_network: true,
            show_volume: true,
            show_battery: true,
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            replace_taskbar: true,
            start_with_windows: false,
            reduce_motion: false,
            reserve_edges: true,
        }
    }
}

impl ConfigV1 {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            let config = Self {
                pins: import_taskbar_pins(),
                ..Self::default()
            };
            config.save()?;
            return Ok(config);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let mut config: Self = serde_json::from_str(&text).context("parse YumeDock config")?;
        for pin in &mut config.pins {
            if pin.identity_path.is_none()
                && pin
                    .path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
            {
                pin.identity_path = resolve_shortcut(&pin.path);
            }
        }
        config.validate();
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, path)?;
        Ok(())
    }

    pub fn validate(&mut self) {
        self.version = 1;
        self.dock.icon_size = self.dock.icon_size.clamp(32.0, 80.0);
        self.dock.magnification = self.dock.magnification.clamp(1.0, 1.8);
        self.dock.animation_ms = self.dock.animation_ms.clamp(0, 500);
        self.dock.opacity = self.dock.opacity.clamp(0.35, 1.0);
        self.dock.height = self.dock.height.clamp(56, 120);
        self.dock.auto_hide_delay_ms = self.dock.auto_hide_delay_ms.clamp(100, 5000);
        self.top_bar.height = self.top_bar.height.clamp(26, 48);
        self.top_bar.opacity = self.top_bar.opacity.clamp(0.35, 1.0);
        self.pins
            .retain(|p| p.kind == PinKind::RecycleBin || p.path.exists());
    }
}

pub fn app_data_dir() -> Result<PathBuf> {
    let root = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?;
    Ok(PathBuf::from(root).join("YumeDock"))
}

fn config_path() -> Result<PathBuf> {
    Ok(app_data_dir()?.join("config.json"))
}

fn import_taskbar_pins() -> Vec<PinConfig> {
    let Some(appdata) = env::var_os("APPDATA") else {
        return Vec::new();
    };
    let dir = PathBuf::from(appdata)
        .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar");
    let mut pins: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")))
        .map(|path| PinConfig {
            label: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            identity_path: resolve_shortcut(&path),
            path,
            kind: PinKind::Application,
        })
        .collect();
    pins.sort_by_key(|a| a.label.to_lowercase());
    pins.push(PinConfig {
        label: "Recycle Bin".into(),
        path: PathBuf::new(),
        identity_path: None,
        kind: PinKind::RecycleBin,
    });
    pins
}

pub(crate) fn resolve_shortcut(path: &std::path::Path) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ},
            UI::Shell::{IShellLinkW, SLGP_RAWPATH, ShellLink},
        },
        core::{Interface, PCWSTR},
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut target = vec![0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .ok()?;
        let len = target.iter().position(|c| *c == 0)?;
        (len > 0).then(|| PathBuf::from(String::from_utf16_lossy(&target[..len])))
    }
}

pub fn sync_startup(enabled: bool) -> Result<()> {
    let appdata = env::var_os("APPDATA").context("APPDATA is unavailable")?;
    let startup =
        PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup\YumeDock.cmd");
    if enabled {
        let exe = env::current_exe()?;
        let command = format!("@start \"\" \"{}\"\r\n", exe.display());
        fs::write(startup, command)?;
    } else if startup.exists() {
        fs::remove_file(startup)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_clamps_visual_values() {
        let mut c = ConfigV1::default();
        c.dock.icon_size = 500.0;
        c.dock.opacity = -2.0;
        c.dock.auto_hide_delay_ms = 1;
        c.top_bar.height = 2;
        c.validate();
        assert_eq!(c.dock.icon_size, 80.0);
        assert_eq!(c.dock.opacity, 0.35);
        assert_eq!(c.dock.auto_hide_delay_ms, 100);
        assert_eq!(c.top_bar.height, 26);
    }
}
