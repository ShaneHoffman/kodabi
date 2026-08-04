import type { ReactNode } from "react";

type Props = {
  children: ReactNode;
  /**
   * Whether audio is actually being recorded. Full ink when it is, receded
   * when it is not — the state reads through VALUE.
   *
   * Deliberately NOT the reserved green. `--accent-dot` belongs to the dot
   * beside this line, which carries the recording state as a graphic. As text
   * the green measures 3.42-3.70:1 against the light theme's surfaces, under
   * the 4.5:1 floor for small text; as a graphic it clears the 3:1 one
   * comfortably. Keeping the label ink makes the pair legible without
   * spending the green on anything but the dot (docs/DESIGN_SYSTEM.md §6).
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
 * One line of capture status, paired with the listening dot.
 *
 * The remaining pre-Grove on-air surfaces render this — the always-on-top
 * CaptureOverlayPill and the quick-capture window — where they used to spell
 * out the same uppercase treatment and the same live/faint ternary
 * independently. (The main window's indicator no longer does: it is the Grove
 * ListenPill in the transport bar, which carries its own label.)
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
