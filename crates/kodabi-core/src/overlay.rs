//! Whether the always-on-top capture pill should be on screen.
//!
//! The pill is the last line of the capture-visibility invariant: during a
//! full-screen scenario the taskbar and tray are hidden, the main window is
//! usually closed to tray, and WASAPI loopback never lights the Windows
//! microphone indicator — so without it a running capture can be completely
//! invisible.
//!
//! The decision is pure and lives here rather than in the shell so the one
//! rule that actually matters ("idle never shows a pill") is unit-testable
//! without a window system. `src-tauri/src/overlay.rs` supplies the inputs and
//! applies the answer.

use crate::settings::OverlaySettings;

/// How a capture session began.
///
/// Every start path today is [`CaptureOrigin::Manual`]: the hotkey, the tray
/// menu, and the `start_capture` IPC command all originate in a user action.
/// Meeting auto-detection (`docs/FOUNDING_DOC.md` §7) will pass
/// [`CaptureOrigin::AutoDetected`] when it lands — the origin split exists
/// precisely so an unattended start can be *more* visible than a deliberate
/// one, without the detection feature having to revisit this decision.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureOrigin {
    /// The user started this capture themselves.
    #[default]
    Manual,
    /// A detector started this capture without anyone pressing anything.
    AutoDetected,
}

/// Whether the capture pill should be visible right now.
///
/// `capture_active` is "the backend says a capture is running" (any phase other
/// than idle, including the starting-up window). The pill carries no controls of
/// its own — it is pure status, and the only way to hide one is the persistent
/// setting or stopping the capture — so this is the whole rule.
///
/// Note the ordering of the conjunction is not merely stylistic: `capture_active`
/// gates everything, so no combination of settings can put a pill on screen while
/// nothing is being recorded.
pub fn should_show_overlay(
    capture_active: bool,
    origin: CaptureOrigin,
    overlay: OverlaySettings,
) -> bool {
    let enabled_for_origin = match origin {
        CaptureOrigin::Manual => overlay.manual_captures,
        CaptureOrigin::AutoDetected => overlay.auto_captures,
    };
    capture_active && enabled_for_origin
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overlay(manual_captures: bool, auto_captures: bool) -> OverlaySettings {
        OverlaySettings {
            manual_captures,
            auto_captures,
        }
    }

    #[test]
    fn an_idle_capture_never_shows_the_pill() {
        // The trust invariant's other half: the pill claims a recording is
        // running, so it must be impossible to show one when none is.
        for origin in [CaptureOrigin::Manual, CaptureOrigin::AutoDetected] {
            for manual in [false, true] {
                for auto in [false, true] {
                    assert!(
                        !should_show_overlay(false, origin, overlay(manual, auto)),
                        "idle showed a pill for {origin:?} manual={manual} auto={auto}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_active_capture_shows_the_pill_when_enabled() {
        assert!(should_show_overlay(
            true,
            CaptureOrigin::Manual,
            overlay(true, false)
        ));
        assert!(should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            overlay(false, true)
        ));
    }

    #[test]
    fn the_setting_for_the_other_origin_is_ignored() {
        // A manual capture must not be revealed by the auto flag, or the
        // dormant default-on setting would leak into today's behavior.
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::Manual,
            overlay(false, true)
        ));
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            overlay(true, false)
        ));
    }

    #[test]
    fn the_default_origin_is_manual() {
        // Managed state defaults to Manual, so a capture that somehow reached
        // the pill without an announced origin uses the conservative (off by
        // default) setting rather than the dormant auto one.
        assert_eq!(CaptureOrigin::default(), CaptureOrigin::Manual);
    }

    #[test]
    fn the_shipped_defaults_hide_manual_captures_and_would_show_auto_detected() {
        let shipped = OverlaySettings::default();
        assert!(!should_show_overlay(true, CaptureOrigin::Manual, shipped));
        assert!(should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            shipped
        ));
    }
}
