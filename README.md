# YumeDock

YumeDock is a lightweight Windows 11 dock and top bar written in native Rust.

## Build

```powershell
cargo build --release
```

The portable executable is created at `target\release\YumeDock.exe`.

## Safety

YumeDock starts a watchdog before hiding the Windows taskbar. Normal shutdown,
the emergency shortcut (`Ctrl+Alt+Shift+F12`), and watchdog crash recovery all
restore the taskbar.

Run without replacing the Windows taskbar while developing:

```powershell
target\release\YumeDock.exe --safe-mode
```

Configuration is stored in `%LOCALAPPDATA%\YumeDock\config.json`.

