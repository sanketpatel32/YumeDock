# YumeDock

A lightweight, native **macOS-style dock and menu bar for Windows 11**, written in Rust. It replaces the Windows taskbar with a calm, polished top bar and a magnifying dock — without a heavy browser-based shell layer.

---

## Install

Open **PowerShell** and paste this one line:

```powershell
irm https://raw.githubusercontent.com/sanketpatel32/YumeDock/main/install.ps1 | iex
```

This downloads the latest `YumeDock.exe` to `%LOCALAPPDATA%\Programs\YumeDock` and creates a Start Menu shortcut — no installer, no admin prompt. Launch it from the Start Menu afterward (search "YumeDock").

> Windows SmartScreen may warn the first time because the exe isn't code-signed yet. Click **More info → Run anyway**. See [Releases](https://github.com/sanketpatel32/YumeDock/releases) to download manually.

---

## ⚠️ Safety — read before first run

YumeDock hides the Windows taskbar by default. You can **always** get it back:

| How | What it does |
| --- | --- |
| **`Ctrl` + `Alt` + `Shift` + `F12`** | Emergency hotkey — quits YumeDock and restores the taskbar immediately. |
| Right-click the top bar → *Restore taskbar and quit* | Normal shutdown with restore. |
| Task Manager → end `YumeDock` | The watchdog restores the taskbar on crash/kill. |

**Want to try it without hiding your taskbar?** Run `YumeDock.exe --safe-mode`. The dock and top bar appear, but the Windows taskbar stays visible.

---

## What it does

- **Top menu bar** — a neutral mark + the name of your active app on the left; on the right, discrete clickable segments: Wi-Fi, volume, battery, and a clock with the date. Each segment lights up on hover and opens the matching Windows setting on click.
- **Dock** — a macOS-style dock with smooth magnification, running-app indicators, drag-to-reorder pins, and folder/recycle-bin stacks. Imported automatically from your existing taskbar pins on first run.
- **Native and light** — pure Rust + Direct2D/DirectComposition. No Electron, no WebView, no browser layer.

Settings live in `%LOCALAPPDATA%\YumeDock\config.json` and **survive updates**.

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

## Releasing (build on your own PC)

There is no CI pipeline — releases are built locally. One script does everything:

```powershell
.\build-release.ps1 -Latest
```

This:
1. Generates the app icon (`python assets/make_icon.py`).
2. Builds the release binary (`cargo build --release`) with the icon embedded.
3. Zips the exe + a short README into `dist\`.
4. Publishes a GitHub Release (rolling **`latest`** tag) with `YumeDock.exe` and the zip attached.

The one-line install command at the top always pulls from that `latest` release, so re-running `.\build-release.ps1 -Latest` updates everyone.

**Requirements to build:** Rust toolchain, Python 3, and the [GitHub CLI](https://cli.github.com/) (`gh auth login` once).

Other options:
```powershell
.\build-release.ps1                  # versioned as <cargo-version>+<git-sha>
.\build-release.ps1 -Tag v0.2.0      # explicit versioned release
```

---

## Uninstall

Delete the install folder (`%LOCALAPPDATA%\Programs\YumeDock`) and the Start Menu shortcut. To keep your settings or start fresh: `%LOCALAPPDATA%\YumeDock\config.json`.

---

## Build from source (development)

Requires the [Rust toolchain](https://rustup.rs/) (stable) and Python 3 for the icon.

```powershell
python assets\make_icon.py     # generate the app icon (stdlib, no packages)
cargo build --release
target\release\YumeDock.exe
cargo test                     # 20 unit tests
```

If `assets/yumedock.ico` is absent, the build still succeeds (just without the icon).
