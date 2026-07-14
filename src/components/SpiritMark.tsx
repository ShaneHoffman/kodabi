import type { CSSProperties } from "react";
import type { CapturePhase } from "../useCaptureState";
import "./SpiritMark.css";

type Props = {
  phase: CapturePhase;
  /** Core diameter, e.g. "2.5rem". Falls back to the CSS default. */
  size?: string;
  /** Aura reach beyond the core, e.g. "2.4rem". Falls back to the CSS default. */
  halo?: string;
  className?: string;
};

/**
 * The runtime listening indicator — the kodama spirit-mark. Decorative
 * (aria-hidden); pair it with a visible text label reflecting `phase` so
 * the on-air state isn't conveyed by color and motion alone.
 */
export function SpiritMark({ phase, size, halo, className }: Props) {
  const style: CSSProperties = {
    ...(size ? { "--mark-size": size } : {}),
    ...(halo ? { "--halo-spread": halo } : {}),
  } as CSSProperties;

  return (
    <span
      className={`spirit-mark${phase === "listening" ? " is-listening" : ""}${
        className ? ` ${className}` : ""
      }`}
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
