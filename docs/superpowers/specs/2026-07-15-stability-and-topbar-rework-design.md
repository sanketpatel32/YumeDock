# YumeDock: Stability Hardening + Topbar Rework

**Date:** 2026-07-15
**Status:** Phase 1 built & committed (manual GUI verification pending). Phases 2–4 not started.
**Scope:** One cycle, four phases delivered incrementally inside it.

## Goal

Two interlocking outcomes:

1. **Stability foundation.** Make the renderer *recover* from GPU device loss instead of silently breaking, remove abort-on-recoverable-failure footguns, and cut idle cost. Every popover added in the topbar rework inherits this behavior.
2. **Full macOS-style topbar.** A popover system plus four features: logo → launcher menu (actions + app list), Wi-Fi/volume/battery quick popovers, a Control Center panel, and an active-app window/app menu.

## Non-goals

- Real per-app File/Edit/View menu introspection (UI Automation / UIAccess). The active-app menu delivers window/app controls only. True menu introspection is a future phase.
- Redesigning the dock or its magnification/animation system.
- Changing config file format or breaking existing settings.
- Networking/battery *control* APIs beyond what Windows exposes cleanly; graceful fallback to deep-links where a toggle API is unavailable.

---

## Architecture

```
                    Stability foundation (Phase 1)
        device-loss recovery · panic-safety · idle-cost · logging
                                   │
                    ┌──────────────┴──────────────┐
                    │   Topbar popover system     │  (Phase 1)
                    │   one active_popover field  │
                    │   per-kind WindowKind       │
                    │   folder_stack-style window │
                    │   pure-function geometry    │
                    └──────────────┬──────────────┘
        ┌─────────────┬────────────┼────────────┬─────────────┐
   Launcher      Quick popovers   Control    Active-app
  (Phase 2)        (Phase 2)     Center       menu
                                 (Phase 3)   (Phase 4)
```

Each popover is its own small Direct2D window anchored under its trigger segment, reusing the exact pattern the dock's `folder_stack` already uses (`create_window` → register `WindowKind` → `paint_*` → click-outside-to-close). Device-loss recovery (Phase 1) covers them automatically since they go through the same `Renderer::surface` path.

---

## Phase 1 — Stability foundation + popover infrastructure

### 1.1 Device-loss recovery (core "rendering glitches" fix)

**Root cause.** `present()` / `present_immediate()` in `src/render.rs:1758` do `surface.swap_chain.Present(...).ok()?` — `.ok()` swallows the HRESULT that carries `DXGI_STATUS_OCCLUDED`, `DXGI_ERROR_DEVICE_REMOVED`, `DXGI_ERROR_DEVICE_HUNG`, `DXGI_ERROR_DEVICE_RESET`. `paint()` in `src/app.rs:1137` then `eprintln!`s the propagated error and moves on, but nothing recreates the device. After sleep / driver update / RDP reconnect / display change the device is permanently lost → frozen or garbled bar until restart. `resize()` (`src/render.rs:518`) similarly swallows `ResizeBuffers` failure.

**Fix.**

- Capture the `Present` HRESULT instead of `.ok()`. Classify it:
  - `DXGI_ERROR_DEVICE_REMOVED` / `DEVICE_HUNG` / `DEVICE_RESET` → set `Renderer::device_lost = true`, invalidate all surfaces.
  - `DXGI_STATUS_OCCLUDED` (minimized / occluded by RDP / secure desktop) → skip the present this frame, harmless.
  - OK → present as today.
- Detect the D2D-side equivalent: `EndDraw` returning `D2DERR_RECREATE_TARGET` also sets `device_lost`.
- Refactor device initialization in `Renderer::new` (`src/render.rs:276`–`347`) into a `recreate_device()` method (D3D device, DXGI factory, D2D device, dcomp device). Call it from `new` and from `handle_device_loss_if_needed()`.
- Add `Renderer::handle_device_loss_if_needed() -> bool` (returns "did I just recreate"). Called at the top of every `paint_*` method. If it recreated, return early this frame and `InvalidateRect` for the next.
- `resize()` propagates device-removed to the same flag instead of silently no-op'ing.

**New unit test.** A pure-Rust state machine `DeviceLossPolicy { fn observe(&mut self, hr: HRESULT) -> PresentAction }` returning `Present | SkipFrame | RecreateAll`, with table-driven tests over the HRESULTs above. This is the logic that currently lives invisibly inside `.ok()?`.

### 1.2 Panic-safety on `panic = "abort"`

`Cargo.toml:56` sets `panic = "abort"` for release. Replace aborting `.expect()` calls on recoverable FFI failures:

- `src/app.rs:2255`, `2367`, `2429` — `CreatePopupMenu().expect("menu")` → fallible: log via §1.4, return from the handler. Worst case is no menu this once, not a dead dock.
- Same treatment for any new menus the launcher/control-center features add.
- Audit `unsafe` blocks that `.unwrap()` FFI results in hot paths (`paint`, `mouse_move`, `window_proc`) and downgrade to logged-and-skip where the operation is not structurally guaranteed to succeed.

### 1.3 Idle cost (the "lag / high idle usage")

**Root cause.** Timer 12 (`src/app.rs:478`) runs `cache_foreground_for_genie()` every **250 ms on every topbar window**, probing `GetForegroundWindow` + `GetWindowRect` + periodic full-window bitmap captures, even when the dock is hidden and no transition is possible. Timer 1 (5 s) invalidates *all* top windows regardless of whether status changed, forcing full repaints.

**Fix.**

- Timer 12: single per-process poll on the **primary** topbar only (not one per monitor), interval raised to 1000 ms, with early-outs: skip when `dock.auto_hide && dock fully hidden && no animation pending && no window_open_animation`.
- `refresh_system_status` (`src/app.rs:662`): compare new status to `self.system_status`; `InvalidateRect` only when a field changed *or* the wall-clock minute rolled (so the clock still updates on the minute).
- Keep genie capture's existing near-caption fast-path (`minimum_age`) but make it not run at all while idle.

### 1.4 Logging

Introduce a minimal logger (no external dep): a `yume_warn!` / `yume_err!` macro writing to `%LOCALAPPDATA%\YumeDock\yumedock.log`, rotating/capped at 512 KB. Today `eprintln!` goes nowhere in a `windows_subsystem = "windows"` release build, so there is no way to distinguish transient from fatal. Existing `eprintln!` calls in paint/hot paths are converted to the appropriate level. Startup/teardown and watchdog keep stderr (still visible when launched from a console).

### 1.5 Topbar popover infrastructure

The load-bearing piece for Phases 2–4.

- **`WindowKind`** (`src/app.rs:131`): add `Launcher`, `QuickPopover`, `ControlCenter`, `AppMenu`.
- **`App` field:**
  ```rust
  active_popover: Option<Popover>,
  // Popover { hwnd: HWND, owner: HWND /* the top window */, kind: PopoverKind }
  ```
  A single field enforces the macOS invariant: only one topbar popover open at a time.
- **`open_popover()`** closes any existing popover (mirrors `close_folder_stack`, `src/app.rs:2778`), then creates the new window via the existing `create_window`, registers its `WindowKind`, and calls `configure_window_backdrop(hwnd, true, high_contrast)`. Anchoring reuses the `folder_stack` math (`src/app.rs:2727`–`2755`): topbar rect + cursor-x → `anchor_x`, clamped within monitor bounds.
- **Dismissal.** On any topbar/dock `WM_LBUTTONDOWN` outside the popover window, close it. Combined with the popover's own `WM_MOUSELEAVE → close` (matching `folder_stack` timer-10 dismiss, `src/app.rs:1221`, `2988`). Escape closes via translation in `window_proc`.
- **Rendering.** Each popover gets a `Renderer::paint_*` method (`paint_launcher`, `paint_quick_popover` with a `kind` arg, `paint_control_center`, `paint_app_menu`) following the exact structure of `paint_folder_stack` (`src/render.rs:910`).
- **Geometry & hit-testing.** Pure functions in `render.rs` (like `top_bar_geometry`, `src/render.rs:1553`) so they are unit-testable without a GPU. Hit-testing is `top_bar_hit_test`-style (`src/render.rs:1636`): pure functions over rects.
- **Segment wiring.** `mouse_up` on a topbar segment today calls `ShellExecute` on a settings URL. That stays as fallback, but new handlers open popovers instead. New `CMD_*` ids for in-popover actions.

---

## Phase 2 — Launcher + quick popovers

### 2.1 Logo → Launcher (`WindowKind::Launcher`)

- **Top section — actions:** Start (Win key), Settings (opens config file), Run… (Win+R), File Explorer, then Sleep / Restart / Shut down (existing `shell` helpers or `InitiateSystemShutdown`).
- **Bottom section — pinned apps:** scanned from `%APPDATA%\Microsoft\Windows\Start Menu\Programs` (`*.lnk`), alphabetized, icon-extracted via the existing `dock_icon_paths` machinery (`src/app.rs:3150`). Shown in a scrollable list (mouse-wheel scroll, ~10 visible, capped total e.g. 64). Click launches via the existing `open_item` (`src/app.rs:3252`).
- **Geometry:** `launcher_geometry(actions_len, apps_visible, scroll_offset)`.
- **Hit-testing:** `launcher_hit_test(geo, x, y) -> LauncherTarget { Action(usize), App(usize), None }`.

### 2.2 Quick popovers (`WindowKind::QuickPopover`, `kind: QuickKind { Wifi, Volume, Battery }`)

- **Volume:** slider bound to the existing `IAudioEndpointVolume` (`src/status.rs:30`) — now with *set* in addition to read. Adjust via click/drag; mute toggle button. Live refresh on timer 1.
- **Wi-Fi:** online/offline indicator + Turn Wi-Fi on/off button. Use the radio/WCM toggle API where available; otherwise fall back to the `ms-settings:network-wifi` deep-link (no regression from today's behavior, just inside a popover).
- **Battery:** percent + charging state + Power & battery settings deep-link button.

---

## Phase 3 — Control Center

`WindowKind::ControlCenter`. Panel with four tile groups:

- **Volume slider + brightness slider.** Brightness via WMI / monitor-DDC/CI where supported; graceful "unsupported" visual state on desktops without DDC/CI (slider disabled with a tooltip).
- **Connectivity tiles:** Wi-Fi / Bluetooth / Airplane mode toggle tiles. Same toggle-or-deeplink approach as §2.2.
- **Focus / DND tile:** Windows quiet hours / `ms-settings:quietmoments`.
- **Accessibility tile:** toggles reduce-motion (`config.behavior.reduce_motion`) + high contrast (already read; already triggers `rebuild_monitors`).

**Trigger.** A new right-side topbar segment (small CC glyph) opens it. Subject to the single-open-at-a-time rule. Opening CC closes any quick popover.

---

## Phase 4 — Active-app menu

`WindowKind::AppMenu`. Triggered by clicking the app-name segment.

- Header: foreground app name (already computed via `active_app_name`, `src/app.rs:3279`).
- **Window actions:** Minimize, Zoom/Restore, Close, New window (where the app supports multi-instance — reuses `activate_dock_item(..., new_instance=true)`, `src/app.rs:1967`).
- **Multi-window:** if the foreground app has >1 window in `self.windows` for its identity, a *Bring to front* list cycles them (reuses `cycle_index` machinery).
- **Scope note (non-goal reminder):** this is window/app controls, not the app's real File/Edit/View menus. True menu introspection is a future phase.

---

## Data flow

All popovers flow through the same path, so device-loss/idle/logging fixes apply uniformly:

```
topbar click  → App::mouse_up → open_popover(kind) → create_window + WindowKind
                                       │
            timer 1 / input → paint_* → Renderer::surface(hwnd)
                                       │   handle_device_loss_if_needed()
                                       EndDraw → present() [classified HRESULT]
                                       │
            outside click / Escape / new popover → close current popover
```

## Error handling

- **Device loss:** classified Present HRESULT → `device_lost` → recreate next paint (§1.1). No user-visible action; the bar just keeps working after sleep/RDP/driver change.
- **Popover creation failure:** fallible (§1.2); log + no-op, the segment click degrades to today's deep-link behavior.
- **Toggle API unavailable** (Wi-Fi/BT/brightness): popover shows the control disabled with a tooltip and a deep-link button. Never silently broken.
- **Watchdog / emergency hotkey:** unchanged — still the safety net for process-level crashes.

## Testing

- **Unit (pure logic, GPU-free):** `DeviceLossPolicy` state machine (§1.1); `launcher_geometry`/`launcher_hit_test`; control-center tile hit-testing; popover anchor/clamp math; status-change comparison that gates `refresh_system_status` invalidation. Target ≥ 6 new unit tests, joining the existing 26.
- **Manual verification matrix:** (a) sleep/wake → bar still renders; (b) `Win+Ctrl+Shift+B` (driver reset) → bar recovers; (c) RDP reconnect → bar recovers; (d) idle 60 s with dock hidden → low CPU/GPU via Task Manager; (e) each popover opens/closes and dismisses on outside click; (f) Control Center toggles actually change the setting; (g) `Ctrl+Alt+Shift+F12` still restores the taskbar.
- Existing `cargo test` (26 tests) must remain green; `cargo build --release` must succeed.

## File impact (indicative)

- `Cargo.toml` — no new deps (logging is hand-rolled, no crates).
- `src/render.rs` — `recreate_device`, `handle_device_loss_if_needed`, classified `present`, `paint_launcher`/`paint_quick_popover`/`paint_control_center`/`paint_app_menu`, geometry/hit-test pure functions.
- `src/app.rs` — `Popover` struct + `active_popover`, `open_popover`/`close_popover`, per-feature handlers, panic-safety on menu creation, idle-cost fixes in timer 1/12, new `CMD_*` ids.
- `src/status.rs` — add volume *set* alongside the existing read.
- `src/shell.rs` — sleep/restart/shutdown helpers if not already present.
- New small module for `DeviceLossPolicy` + popover geometry, or co-located in `render.rs` per existing style.

## Sequencing & verification gates

1. Phase 1a (device-loss + panic-safety) → `cargo test` green, manual sleep/wake + driver-reset check.
2. Phase 1b (idle-cost + logging) → manual idle Task-Manager check, log file appears.
3. Phase 1c (popover infra, no features) → a throwaway debug popover opens/closes via a temp trigger.
4. Phase 2 → launcher + quick popovers functional.
5. Phase 3 → Control Center functional.
6. Phase 4 → active-app menu functional.
7. Final: full manual matrix + `cargo build --release`.
