# YumeDock

A lightweight, native **macOS-style dock and menu bar for Windows 11**, written in Rust. It replaces the Windows taskbar with a calm, polished top bar and a magnifying dock — without a heavy browser-based shell layer.

---

## Install

Open **PowerShell** and paste this one line:

```powershell
irm https://raw.githubusercontent.com/sanketpatel32/YumeDock/main/install.ps1 | iex
```

This downloads the latest `YumeDock-Setup.exe` and installs it **per-user** — no administrator prompt, no UAC. Launch it from the Start Menu afterward (search "YumeDock").

> Windows SmartScreen may warn the first time because the installer isn't code-signed yet. Click **More info → Run anyway**. See [Releases](https://github.com/sanketpatel32/YumeDock/releases) to download manually.

---

## ⚠️ Safety — read before first run

YumeDock hides the Windows taskbar by default. You can **always** get it back:

| How | What it does |
| --- | --- |
| **`Ctrl` + `Alt` + `Shift` + `F12`** | Emergency hotkey — quits YumeDock and restores the taskbar immediately. |
| Right-click the top bar → *Restore taskbar and quit* | Normal shutdown with restore. |
| Task Manager → end `YumeDock` | The watchdog restores the taskbar on crash/kill. |

**Want to try it without hiding your taskbar?** Launch the *YumeDock (safe mode)* Start Menu shortcut, or run `YumeDock.exe --safe-mode`. The dock and top bar appear, but the Windows taskbar stays visible.

---

## What it does

- **Top menu bar** — a neutral mark + the name of your active app on the left; on the right, discrete clickable segments: Wi-Fi, volume, battery, and a clock with the date. Each segment lights up on hover and opens the matching Windows setting on click.
- **Dock** — a macOS-style dock with smooth magnification, running-app indicators, drag-to-reorder pins, and folder/recycle-bin stacks. Imported automatically from your existing taskbar pins on first run.
- **Native and light** — pure Rust + Direct2D/DirectComposition. No Electron, no WebView, no browser layer.

Settings live in `%LOCALAPPDATA%\YumeDock\config.json` and **survive uninstall/reinstall**.

---

## Configuration

Open the settings file (right-click the top bar → *Open settings file*) or edit it directly:

```json
{
  "top_bar": {
    "show_network": true,
    "show_volume": true,
    "show_battery": true,
    "use_24_hour_clock": false
  },
  "dock": { "auto_hide": true, "icon_size": 48.0, "magnification": 1.42 },
  "behavior": { "replace_taskbar": true, "start_with_windows": false }
}
```

Toggle the right-side menu-bar segments, dock magnification, auto-hide, and whether YumeDock replaces the Windows taskbar on startup.

---

## Build from source

Requires the [Rust toolchain](https://rustup.rs/) (stable) and, for the embedded app icon, Python 3.

```powershell
# Generate the app icon (stdlib Python, no extra packages):
python assets/make_icon.py

# Build:
cargo build --release

# The portable executable:
target\release\YumeDock.exe
```

The icon is embedded via `build.rs` + `embed-resource`. If `assets/yumedock.ico` is absent, the build still succeeds (just without the icon).

Run tests:

```powershell
cargo test
```

### Produce the installer locally

```powershell
# Install Inno Setup 6 (https://jrsoftware.org/isdl.php), then:
iscc /DAPP_VERSION=0.1.0 /DAPP_SOURCE="..\target\release\YumeDock.exe" installer\yumedock.iss
# -> installer\Output\YumeDock-Setup.exe
```

---

## How releases work

Every push to `main` triggers the [`release`](.github/workflows/release.yml) GitHub Actions workflow, which:

1. Builds the release binary (with the embedded icon).
2. Runs the test suite.
3. Compiles the Inno Setup installer.
4. Publishes both to a rolling **`latest`** [Release](https://github.com/sanketpatel32/YumeDock/releases) (pre-release), replacing the previous one.

The one-line install command always pulls the newest build from that rolling release. To update an existing install later, just re-run the one-liner.

---

## Uninstall

Settings → Apps → *YumeDock* → Uninstall. The install directory is removed; your `config.json` (in `%LOCALAPPDATA%\YumeDock`) is preserved.

---

## License

See the repository for licensing terms.
