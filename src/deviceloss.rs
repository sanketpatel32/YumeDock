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
pub(crate) struct DeviceLossPolicy {
    device_lost: bool,
}

impl DeviceLossPolicy {
    /// Observe a `Present` HRESULT and decide what to do.
    ///
    /// `hr` is the raw HRESULT value (`hr.0` from a `windows::core::HRESULT`,
    /// which is an `i32`). We cast to `u32` first because the DXGI error
    /// codes (e.g. `0x887A0005`) have bit 31 set and do **not** fit as `i32`
    /// literals — matching on `0x887A0005_i32` would be a compile error.
    pub(crate) const fn classify_present(hr: i32) -> PresentAction {
        let hr = hr as u32;
        // DXGI_ERROR_DEVICE_REMOVED  0x887A0005
        // DXGI_ERROR_DEVICE_HUNG     0x887A0006
        // DXGI_ERROR_DEVICE_RESET    0x887A0007
        if matches!(hr, 0x887A0005..=0x887A0007) {
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
