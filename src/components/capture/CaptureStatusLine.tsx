import type { ReactNode } from "react";

type Props = {
  children: ReactNode;
  /**
   * Whether audio is actually being recorded. Full ink when it is, receded
   * when it is not — the state reads through VALUE.
   *
   * Deliberately NOT the reserved green. The green belongs to the spirit mark
   * beside this line, which carries the recording state as a graphic. As text
   * the green measures 3.42-3.70:1 against the light theme's surfaces, under
   * the 4.5:1 floor for small text; as a graphic it clears the 3:1 one
   * comfortably. Keeping the label ink makes the pair legible without
   * spending the green on anything but the mark (docs/DESIGN_SYSTEM.md §6).
   */
  live?: boolean;
  /**
   * `caption` (the default) is the uppercase mono micro-line every quiet
   * capture state uses. `headline` is the live state's own voice: sentence
   * case, sans, semibold, at label size — the design gives the on-air moment
   * a real heading rather than another whisper, and it is the only capture
   * line that gets one.
   */
  variant?: "caption" | "headline";
};

/**
 * One line of capture status, paired with the spirit mark beside it.
 *
 * The sidebar's ListeningIndicator is its only consumer. The two floating
 * capture windows (CaptureOverlayPill, and RecordingStatus inside QuickCapture)
 * draw their own status line rather than mounting this one — they are the
 * surface rather than a chip inside one, and their type sits on glass tuned for
 * the desktop behind them (docs/SPIRIT_MARK.md). What must not diverge between
 * the three is the MEANING — the mark's fill and the label's value — so a change
 * to the live/faint rule here belongs in all three or in none of them.
 *
 * `role="status"` (polite) rather than `alert`: capture state is progress the
 * user initiated, not a failure sprung on them. One region per concern, so a
 * surface with several lines renders several of these rather than one node
 * that rewrites itself (docs/DESIGN_SYSTEM.md §6).
 */
export function CaptureStatusLine({
  children,
  live = false,
  variant = "caption",
}: Props) {
  const look =
    variant === "headline"
      ? "text-label font-semibold leading-tight"
      : `text-cap uppercase tracking-caps ${live ? "" : "text-text-faint"}`;
  return (
    <p role="status" className={`${live ? "text-text" : ""} ${look}`}>
      {children}
    </p>
  );
}
