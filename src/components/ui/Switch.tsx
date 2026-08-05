import { clsx } from "clsx";

/**
 * A boolean whose state is its geometry.
 *
 * `role="switch"` rather than a checkbox: the platform semantics for "this
 * takes effect immediately" differ from "this is one of several things you are
 * about to submit", and every switch in this app writes the moment it is
 * pressed.
 *
 * Hand-rolled rather than `@base-ui/react`'s Switch, and the reason is the
 * `busy` contract below rather than a preference: base-ui's switch is a real
 * `<input>` under the hood, and the one thing this control must never do is
 * take the native `disabled` attribute. Adopting it is the same conversation
 * `Select` is waiting on (.claude/rules/typescript-style.md, "installed is not
 * adopted"), not a side effect of a settings ticket.
 *
 * WHAT CARRIES THE STATE IS THE KNOB, not the track. The on-track fill is a
 * supporting cue that does not clear 3:1 against the panel it sits on at any
 * alpha quiet enough to belong there; the knob is ink on that track at ~15:1,
 * and it moves 18px, which is a distance you can see at a glance down a column
 * of rows (docs/DESIGN_SYSTEM.md §6). That is also why the knob's travel is
 * gated by DURATION alone under reduced motion, never suppressed: position is
 * the readout, so it must still arrive — just at once rather than over 180ms.
 */
type Props = {
  /** The accessible name. There is no visible label inside the control, so
   * this must be the words next to it, verbatim: what you read is what a
   * voice-control tool can say. */
  label: string;
  checked: boolean;
  onChange: (next: boolean) => void;
  /**
   * A write is in flight. `aria-disabled` + `aria-busy`, never the native
   * `disabled` attribute: this control is the thing the user just pressed, so
   * it is the thing that holds focus, and `disabled` would blur it and reset
   * focus to <body> for the length of every save (docs/DESIGN_SYSTEM.md §6).
   * It stays focusable and declines its own activation instead.
   *
   * There is deliberately no genuine-disable prop. A switch that has nothing
   * to switch is a row that should not be on the screen.
   */
  busy?: boolean;
};

export function Switch({ label, checked, onChange, busy = false }: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-disabled={busy || undefined}
      // aria-busy says *why* it went inert; aria-disabled says that it did.
      aria-busy={busy || undefined}
      onClick={() => {
        if (busy) return;
        onChange(!checked);
      }}
      className={clsx(
        "focus-ring relative h-[22px] w-[38px] flex-none rounded-pill",
        // `scale`, not `transform`: Tailwind v4's `scale-*` sets the standalone
        // property, so naming `transform` here would animate nothing. Same trap
        // as Button.tsx's press, and as the knob's `translate` below.
        "transition-[scale,background-color] duration-140 ease-out-strong",
        // The press moves the WHOLE track, so the knob and its travel scale as
        // one object rather than the knob swimming inside a shrinking pill.
        // The reduced-motion swap repeats the guard because `:not()` takes its
        // argument's specificity — an unguarded swap loses the cascade and is
        // dead in the markup (the note in Button.tsx spells this out).
        "not-aria-disabled:active:scale-97",
        "motion-reduce:not-aria-disabled:active:scale-100",
        "aria-disabled:cursor-not-allowed",
        checked ? "bg-switch-on" : "bg-wash-hover",
      )}
    >
      <span
        aria-hidden="true"
        className={clsx(
          "absolute top-0.5 left-0.5 size-4 rounded-pill",
          // 18px is the geometry, not a round number: 38 track - 16 knob - 2
          // insets. `translate`, not `transform`, for the v4 reason above.
          //
          // Only the DURATION is gated. The knob must arrive at the far inset
          // under reduced motion — it is the state — so stillness makes the
          // trip instant rather than cancelling it. The track's colour above
          // keeps its real timing either way: a colour change is not motion.
          "transition-[translate] duration-180 ease-out-strong motion-reduce:duration-0",
          checked ? "translate-x-[18px] bg-ink" : "translate-x-0 bg-ink-dim",
        )}
      />
    </button>
  );
}
