import type { CSSProperties } from "react";

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

/**
 * How the mark spends its motion while listening. `halo` is the default: a
 * soft field of ma around the core, for surfaces where the mark is a presence
 * (the capture windows). `ring` trades that field for a single crisp pulse
 * leaving the core, for chrome where the mark reads as an instrument metering
 * the capture rather than a creature sitting in it.
 */
export type SpiritMarkVariant = "halo" | "ring";

type Props = {
  mode: SpiritMarkMode;
  variant?: SpiritMarkVariant;
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
 *
 * Its material lives in the spirit-mark block of src/index.css §3 (the lobes
 * are pseudo-elements, which take no utility class); the `is-*` classes below
 * are that block's other half.
 */
export function SpiritMark({
  mode,
  variant = "halo",
  size,
  halo,
  className,
}: Props) {
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
      {variant === "ring" ? (
        // Only while listening: degraded drops its field for the same reason
        // the halo variant does, and the ink modes never pulse outward. The
        // ring is a plain bordered circle, so it stays utilities-only — no
        // rule of its own in index.css, where unlayered CSS would outrank
        // them. `spirit-mark__ring` is a test hook, not a style.
        mode === "listening" && (
          // `motion-reduce:scale-[1.7]` is load-bearing, not decoration. The
          // swap partner is opacity-only, and this span's box IS the core's
          // box — so held at rest the ring sits exactly under an opaque disc
          // and the reduced mark would show nothing at all moving. The lobes
          // settle to a resting SHAPE for the same reason; this is the
          // resting RADIUS, parked midway through the ring's travel (1 → 2.4).
          <span className="spirit-mark__ring pointer-events-none absolute inset-0 animate-ring rounded-full border border-kodama motion-reduce:animate-halo-still motion-reduce:scale-[1.7]" />
        )
      ) : (
        <span className="spirit-mark__aura">
          <span className="spirit-mark__bloom" />
        </span>
      )}
      <span className="spirit-mark__core" />
    </span>
  );
}
