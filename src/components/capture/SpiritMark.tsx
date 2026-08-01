import type { CSSProperties } from "react";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; the capture windows' Grove ticket deletes it
import "./SpiritMark.css";

/**
 * What the mark depicts. A *visual* mode rather than the capture phase
 * directly: the reserved green means "audio is being recorded", so a degraded
 * capture with nothing live renders `reconnecting` — ink, so it never implies
 * it is on air, but moving, so it is never mistaken for the dormant `idle`
 * mark of a session that isn't running at all.
 */
export type SpiritMarkMode =
  | "idle"
  | "starting"
  | "listening"
  | "degraded"
  | "reconnecting";

type Props = {
  mode: SpiritMarkMode;
  /** Core diameter, e.g. "2.5rem". Falls back to the CSS default. */
  size?: string;
  /** Aura reach beyond the core, e.g. "2.4rem". Falls back to the CSS default. */
  halo?: string;
  className?: string;
};

const MODE_CLASS: Record<SpiritMarkMode, string> = {
  idle: "",
  starting: " is-starting",
  listening: " is-listening",
  degraded: " is-degraded",
  reconnecting: " is-reconnecting",
};

/**
 * The runtime listening indicator — the kodama spirit-mark. Decorative
 * (aria-hidden); pair it with a visible text label reflecting the same state
 * so it isn't conveyed by color and motion alone.
 */
export function SpiritMark({ mode, size, halo, className }: Props) {
  const style: CSSProperties = {
    ...(size ? { "--mark-size": size } : {}),
    ...(halo ? { "--halo-spread": halo } : {}),
  } as CSSProperties;

  return (
    <span
      className={`spirit-mark${MODE_CLASS[mode]}${className ? ` ${className}` : ""}`}
      style={style}
      aria-hidden="true"
    >
      <span className="spirit-mark__aura">
        <span className="spirit-mark__bloom" />
      </span>
      <span className="spirit-mark__core" />
    </span>
  );
}
