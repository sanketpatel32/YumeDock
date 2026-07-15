# Phase 2 — Launcher + Quick Popovers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the first real topbar features on the Phase 1 popover infrastructure: a logo → launcher popover (quick actions + scrollable pinned-app list), and Wi-Fi / volume / battery quick popovers with real controls (volume slider, mute toggle, connectivity deep-links) instead of the current "click opens Settings."

**Architecture:** Two new `WindowKind` variants (`Launcher`, `QuickPopover`) plug into the existing `open_popover`/`close_popover` lifecycle. Each gets a `paint_*` method following the Phase 1 template (`paint_debug_popover`). The launcher enumerates Start Menu `.lnk` files at open time and caches them on `App`; icons are extracted via the existing `load_icon_bitmap` + per-surface `surface.icons` cache. Volume/mute get real setters added to `status.rs` (`IAudioEndpointVolume` is already imported). Power actions (sleep/restart/shutdown/lock) use `ShellExecuteW` with `rundll32`/`ms-settings:` verbs — no new Cargo features, no privilege-adjustment code.

**Tech Stack:** Rust 2024, `windows` crate (no new features beyond Phase 1), Direct2D, Win32 Shell. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-15-stability-and-topbar-rework-design.md`, Phase 2 (sections 2.1, 2.2).

**Conventions (carried from Phase 1):**
- Repo root `C:\Users\sanpa\OneDrive\Documents\Taskbar to MacOS`. Working tree is clean (only untracked `dist/` + `nul`).
- `#![allow(unsafe_op_in_unsafe_fn)]` is set, so FFI calls don't need wrapping `unsafe` blocks except where noted.
- Paint methods MUST follow the borrow pattern: device-loss guard → clone device-bound resources (`body`/`small`/`bold`/`wic`) into locals → `self.surface(hwnd)?` (borrows `&mut self`) → `BeginDraw`/`Clear`/draw → `present(surface)` → set `self.device_lost`.
- 30 unit tests currently pass; new tests append alongside.
- Commit + push to `main` after every task.

---

## File Structure

- **Modify `src/app.rs`** — `WindowKind::Launcher`/`QuickPopover` variants; `LauncherState` + `App::launcher` field; `QuickKind`; popover open/handler methods; segment-click wiring (Logo → launcher, Volume/Network/Battery → quick popover); `CMD_*` constants for in-popover actions + power; launcher click/mouse-move/scroll handlers; paint dispatch.
- **Modify `src/render.rs`** — `paint_launcher` + `paint_quick_popover` + pure `launcher_geometry`/`quick_popover_geometry` hit-test helpers (unit-testable, GPU-free).
- **Modify `src/status.rs`** — `set_volume(level: u8)` + `set_mute(muted: bool)` (IAudioEndpointVolume setters), refactoring the device-enumeration boilerplate into a shared helper.
- **Modify `src/config.rs`** — nothing (pins stay separate from launcher list).

---

## Task 1: Volume/mute setters in `status.rs`

The volume popover needs to actually change volume. The `IAudioEndpointVolume` API is already imported and used read-only; add the setters.

**Files:**
- Modify: `src/status.rs` (refactor device boilerplate + add `set_volume`/`set_mute`)

- [ ] **Step 1: Read the current `read_audio`** at `src/status.rs:30-52` (shown in exploration). It inlines the `CoCreateInstance`/`GetDefaultAudioEndpoint`/`Activate` chain.

- [ ] **Step 2: Extract a shared `endpoint_volume()` helper and add setters**

Replace the body of `read_audio` and add the new functions. The whole audio section of `src/status.rs` becomes:

```rust
fn endpoint_volume() -> Option<IAudioEndpointVolume> {
    use windows::Win32::{
        Media::Audio::{
            Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia,
            eRender,
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    };
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eMultimedia)
            .ok()?;
        let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        Some(endpoint)
    }
}

fn read_audio() -> Option<(Option<u8>, bool)> {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    let endpoint: IAudioEndpointVolume = endpoint_volume()?;
    unsafe {
        let volume = endpoint
            .GetMasterVolumeLevelScalar()
            .ok()
            .map(|v| (v.clamp(0.0, 1.0) * 100.0).round() as u8);
        let muted = endpoint.GetMute().ok().is_some_and(|m| m.as_bool());
        Some((volume, muted))
    }
}

/// Set the master render volume. `level` is clamped to 0..=100.
pub fn set_volume(level: u8) {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    let Some(endpoint) = endpoint_volume() else { return };
    let scalar = (level as f32).clamp(0.0, 100.0) / 100.0;
    unsafe {
        let _ = endpoint.SetMasterVolumeLevelScalar(scalar, None);
    }
}

/// Mute or unmute the default render endpoint.
pub fn set_mute(muted: bool) {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    let Some(endpoint) = endpoint_volume() else { return };
    unsafe {
        let _ = endpoint.SetMute(windows::Win32::Foundation::BOOL(muted as i32), None);
    }
}
```

The `windows::Win32::Foundation::BOOL` is already in scope project-wide. `SetMasterVolumeLevelScalar` and `SetMute` are the correct `IAudioEndpointVolume` methods (the second arg is an event context GUID pointer, `None` is fine).

- [ ] **Step 3: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 30 tests pass. (`set_volume`/`set_mute` may show a dead-code warning until Task 5 wires them — that's fine, the build does not deny warnings.)

- [ ] **Step 4: Commit**

```bash
git add src/status.rs
git commit -m "feat(status): add set_volume and set_mute endpoint controls"
git push origin main
```

---

## Task 2: Power action helpers + `CMD_*` constants

The launcher's action menu needs Sleep / Restart / Shut down / Lock. Use the existing `ShellExecuteW` + `rundll32`/shutdown.exe pattern (no new Cargo features, no privilege code).

**Files:**
- Modify: `src/app.rs` (new `power_action` helper + `CMD_*` constants + `handle_command` arms)

- [ ] **Step 1: Add the power-action helper** near the other free helpers (e.g. after `open_start_menu` at `src/app.rs:3388`). This is a free function:

```rust
/// Best-effort power action via the shell. Uses shutdown.exe / rundll32 so we
/// avoid linking Win32_System_Shutdown and the SE_SHUTDOWN_NAME privilege
/// dance — these verbs prompt or act as the current user.
fn power_action(action: PowerAction) {
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;
    let (file, args): (windows::core::HSTRING, windows::core::HSTRING) = match action {
        PowerAction::Sleep => (
            windows::core::HSTRING::from("rundll32.exe"),
            windows::core::HSTRING::from("powrprof.dll,SetSuspendState 0,1,0"),
        ),
        PowerAction::Restart => (
            windows::core::HSTRING::from("shutdown.exe"),
            windows::core::HSTRING::from("/r /t 0"),
        ),
        PowerAction::Shutdown => (
            windows::core::HSTRING::from("shutdown.exe"),
            windows::core::HSTRING::from("/s /t 0"),
        ),
        PowerAction::Lock => (
            windows::core::HSTRING::from("rundll32.exe"),
            windows::core::HSTRING::from("user32.dll,LockWorkStation"),
        ),
    };
    unsafe {
        let _ = ShellExecuteW(None, w!("open"), &file, &args, None, SW_SHOW);
    }
}

#[derive(Clone, Copy)]
enum PowerAction {
    Sleep,
    Restart,
    Shutdown,
    Lock,
}
```

(`ShellExecuteW` and `w!` are already imported in app.rs.)

- [ ] **Step 2: Add `CMD_*` constants** after `CMD_DEBUG_POPOVER` (`src/app.rs:112`):

```rust
const CMD_LAUNCHER_ITEM: usize = 310;
const CMD_LAUNCHER_ACTION: usize = 311;
const CMD_VOLUME_SET: usize = 312;
const CMD_MUTE_TOGGLE: usize = 313;
const CMD_WIFI_TOGGLE: usize = 314;
const CMD_POWER_SLEEP: usize = 315;
const CMD_POWER_RESTART: usize = 316;
const CMD_POWER_SHUTDOWN: usize = 317;
const CMD_POWER_LOCK: usize = 318;
```

(We encode the action/item index in the click handler state rather than the command id — see Task 4 — so these constants are mostly sentinel categories. `CMD_LAUNCHER_ACTION`/`CMD_LAUNCHER_ITEM` carry an index via `App` state, not the WPARAM.)

- [ ] **Step 3: Add `handle_command` arms** for the power actions (find `handle_command` at `src/app.rs:2511`, add inside the `match command {`):

```rust
            CMD_POWER_SLEEP => power_action(PowerAction::Sleep),
            CMD_POWER_RESTART => power_action(PowerAction::Restart),
            CMD_POWER_SHUTDOWN => power_action(PowerAction::Shutdown),
            CMD_POWER_LOCK => power_action(PowerAction::Lock),
```

(The volume-set, mute-toggle, wifi-toggle, launcher-item arms are added in later tasks once their state exists.)

- [ ] **Step 4: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 30 tests pass. Dead-code warnings on `power_action`/`PowerAction` until Task 4 wires them; acceptable.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): add power-action helpers (sleep/restart/shutdown/lock)"
git push origin main
```

---

## Task 3: Launcher geometry + hit-test (pure, TDD)

The launcher layout: a header (actions row), then a scrollable pinned-app list. Make the geometry a pure function so it's unit-testable without a GPU.

**Files:**
- Modify: `src/render.rs` (add `LauncherLayout`, `launcher_geometry`, `launcher_hit_test`)

- [ ] **Step 1: Define the layout types and pure geometry function** in `src/render.rs`, near `top_bar_geometry` (`src/render.rs:1709`). Place after `top_bar_hit_test`:

```rust
/// Layout of the launcher popover. Coordinates are in DIPs relative to the
/// popover window's top-left.
#[derive(Debug, Clone, PartialEq)]
pub struct LauncherLayout {
    pub width: f32,
    pub height: f32,
    /// One entry per quick action (Start, Run, Explorer, Sleep, Restart,
    /// Shutdown, Lock). Order is fixed.
    pub actions: Vec<D2D_RECT_F>,
    /// One entry per visible app row. Indices are into the *visible window*
    /// of the full list, i.e. shifted by the scroll offset.
    pub apps: Vec<D2D_RECT_F>,
}

const LAUNCHER_PAD: f32 = 12.0;
const LAUNCHER_ROW_H: f32 = 34.0;
const LAUNCHER_ACTION_H: f32 = 36.0;
const LAUNCHER_HEADER_GAP: f32 = 8.0;
pub const LAUNCHER_MAX_VISIBLE_ROWS: usize = 10;

/// Compute the launcher layout for a popover of `width`×`height` DIPs with
/// `action_count` quick actions and `app_visible` app rows currently drawn.
pub fn launcher_geometry(
    width: f32,
    height: f32,
    action_count: usize,
    app_visible: usize,
) -> LauncherLayout {
    let mut y = LAUNCHER_PAD;
    let row_w = width - LAUNCHER_PAD * 2.0;
    let actions: Vec<D2D_RECT_F> = (0..action_count)
        .map(|i| {
            let rect = D2D_RECT_F {
                left: LAUNCHER_PAD,
                top: y,
                right: LAUNCHER_PAD + row_w,
                bottom: y + LAUNCHER_ACTION_H,
            };
            y += LAUNCHER_ACTION_H;
            rect
        })
        .collect();
    y += LAUNCHER_HEADER_GAP;
    let apps: Vec<D2D_RECT_F> = (0..app_visible)
        .map(|_| {
            let rect = D2D_RECT_F {
                left: LAUNCHER_PAD,
                top: y,
                right: LAUNCHER_PAD + row_w,
                bottom: y + LAUNCHER_ROW_H,
            };
            y += LAUNCHER_ROW_H;
            rect
        })
        .collect();
    LauncherLayout {
        width,
        height,
        actions,
        apps,
    }
}

/// What a point in the launcher hits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LauncherHit {
    Action(usize),
    App(usize),
    None,
}

pub fn launcher_hit_test(layout: &LauncherLayout, x: f32, y: f32) -> LauncherHit {
    if let Some(i) = layout
        .actions
        .iter()
        .position(|r| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
    {
        return LauncherHit::Action(i);
    }
    if let Some(i) = layout
        .apps
        .iter()
        .position(|r| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
    {
        return LauncherHit::App(i);
    }
    LauncherHit::None
}

/// Given a total app count and visible-row capacity, the popover height needed.
pub fn launcher_height(action_count: usize, app_total: usize) -> f32 {
    let visible = app_total.min(LAUNCHER_MAX_VISIBLE_ROWS);
    LAUNCHER_PAD
        + action_count as f32 * LAUNCHER_ACTION_H
        + LAUNCHER_HEADER_GAP
        + visible as f32 * LAUNCHER_ROW_H
        + LAUNCHER_PAD
}

pub const LAUNCHER_WIDTH: f32 = 260.0;
```

- [ ] **Step 2: Write tests** in the existing `#[cfg(test)] mod tests` in `src/render.rs` (find it near the end of the file). Add:

```rust
    #[test]
    fn launcher_geometry_stacks_actions_then_apps_with_gap() {
        let layout = launcher_geometry(260.0, 400.0, 3, 2);
        assert_eq!(layout.actions.len(), 3);
        assert_eq!(layout.apps.len(), 2);
        // Actions start at top padding.
        assert_eq!(layout.actions[0].top, 12.0);
        // Actions are contiguous.
        assert_eq!(layout.actions[1].top, 12.0 + 36.0);
        assert_eq!(layout.actions[2].top, 12.0 + 72.0);
        // Gap between last action and first app row.
        assert_eq!(layout.apps[0].top, 12.0 + 3.0 * 36.0 + 8.0);
        // App rows are contiguous and use the row height.
        assert_eq!(layout.apps[1].top, layout.apps[0].top + 34.0);
    }

    #[test]
    fn launcher_hit_test_distinguishes_actions_and_apps() {
        let layout = launcher_geometry(260.0, 400.0, 2, 3);
        // Center of action 0.
        assert_eq!(
            launcher_hit_test(&layout, 130.0, 12.0 + 18.0),
            LauncherHit::Action(0)
        );
        // Center of app 0 (after gap).
        let app0_center_y = 12.0 + 2.0 * 36.0 + 8.0 + 17.0;
        assert_eq!(
            launcher_hit_test(&layout, 130.0, app0_center_y),
            LauncherHit::App(0)
        );
        // A point in the gap between actions and apps misses both.
        let gap_y = 12.0 + 2.0 * 36.0 + 4.0;
        assert_eq!(
            launcher_hit_test(&layout, 130.0, gap_y),
            LauncherHit::None
        );
    }

    #[test]
    fn launcher_height_caps_visible_rows() {
        // 20 apps but only LAUNCHER_MAX_VISIBLE_ROWS (10) shown.
        let h_full = launcher_height(3, 20);
        let h_capped = launcher_height(3, 10);
        assert!((h_full - h_capped).abs() < 0.001);
        // 5 apps is shorter.
        assert!(launcher_height(3, 5) < h_capped);
    }
```

- [ ] **Step 3: Run tests**

Run: `cargo test launcher`
Expected: 3 new tests PASS.

- [ ] **Step 4: Build + full test**

Run: `cargo build && cargo test`
Expected: compiles, 33 tests pass (30 + 3).

- [ ] **Step 5: Commit**

```bash
git add src/render.rs
git commit -m "feat(render): add pure launcher geometry + hit-test with tests"
git push origin main
```

---

## Task 4: Launcher state, enumeration, open/handlers

Wire the launcher: enumerate Start Menu `.lnk` files, store on `App`, open the popover, handle clicks on actions + app rows, handle scroll.

**Files:**
- Modify: `src/app.rs` (`WindowKind::Launcher`, `LauncherState`, `App::launcher`, `enumerate_start_menu`, `open_launcher`, click/scroll handlers, paint dispatch, Logo segment wiring)

- [ ] **Step 1: Add `WindowKind::Launcher`** to the enum (`src/app.rs:132-141`):

```rust
enum WindowKind {
    Top,
    Dock,
    Reserve,
    Preview,
    FolderStack,
    LaunchOverlay,
    DebugPopover,
    Launcher,
}
```

- [ ] **Step 2: Add the `LauncherState` struct + `App::launcher` field.** Place the struct near `FolderStack` (`src/app.rs:152`):

```rust
/// State for the open launcher popover. `apps` is the full enumerated list;
/// `scroll` is the index of the first visible row.
struct LauncherState {
    hwnd: HWND,
    owner: HWND,
    /// (label, .lnk path) for each pinned Start Menu app, alphabetical.
    apps: Vec<(String, std::path::PathBuf)>,
    /// Index of the first visible app row.
    scroll: usize,
    /// Currently hovered target (action index or visible app index).
    hover: Option<LauncherHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherHit {
    Action(usize),
    App(usize),
}
```

(We define a local `LauncherHit` here too because `render::LauncherHit` isn't imported yet and it's cleaner to convert at the boundary. Alternatively, `use crate::render::LauncherHit;` and reuse it — **do that instead**: add `LauncherHit` to the import list at `src/app.rs:6-9` and do NOT re-declare it. Use the render type.)

So: add `LauncherHit,` to the `use crate::render::{ ... }` block at `src/app.rs:6`, and drop the local enum above. `LauncherState.hover` becomes `Option<crate::render::LauncherHit>`.

Add the field to `App` (`src/app.rs:257`, after `active_popover`):

```rust
    launcher: Option<LauncherState>,
```

Initialize in `App::run`'s literal (`src/app.rs:308`, after `active_popover: None,`):

```rust
            launcher: None,
```

- [ ] **Step 3: Add `enumerate_start_menu`.** A free function near `open_start_menu` (`src/app.rs:3388`):

```rust
/// Enumerate pinned Start Menu apps (system + user), alphabetical, as
/// (label, .lnk path). Caps at 64 entries.
fn enumerate_start_menu() -> Vec<(String, std::path::PathBuf)> {
    let mut entries = Vec::new();
    let dirs: [Option<std::path::PathBuf>; 2] = [
        env::var_os("ProgramData")
            .map(|p| std::path::PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
        env::var_os("APPDATA")
            .map(|p| std::path::PathBuf::from(p).join(r"Microsoft\Windows\Start Menu\Programs")),
    ];
    for dir in dirs.into_iter().flatten() {
        collect_lnk(&dir, &mut entries);
    }
    entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    entries.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
    entries.truncate(64);
    entries
}

fn collect_lnk(dir: &std::path::Path, out: &mut Vec<(String, std::path::PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lnk(&path, out);
            continue;
        }
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
            if let Some(name) = path.file_stem() {
                out.push((name.to_string_lossy().into_owned(), path));
            }
        }
    }
}
```

(`env` is already imported in app.rs? Check: `std::env` — search. If `env` isn't imported, use `std::env::var_os` fully-qualified in the function body. Verify by reading the imports; safest is to write `std::env::var_os(...)` fully-qualified to avoid an import edit.)

- [ ] **Step 4: Add `open_launcher`.** Method near `toggle_debug_popover` (`src/app.rs:2917`):

```rust
    fn open_launcher(&mut self, owner: HWND) {
        let apps = enumerate_start_menu();
        let height = crate::render::launcher_height(LAUNCHER_ACTIONS.len(), apps.len());
        self.open_popover(owner, WindowKind::Launcher, crate::render::LAUNCHER_WIDTH as i32, height.round() as i32);
        // open_popover stored the Popover; now attach the launcher state.
        if let Some(popover) = &self.active_popover {
            self.launcher = Some(LauncherState {
                hwnd: popover.hwnd,
                owner: popover.owner,
                apps,
                scroll: 0,
                hover: None,
            });
            unsafe {
                let _ = InvalidateRect(Some(popover.hwnd), None, false);
            }
        }
    }
```

Define the fixed action list as a const near the top of `src/app.rs` (after the CMD constants):

```rust
/// Fixed launcher quick-action labels, in display order. Indices map to the
/// `actions` rects in `launcher_geometry`.
const LAUNCHER_ACTIONS: &[(&str, fn(&mut App))] = &[
    ("Start", |_app| open_start_menu()),
    ("Run…", |_app| send_windows_shortcut(b'R')),
    ("File Explorer", |_app| launch_path(std::path::Path::new("explorer.exe"))),
    ("Lock", |app| app.handle_command(CMD_POWER_LOCK)),
    ("Sleep", |app| app.handle_command(CMD_POWER_SLEEP)),
    ("Restart", |app| app.handle_command(CMD_POWER_RESTART)),
    ("Shut Down", |app| app.handle_command(CMD_POWER_SHUTDOWN)),
];
```

(`send_windows_shortcut`, `open_start_menu`, `launch_path` are all existing free functions in app.rs. `LAUNCHER_ACTIONS.len()` feeds `launcher_geometry`'s `action_count`.)

- [ ] **Step 5: Wire the Logo segment to open the launcher.** In `left_click` (`src/app.rs:2218-2238`), change the `Logo` arm (currently `TopBarSegment::Logo => self.handle_command(CMD_START_MENU)`) to:

```rust
            TopBarSegment::Logo => {
                self.open_launcher(hwnd);
            }
```

- [ ] **Step 6: Add launcher click + scroll + mouse-move handling.** Add these methods near `folder_stack_mouse_move` (`src/app.rs:2808+`):

```rust
    fn launcher_click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let x = x as f32 / scale;
        let y = y as f32 / scale;
        let Some(launcher) = self.launcher.as_ref() else {
            return;
        };
        let visible = launcher.apps.len().min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
        let layout = crate::render::launcher_geometry(
            crate::render::LAUNCHER_WIDTH,
            crate::render::launcher_height(LAUNCHER_ACTIONS.len(), launcher.apps.len()),
            LAUNCHER_ACTIONS.len(),
            visible,
        );
        match crate::render::launcher_hit_test(&layout, x, y) {
            crate::render::LauncherHit::Action(i) => {
                let app_ptr = self as *mut App;
                if let Some((_, action)) = LAUNCHER_ACTIONS.get(i) {
                    // SAFETY: the function pointer only touches `self` via the
                    // passed &mut, and we hold no other borrow at this point
                    // (launcher was cloned-out above). This mirrors how the
                    // existing menu commands call into self.
                    unsafe { action(&mut *app_ptr) };
                }
                self.close_popover();
            }
            crate::render::LauncherHit::App(visible_i) => {
                let path = launcher
                    .apps
                    .get(launcher.scroll + visible_i)
                    .map(|(_, p)| p.clone());
                self.close_popover();
                if let Some(path) = path {
                    launch_path(&path);
                }
            }
            crate::render::LauncherHit::None => {}
        }
    }

    fn launcher_mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let x = x as f32 / scale;
        let y = y as f32 / scale;
        let Some(launcher) = self.launcher.as_mut() else { return };
        let visible = launcher.apps.len().min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
        let layout = crate::render::launcher_geometry(
            crate::render::LAUNCHER_WIDTH,
            crate::render::launcher_height(LAUNCHER_ACTIONS.len(), launcher.apps.len()),
            LAUNCHER_ACTIONS.len(),
            visible,
        );
        let new_hover = crate::render::launcher_hit_test(&layout, x, y);
        if new_hover != launcher.hover {
            launcher.hover = Some(new_hover);
            unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
        }
    }

    fn launcher_scroll(&mut self, hwnd: HWND, delta: i32) {
        let Some(launcher) = self.launcher.as_mut() else { return };
        let visible = crate::render::LAUNCHER_MAX_VISIBLE_ROWS;
        if launcher.apps.len() <= visible {
            return;
        }
        // Wheel notches: positive = up. Step 3 rows per notch.
        let step = 3;
        let new_scroll = if delta > 0 {
            launcher.scroll.saturating_sub(step)
        } else {
            (launcher.scroll + step).min(launcher.apps.len().saturating_sub(visible))
        };
        if new_scroll != launcher.scroll {
            launcher.scroll = new_scroll;
            unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
        }
    }
```

- [ ] **Step 7: Dispatch launcher messages in `window_proc`.** The launcher window needs to receive `WM_LBUTTONDOWN`, `WM_MOUSEMOVE`, `WM_MOUSEWHEEL`. Add to the top of `WM_LBUTTONDOWN` arm (`src/app.rs:3065`) — after the existing DebugPopover check, before `mouse_down`:

```rust
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let pos = lparam.0;
                let x = (pos as i16) as i32;
                let y = ((pos >> 16) as i16) as i32;
                with_app(|app| app.launcher_click(hwnd, x, y));
                return LRESULT(0);
            }
```

Add a `WM_MOUSEMOVE` launcher branch: inside the existing `WM_MOUSEMOVE => { ... }` arm (`src/app.rs:3030` region — read it first), prepend a launcher check before the existing `mouse_move` call:

```rust
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                with_app(|app| app.launcher_mouse_move(hwnd, x, y));
                return LRESULT(0);
            }
```

Add a `WM_MOUSEWHEEL` arm (new message). First check `WM_MOUSEWHEEL` is imported — search the import block; if missing, add it to the `WindowsAndMessaging::{ ... }` list. Add the arm near the other mouse arms:

```rust
        WM_MOUSEWHEEL => {
            let is_launcher = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::Launcher)
            });
            if is_launcher {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                with_app(|app| app.launcher_scroll(hwnd, delta));
                return LRESULT(0);
            }
        }
```

(`WM_MOUSEWHEEL`'s high word of wParam is the wheel delta as a signed short; `HIWORD`.)

- [ ] **Step 8: Make the launcher window client-hittable** like the debug popover. In `WM_NCHITTEST` (`src/app.rs:3010`), extend the existing `DebugPopover` HTCLIENT guard to also cover `Launcher`:

```rust
            if with_app_value(false, |app| {
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover) | Some(WindowKind::Launcher)
                )
            }) {
                return LRESULT(HTCLIENT as isize);
            }
```

- [ ] **Step 9: `close_popover` must also clear launcher state.** Find `close_popover` (`src/app.rs:2851`) and add `self.launcher = None;` (also the launcher quick-popover state in Task 5) inside the `if let Some(popover) = ...` block, before/after `DestroyWindow`:

```rust
    fn close_popover(&mut self) {
        if let Some(popover) = self.active_popover.take() {
            self.launcher = None;
            self.kinds.remove(&(popover.hwnd.0 as isize));
            self.renderer.forget(popover.hwnd);
            unsafe {
                let _ = DestroyWindow(popover.hwnd);
                let _ = InvalidateRect(Some(popover.owner), None, false);
            }
        }
    }
```

- [ ] **Step 10: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 33 tests pass. Dead-code warnings on `launcher_click`/`launcher_mouse_move`/`launcher_scroll` until they're called from window_proc — but they ARE called in Step 7, so no warnings expected. There may be a warning on unused `WM_MOUSEWHEEL` if import is added but not used — confirm it's used.

- [ ] **Step 11: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): add launcher popover (start-menu app list + quick actions)"
git push origin main
```

---

## Task 5: `paint_launcher`

Render the launcher panel: translucent rounded background, action rows, app rows with icons + labels, hover highlight, scroll affordance.

**Files:**
- Modify: `src/render.rs` (`paint_launcher`)

- [ ] **Step 1: Add `paint_launcher`** after `paint_debug_popover` (`src/render.rs:1237`). It takes the full state needed to draw. Signature:

```rust
    /// Render the launcher popover. `action_labels` are the quick-action row
    /// labels; `apps` is the visible window of (label, icon path) tuples
    /// (already scroll-shifted); `hover` highlights one target.
    pub fn paint_launcher(
        &mut self,
        hwnd: HWND,
        action_labels: &[&str],
        apps: &[(String, std::path::PathBuf)],
        hover: Option<LauncherHit>,
    ) -> Result<()> {
        if self.handle_device_loss_if_needed() {
            return Ok(());
        }
        let body = self.body.clone();
        let small = self.small.clone();
        let wic = self.wic.clone();
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        let layout = launcher_geometry(size.width, size.height, action_labels.len(), apps.len());
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0x0c, 0x0f, 0x14, 0.92)));

            // Hover highlight + label for each action row.
            for (i, label) in action_labels.iter().enumerate() {
                if let Some(rect) = layout.actions.get(i) {
                    if hover == Some(LauncherHit::Action(i)) {
                        surface.context.FillRoundedRectangle(
                            &rounded_rect(*rect, 6.0),
                            &surface.bar_pill_fill,
                        );
                    }
                    let text: Vec<u16> = label.encode_utf16().collect();
                    body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                    surface.context.DrawText(
                        &text,
                        &body,
                        &D2D_RECT_F {
                            left: rect.left + 12.0,
                            ..*rect
                        },
                        &surface.foreground,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            // App rows: icon + label.
            for (i, (label, icon_path)) in apps.iter().enumerate() {
                if let Some(rect) = layout.apps.get(i) {
                    if hover == Some(LauncherHit::App(i)) {
                        surface.context.FillRoundedRectangle(
                            &rounded_rect(*rect, 6.0),
                            &surface.bar_pill_fill,
                        );
                    }
                    // Icon (cached per-surface, same pattern as folder_stack).
                    let icon_rect = D2D_RECT_F {
                        left: rect.left + 4.0,
                        top: rect.top + 3.0,
                        right: rect.left + 4.0 + 28.0,
                        bottom: rect.top + 3.0 + 28.0,
                    };
                    if !surface.icons.contains_key(icon_path)
                        && let Some(bitmap) = load_icon_bitmap(&wic, &surface.context, icon_path)
                    {
                        surface.icons.insert(icon_path.clone(), bitmap);
                    }
                    if let Some(bitmap) = surface.icons.get(icon_path) {
                        surface.context.DrawBitmap(
                            bitmap,
                            Some(&icon_rect),
                            1.0,
                            D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                            None,
                            None,
                        );
                    }
                    let text: Vec<u16> = label.encode_utf16().collect();
                    small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
                    surface.context.DrawText(
                        &text,
                        &small,
                        &D2D_RECT_F {
                            left: icon_rect.right + 8.0,
                            top: rect.top,
                            right: rect.right,
                            bottom: rect.bottom,
                        },
                        &surface.foreground,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            let action = present(surface);
            if matches!(action, PresentAction::RecreateAll) {
                self.device_lost = true;
            }
        }
        Ok(())
    }
```

**Note on borrows:** `surface.icons.contains_key`/`.get` borrow `surface` immutably while we also hold `surface.context` mutably via `DrawBitmap`. This is the SAME pattern `paint_folder_stack` uses (it works because `load_icon_bitmap` returns the bitmap into a temporary before insert, and the immutable `.get` for DrawBitmap happens in a separate statement). If the borrow checker complains, follow exactly how `paint_folder_stack` (`src/render.rs:1123-1137`) sequences the contains→load→insert→get→DrawBitmap calls.

**Helpers needed:** `rounded_rect(rect, radius) -> D2D1_ROUNDED_RECT` and the `surface.bar_pill_fill` brush. Verify `rounded_rect` exists (search render.rs); `paint_folder_stack`/`paint_top_bar` use `fill_pill` and `D2D1_ROUNDED_RECT`. If `rounded_rect` doesn't exist as a helper, construct `D2D1_ROUNDED_RECT { rect: *rect, radiusX: 6.0, radiusY: 6.0 }` inline and pass `&D2D1_ROUNDED_RECT{...}` to `FillRoundedRectangle`. Read `paint_folder_stack` to see the exact idiom used for rounded fills and copy it.

- [ ] **Step 2: Dispatch `Launcher` in `paint()`.** In the `match self.kinds.get(...)` at `src/app.rs:1169`, add (after the DebugPopover arm):

```rust
            Some(WindowKind::Launcher) => {
                let Some(launcher) = self.launcher.as_ref() else {
                    return;
                };
                let visible = launcher.apps.len().min(crate::render::LAUNCHER_MAX_VISIBLE_ROWS);
                let start = launcher.scroll;
                let apps_window: Vec<(String, std::path::PathBuf)> = launcher
                    .apps
                    .iter()
                    .skip(start)
                    .take(visible)
                    .cloned()
                    .collect();
                let action_labels: Vec<&str> =
                    LAUNCHER_ACTIONS.iter().map(|(label, _)| *label).collect();
                self.renderer.paint_launcher(
                    hwnd,
                    &action_labels,
                    &apps_window,
                    launcher.hover,
                )
            }
```

- [ ] **Step 3: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 33 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/render.rs src/app.rs
git commit -m "feat(render): paint launcher popover with actions, app icons, hover"
git push origin main
```

---

## Task 6: Quick popover geometry + state + open

The volume / Wi-Fi / battery quick popovers. Each is small. Volume gets a slider + mute button; Wi-Fi and Battery get a status line + deep-link button.

**Files:**
- Modify: `src/render.rs` (`QuickKind`, `quick_popover_geometry`, hit-test), `src/app.rs` (`WindowKind::QuickPopover`, `QuickPopoverState`, `App::quick_popover`, `open_quick_popover`, segment wiring)

- [ ] **Step 1: Add quick-popover layout in `src/render.rs`** near `launcher_geometry`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickKind {
    Volume,
    Wifi,
    Battery,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuickLayout {
    pub width: f32,
    pub height: f32,
    /// Mute toggle button rect (volume only).
    pub mute_button: Option<D2D_RECT_F>,
    /// Volume slider track rect (volume only). Horizontal.
    pub slider: Option<D2D_RECT_F>,
    /// Primary deep-link button rect.
    pub button: D2D_RECT_F,
}

pub const QUICK_WIDTH: f32 = 220.0;
pub const QUICK_HEIGHT: f32 = 110.0;
const QUICK_PAD: f32 = 14.0;
const QUICK_BUTTON_H: f32 = 32.0;

pub fn quick_popover_geometry(kind: QuickKind, scale: f32) -> QuickLayout {
    let width = QUICK_WIDTH;
    let height = QUICK_HEIGHT;
    let mut y = QUICK_PAD;
    let row_w = width - QUICK_PAD * 2.0;
    let (mute_button, slider) = if matches!(kind, QuickKind::Volume) {
        let slider = D2D_RECT_F {
            left: QUICK_PAD,
            top: y,
            right: QUICK_PAD + row_w - 40.0,
            bottom: y + 20.0,
        };
        let mute = D2D_RECT_F {
            left: QUICK_PAD + row_w - 32.0,
            top: y - 6.0,
            right: QUICK_PAD + row_w,
            bottom: y + 26.0,
        };
        y += 36.0;
        (Some(mute), Some(slider))
    } else {
        (None, None)
    };
    // Spacer to push the button toward the bottom.
    y = height - QUICK_PAD - QUICK_BUTTON_H;
    let button = D2D_RECT_F {
        left: QUICK_PAD,
        top: y,
        right: QUICK_PAD + row_w,
        bottom: y + QUICK_BUTTON_H,
    };
    let _ = scale;
    QuickLayout {
        width,
        height,
        mute_button,
        slider,
        button,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickHit {
    MuteButton,
    Slider,
    Button,
    None,
}

pub fn quick_hit_test(layout: &QuickLayout, x: f32, y: f32) -> QuickHit {
    if let Some(r) = layout.mute_button
        && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
    {
        return QuickHit::MuteButton;
    }
    if let Some(r) = layout.slider
        && x >= r.left && x <= r.right && y >= r.top - 6.0 && y <= r.bottom + 6.0
    {
        return QuickHit::Slider;
    }
    let r = layout.button;
    if x >= r.left && x <= r.right && y >= r.top && y <= r.bottom {
        return QuickHit::Button;
    }
    QuickHit::None
}
```

Add tests in `src/render.rs`'s test module:

```rust
    #[test]
    fn quick_volume_layout_has_slider_and_mute() {
        let layout = quick_popover_geometry(QuickKind::Volume, 1.0);
        assert!(layout.slider.is_some());
        assert!(layout.mute_button.is_some());
        // Slider is to the left of the mute button.
        let s = layout.slider.unwrap();
        let m = layout.mute_button.unwrap();
        assert!(s.right <= m.left);
    }

    #[test]
    fn quick_wifi_layout_has_no_slider() {
        let layout = quick_popover_geometry(QuickKind::Wifi, 1.0);
        assert!(layout.slider.is_none());
        assert!(layout.mute_button.is_none());
    }

    #[test]
    fn quick_hit_test_finds_button() {
        let layout = quick_popover_geometry(QuickKind::Battery, 1.0);
        let cy = (layout.button.top + layout.button.bottom) / 2.0;
        assert_eq!(quick_hit_test(&layout, QUICK_WIDTH / 2.0, cy), QuickHit::Button);
    }
```

- [ ] **Step 2: Add `WindowKind::QuickPopover` + state + field in `src/app.rs`.**

Add `QuickPopover,` to the `WindowKind` enum. Add `QuickKind` to the render import at `src/app.rs:6`.

```rust
struct QuickPopoverState {
    hwnd: HWND,
    owner: HWND,
    kind: crate::render::QuickKind,
    /// True while the user is dragging the volume slider.
    dragging: bool,
}
```

Add field `quick_popover: Option<QuickPopoverState>` to `App` (after `launcher`), init `None`.

- [ ] **Step 3: Add `open_quick_popover`** near `open_launcher`:

```rust
    fn open_quick_popover(&mut self, owner: HWND, kind: crate::render::QuickKind) {
        let w = crate::render::QUICK_WIDTH as i32;
        let h = crate::render::QUICK_HEIGHT as i32;
        self.open_popover(owner, WindowKind::QuickPopover, w, h);
        if let Some(popover) = &self.active_popover {
            self.quick_popover = Some(QuickPopoverState {
                hwnd: popover.hwnd,
                owner: popover.owner,
                kind,
                dragging: false,
            });
            unsafe {
                let _ = InvalidateRect(Some(popover.hwnd), None, false);
            }
        }
    }
```

Clear `quick_popover` in `close_popover` alongside `self.launcher = None;`:
```rust
            self.launcher = None;
            self.quick_popover = None;
```

- [ ] **Step 4: Wire segments.** In `left_click` (`src/app.rs:2232-2234`), replace the combined `Network | Volume | Battery => self.handle_command(CMD_QUICK_SETTINGS)` arm with per-segment opens:

```rust
            TopBarSegment::Network => self.open_quick_popover(hwnd, crate::render::QuickKind::Wifi),
            TopBarSegment::Volume => self.open_quick_popover(hwnd, crate::render::QuickKind::Volume),
            TopBarSegment::Battery => self.open_quick_popover(hwnd, crate::render::QuickKind::Battery),
```

- [ ] **Step 5: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 36 tests pass (30 + 3 launcher + 3 quick).

- [ ] **Step 6: Commit**

```bash
git add src/render.rs src/app.rs
git commit -m "feat(app): add quick-popover geometry, state, and segment wiring"
git push origin main
```

---

## Task 7: Quick popover paint + interactions

Render the quick popover and handle clicks/drags (volume slider, mute, deep-link buttons).

**Files:**
- Modify: `src/render.rs` (`paint_quick_popover`), `src/app.rs` (click/drag handlers, window_proc dispatch, paint dispatch)

- [ ] **Step 1: Add `paint_quick_popover`** in `src/render.rs` after `paint_launcher`:

```rust
    /// Render a quick popover. `volume_pct`/`muted`/`online`/`charging`/
    /// `battery_pct` describe current status; `status_text` is the headline
    /// (e.g. "Wi-Fi: On", "Volume: 45%", "Battery: 80% (charging)").
    pub fn paint_quick_popover(
        &mut self,
        hwnd: HWND,
        kind: QuickKind,
        layout: &QuickLayout,
        status_text: &str,
        button_label: &str,
        volume_pct: Option<u8>,
        muted: bool,
        hit: Option<QuickHit>,
    ) -> Result<()> {
        if self.handle_device_loss_if_needed() {
            return Ok(());
        }
        let body = self.body.clone();
        let small = self.small.clone();
        let surface = self.surface(hwnd)?;
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0x0c, 0x0f, 0x14, 0.92)));

            // Headline status text.
            let status: Vec<u16> = status_text.encode_utf16().collect();
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            surface.context.DrawText(
                &status,
                &body,
                &D2D_RECT_F {
                    left: QUICK_PAD,
                    top: QUICK_PAD,
                    right: layout.width - QUICK_PAD,
                    bottom: QUICK_PAD + 28.0,
                },
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            if matches!(kind, QuickKind::Volume)
                && let (Some(slider), Some(pct)) = (layout.slider, volume_pct)
            {
                // Track background.
                surface.context.FillRectangle(&slider, &surface.bar_dim);
                // Filled portion.
                let fill_w = slider.width() * (pct as f32 / 100.0);
                surface.context.FillRectangle(
                    &D2D_RECT_F {
                        left: slider.left,
                        top: slider.top,
                        right: slider.left + fill_w,
                        bottom: slider.bottom,
                    },
                    &surface.foreground,
                );
                // Mute button highlight + glyph (an "M" / "X" label).
                if let Some(mr) = layout.mute_button {
                    if hit == Some(QuickHit::MuteButton) {
                        surface.context.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT { rect: mr, radiusX: 6.0, radiusY: 6.0 },
                            &surface.bar_pill_fill,
                        );
                    }
                    let label = if muted { "🔇" } else { "🔊" };
                    let t: Vec<u16> = label.encode_utf16().collect();
                    small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                    surface.context.DrawText(
                        &t,
                        &small,
                        &mr,
                        &surface.foreground,
                        D2D1_DRAW_TEXT_OPTIONS_CLIP,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }

            // Deep-link button.
            let b = layout.button;
            if hit == Some(QuickHit::Button) {
                surface.context.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT { rect: b, radiusX: 6.0, radiusY: 6.0 },
                    &surface.bar_pill_fill,
                );
            }
            let btn: Vec<u16> = button_label.encode_utf16().collect();
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            surface.context.DrawText(
                &btn,
                &small,
                &b,
                &surface.foreground,
                DWRITE_TEXT_ALIGNMENT_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            let action = present(surface);
            if matches!(action, PresentAction::RecreateAll) {
                self.device_lost = true;
            }
        }
        Ok(())
    }
```

(Note: `DWRITE_TEXT_ALIGNMENT_CLIP` is not a real value — use `DWRITE_TEXT_ALIGNMENT_CENTER` or `DWRITE_TEXT_ALIGNMENT_LEADING`. Fix that to `DWRITE_TEXT_ALIGNMENT_CENTER` before finalizing. Also `slider.width()` requires `rect_width` helper or inline `slider.right - slider.left` — check; `D2D_RECT_F` doesn't have a `.width()` method unless a helper trait is in scope. In render.rs there's a `rect_width`-style helper? Search; `paint_folder_stack` uses raw arithmetic. Use `slider.right - slider.left`.)

- [ ] **Step 2: Add quick-popover click + drag handlers in `src/app.rs`** near `launcher_click`:

```rust
    fn quick_popover_click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let scale = window_scale(hwnd);
        let xf = x as f32 / scale;
        let yf = y as f32 / scale;
        let Some(qp) = self.quick_popover.as_ref() else { return };
        let layout = crate::render::quick_popover_geometry(qp.kind, scale);
        match (qp.kind, crate::render::quick_hit_test(&layout, xf, yf)) {
            (_, crate::render::QuickHit::Button) => {
                let cmd = match qp.kind {
                    crate::render::QuickKind::Volume => CMD_SOUND_SETTINGS,
                    crate::render::QuickKind::Wifi => CMD_NETWORK_SETTINGS,
                    crate::render::QuickKind::Battery => CMD_POWER_SETTINGS,
                };
                self.close_popover();
                self.handle_command(cmd);
            }
            (crate::render::QuickKind::Volume, crate::render::QuickHit::MuteButton) => {
                let now_muted = !self.system_status.muted;
                crate::status::set_mute(now_muted);
                self.system_status.muted = now_muted;
                unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
            }
            (crate::render::QuickKind::Volume, crate::render::QuickHit::Slider) => {
                self.set_volume_from_x(hwnd, x);
            }
            _ => {}
        }
    }

    fn set_volume_from_x(&mut self, hwnd: HWND, screen_x: i32) {
        let scale = window_scale(hwnd);
        let x = screen_x as f32 / scale;
        let layout = crate::render::quick_popover_geometry(crate::render::QuickKind::Volume, scale);
        let Some(slider) = layout.slider else { return };
        let frac = ((x - slider.left) / (slider.right - slider.left)).clamp(0.0, 1.0);
        let level = (frac * 100.0).round() as u8;
        crate::status::set_volume(level);
        self.system_status.volume_percent = Some(level);
        unsafe { let _ = InvalidateRect(Some(hwnd), None, false); }
    }

    fn quick_popover_drag(&mut self, hwnd: HWND, x: i32) {
        let is_volume = self
            .quick_popover
            .as_ref()
            .is_some_and(|qp| matches!(qp.kind, crate::render::QuickKind::Volume));
        if is_volume {
            self.set_volume_from_x(hwnd, x);
        }
    }
```

- [ ] **Step 3: Dispatch quick-popover messages in `window_proc`.** Extend the launcher checks. In `WM_LBUTTONDOWN` (`src/app.rs:3065`), after the launcher block, add:

```rust
            let is_quick = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::QuickPopover)
            });
            if is_quick {
                let x = (lparam.0 as i16) as i32;
                let y = ((lparam.0 >> 16) as i16) as i32;
                // Mark dragging if the click was on the slider.
                with_app(|app| {
                    if let Some(qp) = app.quick_popover.as_mut() {
                        let scale = window_scale(hwnd);
                        let layout = crate::render::quick_popover_geometry(qp.kind, scale);
                        if matches!(crate::render::quick_hit_test(&layout, x as f32 / scale, y as f32 / scale), crate::render::QuickHit::Slider) {
                            qp.dragging = true;
                        }
                    }
                    app.quick_popover_click(hwnd, x, y);
                });
                return LRESULT(0);
            }
```

In `WM_LBUTTONUP`, add a quick-popover branch to clear dragging:

```rust
        WM_LBUTTONUP => {
            with_app(|app| {
                if app.quick_popover.as_mut().is_some_and(|qp| {
                    let _ = qp; qp.dragging = false; true
                }) {
                    // dragging cleared
                }
            });
            with_app(|app| app.mouse_up(hwnd));
            return LRESULT(0);
        }
```

(That's awkward — cleaner: `with_app(|app| { if let Some(qp) = app.quick_popover.as_mut() { qp.dragging = false; } });`. Use that.)

In `WM_MOUSEMOVE`, add a quick-popover drag branch (after the launcher block):

```rust
            let is_quick = with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::QuickPopover)
            });
            if is_quick {
                let dragging = with_app_value(false, |app| {
                    app.quick_popover.as_ref().is_some_and(|qp| qp.dragging)
                });
                if dragging {
                    let x = (lparam.0 as i16) as i32;
                    with_app(|app| app.quick_popover_drag(hwnd, x));
                }
                return LRESULT(0);
            }
```

Extend the `WM_NCHITTEST` HTCLIENT guard to include `QuickPopover`:

```rust
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover) | Some(WindowKind::Launcher) | Some(WindowKind::QuickPopover)
                )
```

- [ ] **Step 4: Dispatch paint** in `paint()` (`src/app.rs:1169`), after the Launcher arm:

```rust
            Some(WindowKind::QuickPopover) => {
                let Some(qp) = self.quick_popover.as_ref() else { return };
                let scale = window_scale(hwnd);
                let layout = crate::render::quick_popover_geometry(qp.kind, scale);
                let (status_text, button_label) = match qp.kind {
                    crate::render::QuickKind::Volume => {
                        let pct = self.system_status.volume_percent.unwrap_or(0);
                        let head = if self.system_status.muted { "Muted" } else { "Volume" };
                        (format!("{}: {}%", head, pct), "Sound settings")
                    }
                    crate::render::QuickKind::Wifi => {
                        let head = if self.system_status.network_online { "Wi-Fi: On" } else { "Wi-Fi: Off" };
                        (head.to_string(), "Network settings")
                    }
                    crate::render::QuickKind::Battery => {
                        let pct = self.system_status.battery_percent.unwrap_or(0);
                        let head = if self.system_status.charging {
                            format!("{}% (charging)", pct)
                        } else {
                            format!("Battery: {}%", pct)
                        };
                        (head, "Power & battery")
                    }
                };
                self.renderer.paint_quick_popover(
                    hwnd,
                    qp.kind,
                    &layout,
                    &status_text,
                    button_label,
                    self.system_status.volume_percent,
                    self.system_status.muted,
                    None,
                )
            }
```

- [ ] **Step 5: Build + test**

Run: `cargo build && cargo test`
Expected: compiles, 36 tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/render.rs src/app.rs
git commit -m "feat(app): paint quick popovers with volume slider, mute, deep-links"
git push origin main
```

---

## Task 8: Final integration + clippy + manual check

- [ ] **Step 1: Full build + test + clippy**

Run: `cargo build --release && cargo test && cargo clippy`
Expected: release builds, 36 tests pass, no NEW clippy warnings in Phase 2 code (pre-existing 6 in older code remain).

- [ ] **Step 2: Manual verification matrix**

Run `target\release\YumeDock.exe`:
1. Click the logo (left dot) → launcher popover opens with action rows + scrollable app list. Click an app → it launches; popover closes. Click an action (Start/Run/Lock) → works. Scroll wheel → list scrolls. Click outside / Esc → closes.
2. Click the volume segment → volume popover with slider + mute. Drag slider → volume changes audibly + topbar volume icon reflects it. Click mute → mutes. Click "Sound settings" → opens Settings.
3. Click the Wi-Fi segment → popover shows On/Off + "Network settings" deep-link.
4. Click the battery segment → popover shows percent/charging + "Power & battery" deep-link.
5. Only one popover open at a time (opening one closes another).
6. Sleep/wake still recovers (Phase 1 stability holds).

- [ ] **Step 3: Commit any fixes** found in manual testing.

- [ ] **Step 4: Update spec status**

Edit `docs/superpowers/specs/2026-07-15-stability-and-topbar-rework-design.md` status line to "Phase 2 built (manual verify pending). Phases 3–4 not started." Commit + push.

---

## Self-Review (completed)

**Spec coverage (Phase 2):**
- §2.1 launcher actions (Start/Run/Explorer/Sleep/Restart/Shutdown/Lock) → Task 2 + Task 4 LAUNCHER_ACTIONS. ✓
- §2.1 pinned Start Menu app list (scanned, alphabetical, scrollable, icon-extracted) → Task 4 enumerate_start_menu + Task 5 paint_launcher. ✓
- §2.1 launch via existing open_item/launch_path → Task 4 uses launch_path. ✓
- §2.2 volume slider (set) → Task 1 set_volume + Task 7 slider drag. ✓
- §2.2 volume mute toggle → Task 1 set_mute + Task 7 mute button. ✓
- §2.2 Wi-Fi status + deep-link fallback → Task 7 wifi popover. ✓
- §2.2 battery status + deep-link → Task 7 battery popover. ✓
- Popover infra reuse → all tasks use open_popover/close_popover. ✓

**Placeholder scan:** No TBD/TODO. The two flagged spots (`DWRITE_TEXT_ALIGNMENT_CLIP` typo, `slider.width()` method) are flagged inline in Task 7 Step 1 with the explicit fix. The awkward `WM_LBUTTONUP` block has a cleaner alternative stated.

**Type/signature consistency:**
- `LauncherHit` (render) reused in app.rs via import, not re-declared. ✓ (Task 4 Step 2)
- `QuickKind`/`QuickLayout`/`QuickHit` consistent across Tasks 6–7. ✓
- `paint_launcher(action_labels: &[&str], apps: &[(String, PathBuf)], hover: Option<LauncherHit>)` — call site in Task 5 Step 2 builds exactly these. ✓
- `paint_quick_popover(...)` signature matches the Task 7 Step 4 call site. ✓

**Known risks:**
- The function-pointer table `LAUNCHER_ACTIONS: &[(&str, fn(&mut App))]` requires raw-pointer `*mut App` in `launcher_click` because `LAUNCHER_ACTIONS.get(i)` borrows the static while we call into `self`. The `unsafe` block is documented; if the reviewer flags it, the alternative is a `match i { 0 => open_start_menu(), 1 => send_windows_shortcut(b'R'), ... }` — equally valid, slightly more verbose. Prefer the `match` form to avoid `unsafe`; **revision: replace the fn-pointer table with a plain `match` in `launcher_click`.** (See Task 4 Step 6 revision below.)

**Revision to Task 4 Step 6 (apply during implementation):** Instead of `LAUNCHER_ACTIONS` as a fn-pointer table + unsafe, define `LAUNCHER_ACTION_LABELS: &[&str] = &["Start", "Run…", "File Explorer", "Lock", "Sleep", "Restart", "Shut Down"];` (labels only, for geometry/paint) and in `launcher_click`'s `LauncherHit::Action(i)` arm use a safe `match`:
```rust
                crate::render::LauncherHit::Action(i) => {
                    match i {
                        0 => open_start_menu(),
                        1 => send_windows_shortcut(b'R'),
                        2 => launch_path(std::path::Path::new("explorer.exe")),
                        3 => power_action(PowerAction::Lock),
                        4 => power_action(PowerAction::Sleep),
                        5 => power_action(PowerAction::Restart),
                        6 => power_action(PowerAction::Shutdown),
                        _ => {}
                    }
                    self.close_popover();
                }
```
And `launcher_geometry`/`paint_launcher` use `LAUNCHER_ACTION_LABELS.len()` / the labels. This removes all `unsafe` from the launcher action dispatch.
