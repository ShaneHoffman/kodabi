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
/// `capture_active` is "the backend says a capture is running" (any phase
/// other than idle, including the starting-up window), `dismissed_this_session`
/// is the user having hidden the pill for the current capture — a per-session
/// dismissal that the caller clears when capture returns to idle, so the next
/// capture is announced again.
///
/// Note the ordering of the conjunction is not merely stylistic: `capture_active`
/// gates everything, so no combination of settings or dismissal state can put a
/// pill on screen while nothing is being recorded.
pub fn should_show_overlay(
    capture_active: bool,
    origin: CaptureOrigin,
    overlay: OverlaySettings,
    dismissed_this_session: bool,
) -> bool {
    let enabled_for_origin = match origin {
        CaptureOrigin::Manual => overlay.manual_captures,
        CaptureOrigin::AutoDetected => overlay.auto_captures,
    };
    capture_active && enabled_for_origin && !dismissed_this_session
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
                    for dismissed in [false, true] {
                        assert!(
                            !should_show_overlay(false, origin, overlay(manual, auto), dismissed),
                            "idle showed a pill for {origin:?} manual={manual} auto={auto} dismissed={dismissed}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_active_capture_shows_the_pill_when_enabled_and_not_dismissed() {
        assert!(should_show_overlay(
            true,
            CaptureOrigin::Manual,
            overlay(true, false),
            false
        ));
        assert!(should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            overlay(false, true),
            false
        ));
    }

    #[test]
    fn the_setting_for_the_other_origin_is_ignored() {
        // A manual capture must not be revealed by the auto flag, or the
        // dormant default-on setting would leak into today's behavior.
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::Manual,
            overlay(false, true),
            false
        ));
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            overlay(true, false),
            false
        ));
    }

    #[test]
    fn dismissing_hides_the_pill_even_while_enabled_and_active() {
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::Manual,
            overlay(true, true),
            true
        ));
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            overlay(true, true),
            true
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
        assert!(!should_show_overlay(
            true,
            CaptureOrigin::Manual,
            shipped,
            false
        ));
        assert!(should_show_overlay(
            true,
            CaptureOrigin::AutoDetected,
            shipped,
            false
        ));
    }
}
