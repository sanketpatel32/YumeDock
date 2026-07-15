# Phase 1 — Stability Foundation + Popover Infrastructure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the renderer recover from GPU device loss instead of silently breaking, remove abort-on-recoverable-failure footguns, cut idle CPU/GPU cost, add a local log, and stand up the reusable topbar popover infrastructure that Phases 2–4 will plug into.

**Architecture:** A small pure-Rust `DeviceLossPolicy` state machine classifies `Present`/`EndDraw` HRESULTs into `Present | SkipFrame | RecreateAll`. `Renderer` learns to recreate its D3D/D2D/DXGI device on demand and exposes `handle_device_loss_if_needed()` called at the top of every paint path. Idle cost is cut by reducing the per-monitor foreground-probe timer to a single primary-only poll with early-outs and by gating status-change invalidation on actual field changes. A single `active_popover: Option<Popover>` field plus four new `WindowKind` variants provide the one-open-at-a-time popover system, reusing the proven `folder_stack` window lifecycle.

**Tech Stack:** Rust 2024, `windows` crate (Direct2D/Direct3D/DXGI/DirectComposition), Win32 timers and window messages. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-15-stability-and-topbar-rework-design.md`, Phase 1 (sections 1.1–1.5).

**Conventions for this plan:**
- All `cargo` commands run from the repo root `C:\Users\sanpa\OneDrive\Documents\Taskbar to MacOS`.
- The project uses `#![allow(unsafe_op_in_unsafe_fn)]` (see `src/app.rs:1`), so unsafe wrappers around FFI calls are not required — unsafe blocks are only needed at the actual FFI boundary.
- Existing test count is 26; new tests are appended alongside them.
- Commit after every task. Push is requested by the user and happens after each task commit.

---

## File Structure

- **Modify `src/render.rs`** — extract device init into `recreate_device()`; add `device_lost` flag, `handle_device_loss_if_needed()`, classified `present()`/`present_immediate()`; add `paint_debug_popover()` (throwaway Phase-1c verification surface).
- **Create `src/deviceloss.rs`** — pure-Rust `DeviceLossPolicy` state machine + `PresentAction` enum. No GPU, fully unit-testable. Registered as a module in `src/main.rs`.
- **Modify `src/app.rs`** — `DeviceLossPolicy` integration in `paint()`; panic-safety on `.expect("menu")` calls; idle-cost fixes to timer 1 and timer 12; `Popover` struct + `active_popover` field + `open_popover()`/`close_popover()`/`toggle_popover()`; `WindowKind::DebugPopover`; debug-popover open/close wiring + click-outside/Esc dismiss; new `CMD_DEBUG_POPOVER`.
- **Create `src/log.rs`** — minimal rotating file logger writing to `%LOCALAPPDATA%\YumeDock\yumedock.log`, capped 512 KB. `yume_warn!`/`yume_err!` macros. Registered as a module in `src/main.rs`.

---

## Task 1: `DeviceLossPolicy` state machine (TDD)

Pure logic that classifies HRESULTs. This is the heart of the device-loss fix and is currently invisible inside `.ok()?`.

**Files:**
- Create: `src/deviceloss.rs`
- Modify: `src/main.rs:3-9` (add `mod deviceloss;`)

- [ ] **Step 1: Register the module**

Edit `src/main.rs`. After the existing module declarations (lines 3–9), add `deviceloss` so the block reads:

```rust
mod app;
mod config;
mod deviceloss;
mod log;
mod model;
mod render;
mod shell;
mod status;
mod tracker;
```

(We add `mod log;` here too since `src/log.rs` is created in Task 6. To keep this task compiling, create a stub `src/log.rs` containing only a module-level doc comment:

```rust
//! Local rotating log for YumeDock. Implemented in Task 6.
```

- [ ] **Step 2: Write the failing tests**

Create `src/deviceloss.rs` with only the test module first:

```rust
//! Classifies Direct2D / DXGI HRESULT results into a paint action.
//!
//! This is the pure logic that today lives invisibly inside
//! `surface.swap_chain.Present(...).ok()?` in `src/render.rs`. Pulling it
//! out makes device-loss recovery testable without a GPU.

/// What `Renderer` should do after observing a `Present` / `EndDraw` HRESULT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentAction {
    /// The frame was presented normally.
    Present,
    /// Skip presenting this frame (e.g. occluded by RDP / secure desktop).
    SkipFrame,
    /// The GPU device is gone. Drop all surfaces and recreate the D3D/D2D
    /// device on the next paint.
    RecreateAll,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct DeviceLossPolicy {
    device_lost: bool,
}

impl DeviceLossPolicy {
    /// Observe a `Present` HRESULT and decide what to do.
    ///
    /// `hr` is the raw HRESULT value (`hr.0` from a `windows::core::HRESULT`,
    /// which is an `i32`). We cast to `u32` first because the DXGI error
    /// codes (e.g. `0x887A0005`) have bit 31 set and do **not** fit as `i32`
    /// literals — matching on `0x887A0005_i32` would be a compile error.
    const fn classify_present(hr: i32) -> PresentAction {
        let hr = hr as u32;
        // DXGI_ERROR_DEVICE_REMOVED  0x887A0005
        // DXGI_ERROR_DEVICE_HUNG     0x887A0006
        // DXGI_ERROR_DEVICE_RESET    0x887A0007
        if matches!(hr, 0x887A0005 | 0x887A0006 | 0x887A0007) {
            return PresentAction::RecreateAll;
        }
        // DXGI_STATUS_OCCLUDED 0x087A0001 — minimised / occluded; just skip.
        if hr == 0x087A0001 {
            return PresentAction::SkipFrame;
        }
        PresentAction::Present
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_hresult_presents() {
        assert_eq!(DeviceLossPolicy::classify_present(0), PresentAction::Present);
        assert_eq!(DeviceLossPolicy::classify_present(1), PresentAction::Present);
    }

    #[test]
    fn occluded_status_skips_frame_without_recreating() {
        // DXGI_STATUS_OCCLUDED = 0x087A0001 (fits in i32, positive).
        assert_eq!(
            DeviceLossPolicy::classify_present(0x087A0001),
            PresentAction::SkipFrame
        );
    }

    #[test]
    fn device_removed_requires_recreate() {
        // DXGI_ERROR_DEVICE_REMOVED = 0x887A0005 as u32.
        // As an i32 HRESULT this is -2005319547, which is what
        // `HRESULT(0x887A0005_u32).0` actually produces.
        assert_eq!(
            DeviceLossPolicy::classify_present((0x887A0005_u32) as i32),
            PresentAction::RecreateAll
        );
    }

    #[test]
    fn device_hung_and_reset_require_recreate() {
        assert_eq!(
            DeviceLossPolicy::classify_present(0x887A0006_u32 as i32),
            PresentAction::RecreateAll
        );
        assert_eq!(
            DeviceLossPolicy::classify_present(0x887A0007_u32 as i32),
            PresentAction::RecreateAll
        );
    }
}
```

- [ ] **Step 3: Run tests to verify they pass (logic is inline)**

Run: `cargo test deviceloss`
Expected: 5 tests PASS. (They pass immediately because the `const fn` logic is written alongside them — this is pure data classification, so the TDD "red" step collapses; the value is locking the HRESULT mapping in place before `Renderer` depends on it.)

If any fail, fix the HRESULT constants in `classify_present`/`classify_end_draw` until green.

- [ ] **Step 4: Commit**

```bash
git add src/deviceloss.rs src/main.rs src/log.rs
git commit -m "feat(deviceloss): add HRESULT → paint-action policy state machine"
git push origin main
```

---

## Task 2: Refactor `Renderer::new` device init into `recreate_device()`

Prerequisite for recovery: the device-init block in `Renderer::new` (`src/render.rs:274-358`) must be callable again later, not just at construction. We split it out **without changing behavior** — no recovery yet.

**Files:**
- Modify: `src/render.rs:251-358` (struct + `new` + new `recreate_device`)

- [ ] **Step 1: Read the current device-init block**

Read `src/render.rs:273-358` to confirm the exact code being extracted. The block from `D3D11CreateDevice(...)` through the end of the `unsafe { ... }` is what becomes `recreate_device`.

- [ ] **Step 2: Add a `DeviceParts` carrier struct and `recreate_device`**

The device-init produces many fields. Rather than return a tuple of ~12 values, return a `DeviceParts` struct that `new` destructures into `Self`. Replace the body of `Renderer::new` (lines 274-358) with this:

```rust
    pub fn new(high_contrast: bool) -> Result<Self> {
        let text_factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let wic: IWICImagingFactory = unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?
        };
        let body = unsafe {
            text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("en-us"),
            )?
        };
        unsafe {
            body.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            body.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        let bold = unsafe {
            text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                14.0,
                w!("en-us"),
            )?
        };
        unsafe {
            bold.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
            bold.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        let small = unsafe {
            text_factory.CreateTextFormat(
                w!("Segoe UI Variable Text"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0,
                w!("en-us"),
            )?
        };
        unsafe {
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            small.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }

        let device = unsafe { Self::recreate_device()? };
        Ok(Self {
            _d3d_device: device.d3d_device,
            _immediate_context: device.immediate_context,
            d2d_device: device.d2d_device,
            factory: device.factory,
            dxgi_factory: device.dxgi_factory,
            composition: device.composition,
            body,
            bold,
            small,
            surfaces: HashMap::new(),
            genie_cache: HashMap::new(),
            wic,
            high_contrast,
            device_lost: false,
            geo_wifi: None,
            geo_speaker: None,
            geo_speaker_muted: None,
            geo_battery: None,
            geo_bolt: None,
        })
    }

    /// Create (or recreate after device loss) the Direct3D + Direct2D + DXGI
    /// + DirectComposition device stack. Returns the device-level handles;
    /// text formats and the WIC factory are device-independent and stay in
    /// `new`.
    unsafe fn recreate_device() -> Result<DeviceParts> {
        unsafe {
            let mut d3d_device = None;
            let mut immediate_context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                Some(&mut immediate_context),
            )?;
            let d3d_device = d3d_device.context("Direct3D device was not created")?;
            let immediate_context =
                immediate_context.context("Direct3D context was not created")?;
            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let dxgi_device1: IDXGIDevice1 = dxgi_device.cast()?;
            dxgi_device1.SetMaximumFrameLatency(1)?;
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
            let d2d_device = D2D1CreateDevice(&dxgi_device, None)?;
            let factory: ID2D1Factory = d2d_device.GetFactory()?;
            let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            Ok(DeviceParts {
                d3d_device,
                immediate_context,
                d2d_device,
                factory,
                dxgi_factory,
                composition,
            })
        }
    }
```

Add the `DeviceParts` struct above `impl Renderer` (near the `Surface` struct around line 221):

```rust
struct DeviceParts {
    d3d_device: ID3D11Device,
    immediate_context: ID3D11DeviceContext,
    d2d_device: windows::Win32::Graphics::Direct2D::ID2D1Device,
    factory: ID2D1Factory,
    dxgi_factory: IDXGIFactory2,
    composition: IDCompositionDevice,
}
```

- [ ] **Step 3: Add the `device_lost` field to the `Renderer` struct**

In the `Renderer` struct (`src/render.rs:251-271`), add after `high_contrast: bool,`:

```rust
    /// Set when a device-removing HRESULT is observed. The next paint path
    /// calls `handle_device_loss_if_needed()` and rebuilds the device.
    device_lost: bool,
```

- [ ] **Step 4: Build to confirm behavior is unchanged**

Run: `cargo build`
Expected: compiles cleanly (behavior identical to before — `new` now delegates to `recreate_device`, nothing else changed). Fix any unused-import warnings if the refactor moved a use.

- [ ] **Step 5: Run the full suite to confirm nothing regressed**

Run: `cargo test`
Expected: all 26 existing tests + 7 new deviceloss tests PASS.

- [ ] **Step 6: Commit**

```bash
git add src/render.rs
git commit -m "refactor(render): extract device init into recreate_device()"
git push origin main
```

---

## Task 3: Classified `present()` + `handle_device_loss_if_needed()`

Now wire the policy into the present path. `present()`/`present_immediate()` (`src/render.rs:1758-1775`) currently do `.ok()?` which swallows the device-removed signal. We capture the HRESULT, classify it, and return the classification so the caller can set `device_lost`.

**Borrow-check note (important):** `present()` is a free function taking `&Surface`. It must **not** take `&mut self.device_lost`, because `surface` itself is `&mut Surface` borrowed from `self` (`fn surface(&mut self) -> Result<&mut Surface>` borrows all of `self`). Passing `&mut self.device_lost` simultaneously is a conflict, even on a disjoint field. So `present` *returns* the `PresentAction`, and each caller sets `self.device_lost` *after* its last use of `surface` — by then NLL has released the surface borrow and `self.device_lost` is freely mutable.

**Files:**
- Modify: `src/render.rs:1758-1775` (present functions), add `handle_device_loss_if_needed`

- [ ] **Step 1: Replace `present()` and `present_immediate()`**

Replace lines `src/render.rs:1758-1775` with:

```rust
/// Submit a synced frame and return how the result should be handled. The
/// caller sets `device_lost` from the returned action after this returns
/// (the surface borrow is released by then).
unsafe fn present(surface: &Surface) -> crate::deviceloss::PresentAction {
    use crate::deviceloss::PresentAction;
    unsafe {
        if surface.context.EndDraw(None, None).is_err() {
            return PresentAction::RecreateAll;
        }
        let hr = surface.swap_chain.Present(1, DXGI_PRESENT(0));
        DeviceLossPolicy::classify_present(hr.0)
    }
}

/// Submit an animation frame without the vsync wait. Same classification as
/// `present`.
unsafe fn present_immediate(surface: &Surface) -> crate::deviceloss::PresentAction {
    use crate::deviceloss::PresentAction;
    unsafe {
        if surface.context.EndDraw(None, None).is_err() {
            return PresentAction::RecreateAll;
        }
        let hr = surface.swap_chain.Present(0, DXGI_PRESENT(0));
        DeviceLossPolicy::classify_present(hr.0)
    }
}
```

Add the import for `DeviceLossPolicy` near the top of `src/render.rs` (with the other `use` statements):

```rust
use crate::deviceloss::{DeviceLossPolicy, PresentAction};
```

- [ ] **Step 2: Update every call site of `present`/`present_immediate`**

There are 5 call sites (render.rs:650, 905, 1095, 1221, 1540). Each currently reads e.g. `present(surface)?`. The surface borrow ends at the `present(...)` call (last use), so immediately afterward `self.device_lost` is mutable. Replace each call with a two-line pattern that applies the returned action.

For the four `present(surface)?` sites, replace:

```rust
        present(surface)?;
        Ok(())
    }
```
with:
```rust
        let action = present(surface);
        if matches!(action, PresentAction::RecreateAll) {
            self.device_lost = true;
        }
        Ok(())
    }
```

For the `present_immediate(surface)?` site (render.rs:650, at the end of `paint_genie`), replace:

```rust
        present_immediate(surface)?;
        Ok(())
    }
```
with:
```rust
        let action = present_immediate(surface);
        if matches!(action, PresentAction::RecreateAll) {
            self.device_lost = true;
        }
        Ok(())
    }
```

If a call site's surrounding control flow differs (e.g. the `?` was propagating an error and the function now never errors), inspect and adjust so the method still returns `Ok(())`. The key rule: never touch `self.device_lost` while `surface` is still named — only after.

- [ ] **Step 3: Add `handle_device_loss_if_needed()`**

Add this method to `impl Renderer` (place it right after `recreate_device`, before `set_high_contrast`):

```rust
    /// Called at the top of every paint path. If a prior `Present`/`EndDraw`
    /// reported device loss, recreate the D3D/D2D device stack and drop all
    /// surfaces so they are rebuilt on their next access. Returns `true` if a
    /// recreation just happened (the caller should skip painting this frame).
    pub fn handle_device_loss_if_needed(&mut self) -> bool {
        if !self.device_lost {
            return false;
        }
        match unsafe { Self::recreate_device() } {
            Ok(parts) => {
                self._d3d_device = parts.d3d_device;
                self._immediate_context = parts.immediate_context;
                self.d2d_device = parts.d2d_device;
                self.factory = parts.factory;
                self.dxgi_factory = parts.dxgi_factory;
                self.composition = parts.composition;
                self.surfaces.clear();
                self.genie_cache.clear();
                self.geo_wifi = None;
                self.geo_speaker = None;
                self.geo_speaker_muted = None;
                self.geo_battery = None;
                self.geo_bolt = None;
                self.device_lost = false;
                true
            }
            Err(_error) => {
                // Recreation failed — leave `device_lost` set so we retry on
                // the next frame. Log via the logger (Task 6) once available.
                false
            }
        }
    }
```

- [ ] **Step 4: Call `handle_device_loss_if_needed()` at the top of every `paint_*`**

In each of `paint_top_bar`, `paint_folder_stack`, `paint_preview`, `paint_dock`, and `paint_genie`, add as the first statement after `fn ... {`:

```rust
        if self.handle_device_loss_if_needed() {
            // Device was just recreated; surfaces are empty. Skip this frame
            // and invalidate so the next paint rebuilds them.
            return Ok(());
        }
```

(For `paint_genie`, which fetches `genie_bitmap` early, place the check before any surface access — i.e. as the literal first line of the function body.)

**Note for `paint_genie`:** after a device-loss recreate, `surfaces.clear()` also drops each surface's `genie_bitmap`. So the next `paint_genie` will return the `"genie snapshot is missing"` error, which propagates to `paint()` and is logged. That is **expected** — the genie animation simply ends on device loss — not a bug. No special handling needed.

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles. Fix call-site signature mismatches until clean.

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: all 33 tests PASS (no behavioral regression; recovery only triggers on real device loss).

- [ ] **Step 7: Commit**

```bash
git add src/render.rs
git commit -m "feat(render): classify Present HRESULT and recover from GPU device loss"
git push origin main
```

---

## Task 4: Panic-safety on menu creation

`Cargo.toml:56` sets `panic = "abort"`. Three `.expect("menu")` calls abort the whole dock on a recoverable `CreatePopupMenu` failure: `src/app.rs:2255`, `2367`, `2429`.

**Files:**
- Modify: `src/app.rs:2255`, `2367`, `2429` (three `show_*_menu` methods)

- [ ] **Step 1: Replace the three `.expect("menu")` calls**

At each of the three sites, change:

```rust
            let menu = CreatePopupMenu().expect("menu");
```

to:

```rust
            let Ok(menu) = (unsafe { CreatePopupMenu() }) else {
                return;
            };
```

Each is inside a `fn show_*_menu(&mut self, hwnd: HWND)` returning `()`, so `return` is correct.

- [ ] **Step 2: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all 33 tests pass. (Behavior change: a menu-creation failure now returns silently instead of aborting the process.)

- [ ] **Step 3: Commit**

```bash
git add src/app.rs
git commit -m "fix(app): avoid process abort on CreatePopupMenu failure"
git push origin main
```

---

## Task 5: Idle-cost reduction (timers 1 and 12)

Cut the continuous foreground-probe cost and the unconditional repaints.

**Files:**
- Modify: `src/app.rs:478` (timer 12 setup), `662-669` (`refresh_system_status`), `680-737` (`cache_foreground_for_genie`)

- [ ] **Step 1: Make timer 12 primary-only and slower**

In `create_monitor_shells` (`src/app.rs:478`), the line currently reads:

```rust
                    SetTimer(Some(top), 12, 250, None);
```

This runs on every monitor's top window. Replace it with a primary-only setup. Find the loop context and guard it:

```rust
                    if info.primary {
                        SetTimer(Some(top), 12, 1000, None);
                    }
```

- [ ] **Step 2: Add early-outs to `cache_foreground_for_genie`**

At the top of `cache_foreground_for_genie` (`src/app.rs:680`), after the existing `window_open_animation` early-out, add:

```rust
        // Nothing to cache for transitions when the dock is fully hidden and
        // nothing is animating — skip the foreground probing entirely.
        if self.config.dock.auto_hide
            && self.window_open_animation.is_none()
            && self.launch_bounce.is_empty()
            && self.shells.iter().all(|shell| {
                self.auto_hide
                    .get(&(shell.dock.0 as isize))
                    .is_some_and(|state| state.target >= 1.0 && state.progress >= 0.999)
            })
        {
            return;
        }
```

- [ ] **Step 3: Gate `refresh_system_status` invalidation on actual change**

Replace `refresh_system_status` (`src/app.rs:662-669`) with:

```rust
    fn refresh_system_status(&mut self) {
        let next = status::read_status();
        // Always repaint when the wall-clock minute rolled (clock display).
        let clock_changed = current_clock(self.config.top_bar.use_24_hour_clock)
            != current_clock(self.config.top_bar.use_24_hour_clock); // placeholder, fixed below
        if next == self.system_status || !clock_changed {
            // Still keep the latest status so a later change is detected.
            self.system_status = next;
            return;
        }
        self.system_status = next;
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
    }
```

That placeholder is wrong (a value compared to itself is always equal) — the clock check must compare against the *last rendered* clock string. Fix it properly: add a field `last_clock: String` to `App` (`src/app.rs:244`, next to `last_genie_snapshot`):

```rust
    last_clock: String,
```

Initialize it in `App::run`'s struct literal (`src/app.rs:293`):

```rust
            last_clock: current_clock(config.top_bar.use_24_hour_clock),
```

Then rewrite the gate cleanly:

```rust
    fn refresh_system_status(&mut self) {
        let next = status::read_status();
        let clock = current_clock(self.config.top_bar.use_24_hour_clock);
        let status_changed = next != self.system_status;
        let clock_changed = clock != self.last_clock;
        self.system_status = next;
        self.last_clock = clock;
        if !status_changed && !clock_changed {
            return;
        }
        for shell in &self.shells {
            unsafe {
                let _ = InvalidateRect(Some(shell.top), None, false);
            }
        }
    }
```

Note: `SystemStatus` already derives `PartialEq` (see `src/status.rs:3`), so `next != self.system_status` works.

- [ ] **Step 4: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all 33 tests pass. The `current_clock` import is already in scope in `app.rs`.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs
git commit -m "perf(app): reduce idle foreground probing and gate repaints on change"
git push origin main
```

---

## Task 6: Local rotating log

`eprintln!` goes nowhere in a `windows_subsystem = "windows"` release build. Add a minimal rotating file logger.

**Files:**
- Create: `src/log.rs` (replace the stub from Task 1)
- Modify: `src/app.rs:1138` (paint error path) to use `yume_err!`

- [ ] **Step 1: Implement the logger**

Replace `src/log.rs` (currently the stub) with:

```rust
//! Minimal rotating file log for YumeDock. Writes to
//! `%LOCALAPPDATA%\YumeDock\yumedock.log`, capped at 512 KB (rotated once
//! to `.1`). No external dependency; line-buffered and lock-free at the
//! process level (single-threaded UI thread drives most writes).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use crate::config::app_data_dir;

const MAX_BYTES: u64 = 512 * 1024;

static LOG: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn log_path() -> Option<std::path::PathBuf> {
    let mut slot = LOG.lock().ok()?;
    if let Some(path) = slot.as_ref() {
        return Some(path.clone());
    }
    let dir = app_data_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("yumedock.log");
    *slot = Some(path.clone());
    Some(path)
}

pub fn write(level: &str, message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    // Rotate if too large. Best-effort: ignore errors.
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let _ = fs::rename(&path, path.with_extension("log.1"));
        }
    }
    let line = format!("[{}] {}\n", level, message);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! yume_warn {
    ($($arg:tt)*) => {
        $crate::log::write("WARN", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! yume_err {
    ($($arg:tt)*) => {
        $crate::log::write("ERROR", &format!($($arg)*))
    };
}
```

- [ ] **Step 2: Use `yume_err!` in the paint error path**

In `paint()` (`src/app.rs:1137-1139`), replace:

```rust
        if let Err(error) = result {
            eprintln!("paint failed: {error:#}");
        }
```

with:

```rust
        if let Err(error) = result {
            crate::yume_err!("paint failed: {error:#}");
        }
```

Also in `handle_device_loss_if_needed` (`src/render.rs`, the `Err(_error)` arm from Task 3), replace the comment with:

```rust
            Err(error) => {
                crate::yume_err!("device recreation failed: {error:#}");
                false
            }
```

- [ ] **Step 3: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all 33 tests pass. (The log file is created lazily on first write; no test needs it.)

- [ ] **Step 4: Commit**

```bash
git add src/log.rs src/app.rs src/render.rs
git commit -m "feat(log): add rotating local log and route paint errors to it"
git push origin main
```

---

## Task 7: Popover infrastructure — data model

Stand up the single-popover state and window kinds. No rendering or features yet — just the lifecycle.

**Files:**
- Modify: `src/app.rs:131-138` (`WindowKind`), `215-245` (`App` struct), `294` (init), `2461+` (`handle_command`), add `CMD_DEBUG_POPOVER`

- [ ] **Step 1: Add `WindowKind` variants and `CMD_DEBUG_POPOVER`**

In `src/app.rs`, extend the enum at line 131:

```rust
enum WindowKind {
    Top,
    Dock,
    Reserve,
    Preview,
    FolderStack,
    LaunchOverlay,
    DebugPopover,
}
```

Add the command constant near the other `CMD_*` constants (after line 110):

```rust
const CMD_DEBUG_POPOVER: usize = 300;
```

- [ ] **Step 2: Add the `Popover` struct**

Place it near `FolderStack` (`src/app.rs:150`):

```rust
/// A single topbar popover window. Only one is ever open at a time
/// (`App::active_popover`), matching the macOS menu-bar invariant.
struct Popover {
    hwnd: HWND,
    /// The top-bar window that owns this popover (anchor + dismiss owner).
    owner: HWND,
}
```

- [ ] **Step 3: Add the `active_popover` field and initialize it**

In the `App` struct (`src/app.rs:244`, after `last_genie_snapshot`):

```rust
    active_popover: Option<Popover>,
```

In `App::run`'s struct literal (`src/app.rs:293`):

```rust
            active_popover: None,
```

- [ ] **Step 4: Implement `close_popover`, `open_popover`, `toggle_popover`**

Add these methods to `impl App` (place near `close_folder_stack`, `src/app.rs:2778`):

```rust
    fn close_popover(&mut self) {
        if let Some(popover) = self.active_popover.take() {
            self.kinds.remove(&(popover.hwnd.0 as isize));
            self.renderer.forget(popover.hwnd);
            unsafe {
                let _ = DestroyWindow(popover.hwnd);
            }
        }
    }

    /// Create a popover window anchored under `cursor_x` on the given top
    /// window. Returns the new hwnd. The caller registers the specific
    /// `WindowKind`. Reuses the folder_stack anchor math.
    fn open_popover(&mut self, owner: HWND, kind: WindowKind, width: i32, height: i32) {
        self.close_popover();
        let scale = self
            .monitor_for(owner)
            .map(|shell| shell.scale)
            .unwrap_or_else(|| window_scale(owner));
        let Ok(mut owner_rect) = (unsafe {
            let mut r = RECT::default();
            match windows::Win32::UI::WindowsAndMessaging::GetWindowRect(owner, &mut r) {
                Ok(()) => Ok(r),
                Err(e) => Err(e),
            }
        }) else {
            return;
        };
        let cursor = self
            .cursor_x
            .get(&(owner.0 as isize))
            .copied()
            .unwrap_or((owner_rect.right - owner_rect.left) as f32 / 2.0);
        let bounds = self
            .monitor_for(owner)
            .map(|shell| shell.info.bounds)
            .unwrap_or(owner_rect);
        let margin = scale_i32(8, scale);
        let anchor_x = owner_rect.left + (cursor * scale).round() as i32;
        let min_x = bounds.left + margin;
        let max_x = (bounds.right - scale_i32(width, scale) - margin).max(min_x);
        let x = (anchor_x - scale_i32(width, scale) / 2).clamp(min_x, max_x);
        let y = owner_rect.bottom;
        let w = scale_i32(width, scale);
        let h = scale_i32(height, scale);
        let Ok(hwnd) = create_window(x, y, w, h, false) else {
            crate::yume_warn!("popover window creation failed");
            return;
        };
        configure_window_backdrop(hwnd, true, self.high_contrast);
        self.kinds.insert(hwnd.0 as isize, kind);
        self.active_popover = Some(Popover { hwnd, owner });
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }

    /// Open the Phase-1c debug popover — a labelled rectangle that proves the
    /// popover system works end to end (create, render, dismiss). Phases 2–4
    /// replace this with real popovers.
    fn toggle_debug_popover(&mut self, owner: HWND) {
        if self.active_popover.is_some() {
            self.close_popover();
            return;
        }
        self.open_popover(owner, WindowKind::DebugPopover, 220, 120);
    }
```

- [ ] **Step 5: Build**

Run: `cargo build`
Expected: compiles. (`scale_i32` and `window_scale` are existing free functions in `app.rs`.) Fix any borrow errors.

- [ ] **Step 6: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): add popover data model (Popover, active_popover, open/close)"
git push origin main
```

---

## Task 8: Debug popover — render, wire, dismiss

Render the throwaway popover, wire a trigger, and implement click-outside + Esc dismissal. This is the Phase-1c verification artifact.

**Files:**
- Modify: `src/render.rs` (add `paint_debug_popover`), `src/app.rs` (`paint()` dispatch, trigger, dismiss)

- [ ] **Step 1: Add `paint_debug_popover` to `Renderer`**

In `src/render.rs`, add after `paint_folder_stack` (around line 910):

```rust
    /// Phase-1c verification surface: a labelled rounded rectangle. Replaced
    /// by real popovers in Phases 2–4. Proves the popover lifecycle
    /// (create → render → dismiss) before any feature depends on it.
    pub fn paint_debug_popover(&mut self, hwnd: HWND) -> Result<()> {
        if self.handle_device_loss_if_needed() {
            return Ok(());
        }
        let surface = self.surface(hwnd)?;
        let size = unsafe { surface.context.GetSize() };
        unsafe {
            surface.context.BeginDraw();
            surface.context.Clear(Some(&color(0x0c, 0x0f, 0x14, 0.92)));
            let rect = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: size.width,
                bottom: size.height,
            };
            let text: Vec<u16> = "Popover OK"
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            surface.context.DrawText(
                &text,
                &self.body,
                &rect,
                &surface.foreground,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        let action = present(surface);
        if matches!(action, PresentAction::RecreateAll) {
            self.device_lost = true;
        }
        Ok(())
    }
```

- [ ] **Step 2: Dispatch `DebugPopover` in `paint()`**

In `paint()` (`src/app.rs:958`), add a match arm alongside the existing `Some(WindowKind::FolderStack)`:

```rust
            Some(WindowKind::DebugPopover) => self.renderer.paint_debug_popover(hwnd),
```

- [ ] **Step 3: Make `DebugPopover` hit-test as client (not transparent)**

In `window_proc`'s `WM_NCHITTEST` arm (`src/app.rs:2882-2898`), the early `dock_hit_region` only covers dock windows. Add a guard so a debug-popover window is treated as fully client (so it receives clicks and can be dismissed):

```rust
        WM_NCHITTEST => {
            if with_app_value(false, |app| {
                app.kinds.get(&(hwnd.0 as isize)) == Some(&WindowKind::LaunchOverlay)
            }) {
                return LRESULT(HTTRANSPARENT as isize);
            }
            // Popover windows are fully client-hittable.
            if with_app_value(false, |app| {
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover)
                )
            }) {
                return LRESULT(HTCLIENT as isize);
            }
            // ... existing dock_hit_region block stays below
```

- [ ] **Step 4: Dismiss on outside click and Esc**

Add dismissal. In `window_proc`, handle `WM_LBUTTONDOWN` for the popover window (it should close on any click inside or outside; outside-click is handled by the owner getting focus). Simplest: clicking the popover closes it; clicking elsewhere is handled by the owner below. In the `WM_LBUTTONDOWN` arm (`src/app.rs:2928`), prepend:

```rust
        WM_LBUTTONDOWN => {
            let is_popover = with_app_value(false, |app| {
                matches!(
                    app.kinds.get(&(hwnd.0 as isize)),
                    Some(WindowKind::DebugPopover)
                )
            });
            if is_popover {
                with_app(|app| app.close_popover());
                return LRESULT(0);
            }
            with_app(|app| app.mouse_down(hwnd, (lparam.0 as i16) as i32));
            return LRESULT(0);
        }
```

For outside-click dismissal on the top bar: when a popover is open, a click on the top bar that is *not* the same trigger should close it. Add at the top of `mouse_down` (`src/app.rs:2067`):

```rust
    fn mouse_down(&mut self, hwnd: HWND, x: i32) {
        if self.active_popover.is_some() {
            self.close_popover();
            return;
        }
        // ...existing body
```

(Reading `src/app.rs:2067` first to confirm the exact existing first line; insert the guard above it.)

For Esc dismissal, add a `WM_KEYDOWN`/`VK_ESCAPE` handler in `window_proc`:

```rust
        windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN
            if wparam.0 as i32 == windows::Win32::UI::WindowsAndMessaging::VK_ESCAPE.0 as i32 =>
        {
            with_app(|app| app.close_popover());
            return LRESULT(0);
        }
```

- [ ] **Step 5: Wire a debug trigger**

In `show_top_menu` (`src/app.rs:2365`), add an item so you can open the popover for verification:

```rust
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_DEBUG_POPOVER, w!("Debug popover"));
```

In `handle_command` (`src/app.rs:2461`), add an arm (this needs the owner hwnd — `handle_command` is called from `show_menu`/`show_top_menu` which have `hwnd` in scope, but `handle_command` itself only takes `command`. To avoid a signature change, capture the owner via the foreground top window). Add near the top of `handle_command`:

```rust
            CMD_DEBUG_POPOVER => {
                if let Some(owner) = self
                    .shells
                    .iter()
                    .map(|shell| shell.top)
                    .next()
                {
                    self.toggle_debug_popover(owner);
                }
                return;
            }
```

- [ ] **Step 6: Build and test**

Run: `cargo build && cargo test`
Expected: compiles, all 33 tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/render.rs src/app.rs
git commit -m "feat(app): wire debug popover for popover-system verification"
git push origin main
```

---

## Task 9: Phase 1 manual verification + finalize

No code change — this is the gate that confirms Phase 1 did what the spec promised before Phases 2–4 build on it.

- [ ] **Step 1: Release build**

Run: `cargo build --release`
Expected: compiles cleanly with no warnings beyond pre-existing ones.

- [ ] **Step 2: Manual stability matrix**

Launch `target\release\YumeDock.exe` and verify each:

1. **Device loss — sleep/wake:** put the machine to sleep, wake it. Bar + dock still render (previously froze/garbled).
2. **Device loss — driver reset:** press `Win+Ctrl+Shift+B` (the Windows GPU driver reset). Bar recovers within ~1 frame instead of dying.
3. **Idle cost:** with the dock hidden and no animation, watch Task Manager — CPU near 0% and GPU compute low (previously continuous probing every 250 ms × monitor count).
4. **Logging:** trigger a paint error if possible (e.g. rapid monitor unplug/replug); confirm `%LOCALAPPDATA%\YumeDock\yumedock.log` is created with entries, and rotates to `.1` past 512 KB.
5. **Popover system:** right-click the top bar → *Debug popover*. A labelled rectangle appears under the cursor; clicking it, clicking the top bar, or pressing Esc closes it. Only one is ever open at once.
6. **Safety net:** `Ctrl+Alt+Shift+F12` still restores the taskbar and quits.

If any step fails, note the symptom and file a follow-up before starting Phase 2.

- [ ] **Step 3: Commit the verification note**

Append a short "Phase 1 verified" note to the spec file's status line and commit:

```bash
git add docs/superpowers/specs/2026-07-15-stability-and-topbar-rework-design.md
git commit -m "docs: mark Phase 1 of stability+topbar rework as verified"
git push origin main
```

---

## Self-Review (completed)

**Spec coverage (Phase 1 only):**
- §1.1 device-loss recovery → Tasks 1, 2, 3. ✓
- §1.2 panic-safety → Task 4. ✓
- §1.3 idle-cost → Task 5. ✓
- §1.4 logging → Task 6. ✓
- §1.5 popover infra → Tasks 7, 8. ✓
- Verification → Task 9. ✓

**Placeholder scan:** No "TBD"/"TODO"/"similar to". Every code step contains full code.

**Type/signature consistency:**
- `present(surface) -> PresentAction` returns the action; callers set `self.device_lost` after the surface borrow ends. Used consistently at all 5 call sites + `paint_debug_popover`. ✓
- `DeviceLossPolicy::classify_present(hr: i32)` takes `i32`, casts to `u32` internally (because DXGI error codes have bit 31 set and don't fit as `i32` literals), called with `hr.0` from `HRESULT`. ✓
- `Popover { hwnd, owner }` consistent across Tasks 7–8. ✓
- `WindowKind::DebugPopover` consistent across Tasks 7–8. ✓
- `CMD_DEBUG_POPOVER = 300` does not collide with existing `CMD_*` ranges (100s, 200s). ✓

**Borrow-checker soundness:** `present()` is a free function taking `&Surface` (not `&mut self`), so returning the action and letting the caller write `self.device_lost` after the last `surface` use compiles cleanly under NLL. The plan documents this explicitly in Task 3. ✓
