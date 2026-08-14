//! Whether the always-on-top capture pill should be on screen, and where it
//! sits when it gets there.
//!
//! The pill is the last line of the capture-visibility invariant: during a
//! full-screen scenario the taskbar and tray are hidden, the main window is
//! usually closed to tray, and WASAPI loopback never lights the Windows
//! microphone indicator — so without it a running capture can be completely
//! invisible.
//!
//! Both decisions — the visibility rule and the default placement arithmetic —
//! are pure and live here rather than in the shell so the things that actually
//! matter ("idle never shows a pill", "a fresh recording starts from the
//! default spot") are unit-testable without a window system.
//! `src-tauri/src/overlay.rs` supplies the inputs and applies the answers.

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

/// Fraction of the monitor height kept clear above the pill, so it lands below
/// the title bars and menus most full-screen apps put along the top edge.
pub const PILL_TOP_MARGIN_FRACTION: f64 = 0.04;

/// Where the pill sits when nobody has dragged it: top-center of the monitor,
/// [`PILL_TOP_MARGIN_FRACTION`] of the screen height down from its top edge.
///
/// All arguments and the result are physical pixels, and the monitor origin is
/// included so a primary monitor placed left of or above the desktop origin
/// (negative coordinates, which Windows allows) still gets a position inside
/// itself rather than on whatever screen owns 0,0.
///
/// The horizontal offset floors at zero: a pill wider than the monitor would
/// otherwise be centered by pushing its left edge off-screen, and the left edge
/// is the half worth keeping.
pub fn default_pill_position(
    monitor_origin: (i32, i32),
    monitor_size: (u32, u32),
    pill_size: (u32, u32),
) -> (i32, i32) {
    let (origin_x, origin_y) = monitor_origin;
    let (screen_width, screen_height) = monitor_size;
    let (pill_width, _) = pill_size;

    let margin = (screen_height as f64 * PILL_TOP_MARGIN_FRACTION) as i32;
    let x = origin_x + ((screen_width as i32 - pill_width as i32) / 2).max(0);
    let y = origin_y + margin;
    (x, y)
}

/// Whether a capture-state change is the start of a *new* recording: exactly
/// the idle -> active edge.
///
/// The shell re-parks the pill on this edge and only this edge, which is what
/// keeps a mid-recording drag respected. Every other transition the shell sees
/// — a watchdog re-broadcast when a device drops or recovers, a settings
/// resync replaying the last broadcast, an idempotent re-start over a live
/// session — arrives while the capture is already active, so none of them can
/// move a pill the user placed.
pub fn is_capture_start(was_active: bool, now_active: bool) -> bool {
    !was_active && now_active
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

    #[test]
    fn default_pill_position_centers_the_pill_with_the_top_margin() {
        // A 1920x1080 primary monitor at the desktop origin, with the pill's
        // configured 320-wide window (`src-tauri/tauri.conf.json`).
        let (x, y) = default_pill_position((0, 0), (1920, 1080), (320, 96));
        assert_eq!(x, (1920 - 320) / 2);
        assert_eq!(y, (1080.0 * 0.04) as i32);
    }

    #[test]
    fn default_pill_position_clamps_to_the_left_edge_when_the_pill_is_wider_than_the_monitor() {
        // Centering a too-wide pill would push its left edge off-screen, which
        // is the end that has to stay reachable.
        let (x, _) = default_pill_position((0, 0), (240, 800), (320, 96));
        assert_eq!(x, 0);
    }

    #[test]
    fn default_pill_position_offsets_by_the_monitor_origin() {
        // A primary monitor left of and above the desktop origin: the position
        // has to land inside that monitor, not on whichever screen owns 0,0.
        let (x, y) = default_pill_position((-1920, -200), (1920, 1080), (320, 96));
        assert_eq!(x, -1920 + (1920 - 320) / 2);
        assert_eq!(y, -200 + (1080.0 * 0.04) as i32);
    }

    #[test]
    fn is_capture_start_fires_only_on_the_idle_to_active_edge() {
        assert!(is_capture_start(false, true));
        // Already recording: a watchdog re-broadcast, a settings resync, or an
        // idempotent re-start must never move a pill the user dragged.
        assert!(!is_capture_start(true, true));
        // Stopping, and staying stopped.
        assert!(!is_capture_start(true, false));
        assert!(!is_capture_start(false, false));
    }
}
