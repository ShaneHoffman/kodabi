import { clsx } from "clsx";
import { useId, type InputHTMLAttributes, type ReactNode } from "react";

type Props = Omit<
  InputHTMLAttributes<HTMLInputElement>,
  "id" | "type" | "onChange"
> & {
  label: ReactNode;
  checked: boolean;
  /** Receives the new checked value, not the event — the caller almost always
   * wants the boolean. */
  onChange: (checked: boolean) => void;
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** Quiet helper text below the control, bound via aria-describedby. */
  hint?: ReactNode;
  /**
   * Renders the label to screen readers only.
   *
   * For a row that already prints the words beside the box, where a second
   * visible label would be a duplicate but the control still needs a name.
   * Named and shaped after `Select`'s, whose reasoning is the same.
   */
  hideLabel?: boolean;
  /**
   * Inert because a write is in flight.
   *
   * The same contract as `Button`'s `loading` and `Select`'s `busy`, for the
   * same reason: the native `disabled` attribute blurs a focused element when
   * it becomes disabled, so a person who ticked this box with the keyboard
   * would lose their place every time the write round-trips. A busy box takes
   * `aria-busy` + `aria-disabled`, stays focusable, and swallows its own
   * change. `disabled` wins if both are passed.
   */
  busy?: boolean;
};

/**
 * A labelled boolean control. A real `<input type="checkbox">` under a Grove
 * skin (appearance: none), so keyboard, form semantics and screen-reader state
 * come from the platform rather than from re-implemented ARIA.
 *
 * The checked state reads through value, not hue: the mark is ink, never the
 * reserved green, which means "audio is being recorded" and nothing else
 * (docs/UI_CONVENTIONS.md).
 *
 * UNCHECKED IS A RING, NOT A BOX. It carries no fill at all, so a column of
 * action items reads as a column of labels with a mark beside each rather than
 * a column of empty containers — space instead of boxes (docs/DESIGN.md). The
 * ring is `ink-faint` rather than `edge` precisely because it is the whole
 * affordance: it is the only thing marking the control, so it has to clear the
 * contrast floor on its own, and `.hc` promotes that step for free.
 */
export function Checkbox({
  label,
  checked,
  onChange,
  id,
  hint,
  hideLabel = false,
  busy = false,
  disabled = false,
  className = "",
  ...rest
}: Props) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = hint ? `${inputId}-hint` : undefined;
  const isBusy = busy && !disabled;

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-2">
        <input
          type="checkbox"
          id={inputId}
          checked={checked}
          disabled={disabled}
          // aria-busy says *why* it went inert; aria-disabled says that it did.
          // Neither is the native attribute, so focus and the tab order stay
          // exactly where the user left them.
          aria-busy={isBusy || undefined}
          aria-disabled={isBusy || undefined}
          aria-describedby={hintId}
          onChange={(event) => {
            if (isBusy) return;
            onChange(event.target.checked);
          }}
          className={clsx(
            "grid size-[17px] flex-none cursor-pointer appearance-none place-content-center rounded-[4px] bg-transparent",
            // 1.4px rather than a hairline: the ring is the whole control, and
            // there is no fill behind it doing half the work.
            "shadow-[0_0_0_1.4px_var(--color-ink-faint)]",
            // The same clock and curve as Button's press. `scale` is the
            // property, not a transform function, so the transition has to
            // name it or the press lands as a snap (see Button.tsx).
            "transition-[scale,background-color,box-shadow] duration-140 ease-out-strong",
            // Hover firms the ring one step up the ink ladder, spent on the
            // only mark the control has.
            "not-disabled:not-checked:hover:shadow-[0_0_0_1.4px_var(--color-ink-dim)]",
            // The press: one step firmer again, plus the app's one press
            // scale. At 3% on a 17px box the scale is sub-pixel, so the RING
            // still carries the press; the scale is what makes it the same
            // press every other control performs (DESIGN_SYSTEM §2).
            //
            // `not-checked` is load-bearing, not symmetry with the hover above:
            // a checked box has no ring to firm, and an unguarded press rule
            // would OUTRANK `checked:shadow-none` below — `:not()` takes its
            // argument's specificity, so (0,3,0) beats (0,2,0) — and paint a
            // ring around the ink fill while the box is also scaling down.
            "not-disabled:not-checked:active:shadow-[0_0_0_1.4px_var(--color-ink-read)]",
            "not-disabled:not-aria-disabled:active:scale-97",
            // The swap repeats the guard on purpose: `:not()` takes its
            // argument's specificity, so an unguarded stillness rule loses the
            // cascade and would be dead in the markup (see Button.tsx).
            "motion-reduce:not-disabled:not-aria-disabled:active:scale-100",
            // Checked is an ink fill with a ground-coloured glyph — value, not
            // hue. The reserved green is never spent here.
            "checked:bg-ink checked:shadow-none",
            "checked:not-disabled:active:bg-ink-read",
            // A check drawn from two borders, rotated — no icon font, no SVG.
            // The transform is an arbitrary PROPERTY rather than Tailwind's
            // split `rotate-*`/`translate-*` utilities: those apply in a fixed
            // order (translate, then rotate), which would spend the 1px nudge
            // in unrotated coordinates and land the glyph off-centre.
            "before:h-[0.3rem] before:w-[0.55rem] before:border-b-2 before:border-l-2 before:border-ground before:opacity-0 before:content-['']",
            "before:[transform:rotate(-45deg)_translate(1px,-1px)]",
            "checked:before:opacity-100",
            // The box has no text to fade, so it dims instead — the one
            // sanctioned opacity fade in the system (DESIGN_SYSTEM §2).
            "disabled:cursor-not-allowed disabled:opacity-50",
            // The busy pulse is opacity-only, so it is correct unpaired under
            // reduced motion: movement is the accessibility problem, life is
            // not (docs/DESIGN_SYSTEM.md §4). The guards repeat `aria-disabled`
            // for the same specificity reason `not-checked` does above: an
            // unguarded press rule would outrank the stillness it swaps.
            "aria-disabled:cursor-progress aria-disabled:animate-pending",
            "focus-ring",
            className,
          )}
          {...rest}
        />
        <label
          htmlFor={inputId}
          className={
            hideLabel ? "sr-only" : "font-ui text-[12.5px] text-ink-read"
          }
        >
          {label}
        </label>
      </div>
      {hint && (
        // Indented under the LABEL rather than the box, so the text column
        // reads as one block. 25px restates the box's own 17px plus the 8px
        // gap beside it; it tracks the box, not the spacing rhythm.
        //
        // Faint by decision: DESIGN_SYSTEM §6 settles the control hint as a
        // metadata-register occupant (a sentence the user MAY read, bound as a
        // description), bounded by length — a paragraph here would be prose.
        <p
          id={hintId}
          className="pl-[25px] font-ui text-[11.5px] leading-relaxed text-ink-faint"
        >
          {hint}
        </p>
      )}
    </div>
  );
}
