import type { ReactNode } from "react";

type Props = {
  children: ReactNode;
  /**
   * Whether audio is actually being recorded. Full ink when it is, receded
   * when it is not — the state reads through VALUE.
   *
   * Deliberately NOT the reserved green. `--accent-dot` belongs to the
   * SpiritMark, which sits beside every one of these lines and carries the
   * recording state as a graphic. As text the green measures 3.42-3.70:1
   * against the light theme's surfaces, under the 4.5:1 floor for small text;
   * as a graphic it clears the 3:1 one comfortably. Moving the label to ink
   * makes the pair legible without spending the green on anything but the mark
   * (docs/DESIGN_SYSTEM.md §6).
   */
  live?: boolean;
};

/**
 * One line of capture status, paired with a SpiritMark.
 *
 * Both on-air surfaces render this — the sidebar's ListeningIndicator and the
 * always-on-top CaptureOverlayPill — and both used to spell out the same
 * uppercase treatment and the same live/faint ternary independently.
 *
 * `role="status"` (polite) rather than `alert`: capture state is progress the
 * user initiated, not a failure sprung on them. One region per concern, so a
 * surface with several lines renders several of these rather than one node
 * that rewrites itself (docs/DESIGN_SYSTEM.md §6).
 */
export function CaptureStatusLine({ children, live = false }: Props) {
  return (
    <p
      role="status"
      className={`text-cap uppercase tracking-caps ${
        live ? "text-text" : "text-text-faint"
      }`}
    >
      {children}
    </p>
  );
}
