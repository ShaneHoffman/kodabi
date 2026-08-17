import { clsx } from "clsx";
import {
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { useOutsidePointerDown } from "../../useOutsidePointerDown";
import { useScrollIntoView } from "../../useScrollIntoView";
import { menuRow } from "./Menu";

export type SelectOption = { value: string; label: string };

type Props = {
  label: ReactNode;
  value: string | null;
  onChange: (value: string) => void;
  options: SelectOption[];
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /** What the trigger reads when nothing is chosen. */
  placeholder?: string;
  /** Hide the label visually (still an accessible name) — for a control whose
   * purpose is clear from context, e.g. a per-row picker in a list. */
  hideLabel?: boolean;
  /**
   * A genuine disable: there is nothing here to choose. Uses the native
   * attribute, which takes the control out of the tab order.
   *
   * NOT for a write in flight — that is `busy`. See below.
   */
  disabled?: boolean;
  /**
   * Inert because a write is in flight.
   *
   * The same contract as `Button`'s `loading`, and it exists for the same
   * reason: the native `disabled` attribute blurs an element that is focused
   * when it becomes disabled, and focus resets to <body> (the HTML focus
   * fixup rule). A user who changes a setting with the keyboard therefore
   * loses their place in the page every time the save round-trips — and
   * inside a modal it is worse, because the dialog's Escape and Tab handling
   * lives on an ancestor the focus has just left.
   *
   * So a busy trigger takes `aria-disabled` + `aria-busy`, stays focusable,
   * and swallows its own activation here. `disabled` wins if both are passed,
   * because a caller asking for a genuinely inert control means it — and the
   * focus goes with it (docs/DESIGN_SYSTEM.md §6).
   */
  busy?: boolean;
  /** What the list says when there are no options. It opens and says this
   * rather than refusing to open, so the control never looks broken. */
  emptyLabel?: string;
};

/**
 * The app's dropdown, hand-rolled as a collapsible listbox (WAI-ARIA
 * active-descendant) rather than reaching for a headless dependency — the same
 * combobox know-how the command palette proves
 * (src/components/overlays/CommandPalette.tsx), minus the dep. Focus stays on
 * the trigger; ↑/↓ move a virtual highlight via aria-activedescendant,
 * Enter/Space selects, Escape closes, typing jumps.
 *
 * Grove re-skinned the chrome and left every line of that behaviour alone. The
 * trigger is a glass value-button and the list wears `Menu`'s material and
 * `Menu`'s rows, so the two popups in the app are one surface; what did NOT
 * happen here is the swap to `@base-ui/react`, which is its own ticket and its
 * own conversation (.claude/rules/typescript-style.md).
 *
 * The second variant went with the re-skin. `token` was a demoted picker for an
 * Inbox row that now files through `Menu`, so it had no caller left and no
 * Grove chrome was invented for it.
 *
 * The active row is the menu-hover wash, never the reserved green
 * (docs/DESIGN_SYSTEM.md §2).
 */
export function Select({
  label,
  value,
  onChange,
  options,
  id,
  placeholder = "Select…",
  hideLabel = false,
  disabled = false,
  busy = false,
  emptyLabel = "Nothing to choose yet.",
}: Props) {
  const generatedId = useId();
  const baseId = id ?? generatedId;
  const labelId = `${baseId}-label`;
  const listboxId = `${baseId}-listbox`;
  const optionId = (index: number) => `${baseId}-option-${index}`;

  const selectedIndex = options.findIndex((option) => option.value === value);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(() =>
    selectedIndex >= 0 ? selectedIndex : 0,
  );

  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  // Scrolling shifts rows under a stationary cursor and Chromium re-fires
  // move events; only a real pointer move may steal the keyboard highlight
  // (the command palette's guard).
  const lastPointer = useRef<{ x: number; y: number } | null>(null);
  // Typeahead buffer, cleared after a short idle.
  const typed = useRef("");
  const typedTimer = useRef<number | null>(null);

  const active = options.length > 0 ? Math.min(activeIndex, options.length - 1) : 0;
  // Busy, not disabled — an explicit `disabled` wins. Mirrors Button.tsx.
  const isBusy = busy && !disabled;

  // Opens even with no options: the list then says so. Refusing to open left a
  // trigger that swallowed every click and looked broken.
  const openList = () => {
    setActiveIndex(selectedIndex >= 0 ? selectedIndex : 0);
    setOpen(true);
  };

  const close = () => {
    setOpen(false);
    triggerRef.current?.focus();
  };

  const choose = (index: number) => {
    const option = options[index];
    if (option) onChange(option.value);
    close();
  };

  // Close on an outside pointer press while open.
  useOutsidePointerDown(open, rootRef, () => setOpen(false));

  // Keep the highlighted option in view as it walks past the list's edge.
  useScrollIntoView(open ? optionId(active) : null);

  const typeahead = (char: string) => {
    if (typedTimer.current !== null) window.clearTimeout(typedTimer.current);
    typed.current += char.toLowerCase();
    const buffer = typed.current;
    const match = options.findIndex((option) =>
      option.label.toLowerCase().startsWith(buffer),
    );
    if (match >= 0) setActiveIndex(match);
    typedTimer.current = window.setTimeout(() => {
      typed.current = "";
      typedTimer.current = null;
    }, 500);
  };

  const isPrintable = (event: ReactKeyboardEvent) =>
    event.key.length === 1 && !event.metaKey && !event.ctrlKey && !event.altKey;

  const onKeyDown = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    // Keys pressed mid-IME-composition belong to the composition.
    if (event.nativeEvent.isComposing) return;
    // A busy trigger keeps focus and the tab order, so it still receives keys
    // and has to decline them itself. Tab is deliberately not swallowed: the
    // whole point of staying focusable is that focus can still move on.
    if (isBusy && event.key !== "Tab") {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    const { key } = event;

    // A key the widget acts on is fully consumed — stopPropagation keeps it
    // from reaching an ancestor (a dialog that closes on Escape, a form that
    // submits on Enter). Tab is the one exception: it must bubble so focus
    // can leave the trigger.
    if (!open) {
      if (key === "ArrowDown" || key === "ArrowUp" || key === "Enter" || key === " ") {
        event.preventDefault();
        event.stopPropagation();
        openList();
      } else if (isPrintable(event)) {
        event.stopPropagation();
        openList();
        typeahead(key);
      }
      return;
    }

    if (key === "ArrowDown") {
      event.preventDefault();
      event.stopPropagation();
      setActiveIndex(Math.min(active + 1, options.length - 1));
    } else if (key === "ArrowUp") {
      event.preventDefault();
      event.stopPropagation();
      setActiveIndex(Math.max(active - 1, 0));
    } else if (key === "Home") {
      event.preventDefault();
      event.stopPropagation();
      setActiveIndex(0);
    } else if (key === "End") {
      event.preventDefault();
      event.stopPropagation();
      setActiveIndex(options.length - 1);
    } else if (key === "Enter" || key === " ") {
      event.preventDefault();
      event.stopPropagation();
      choose(active);
    } else if (key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (key === "Tab") {
      // Let focus leave, but collapse the list behind it.
      setOpen(false);
    } else if (isPrintable(event)) {
      event.stopPropagation();
      typeahead(key);
    }
  };

  const selectedLabel = selectedIndex >= 0 ? options[selectedIndex].label : null;

  return (
    <div
      ref={rootRef}
      // `ui-select` carries the anchor-scope that keeps each instance's menu on
      // its OWN trigger; `relative` is the containing block the un-anchored
      // fallback still needs (src/index.css §3, the anchored block and the
      // utilities on the list below it).
      className={clsx("ui-select relative flex flex-col", !hideLabel && "gap-1.5")}
    >
      <span
        id={labelId}
        className={
          hideLabel ? "sr-only" : "font-ui text-[11.5px] font-medium text-ink-dim"
        }
      >
        {label}
      </span>
      <button
        ref={triggerRef}
        type="button"
        id={baseId}
        role="combobox"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listboxId}
        aria-labelledby={`${labelId} ${baseId}`}
        aria-activedescendant={open && options.length > 0 ? optionId(active) : undefined}
        disabled={disabled}
        aria-disabled={isBusy || undefined}
        // aria-busy says *why* it went inert; aria-disabled says that it did.
        aria-busy={isBusy || undefined}
        onClick={(event) => {
          if (isBusy) {
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          if (open) setOpen(false);
          else openList();
        }}
        onKeyDown={onKeyDown}
        // It shrink-wraps to its value: this is a chip, and a chip that
        // stretched to its column would be a field pretending to be a chip.
        //
        // The press is Button's — scale 97 over 140ms, with the matched
        // reduced-motion guard `:not()` specificity demands — and it carries
        // one extra exclusion nothing else in the app needs. NOT while the menu
        // is open: an open list is anchored to this trigger, and CSS anchor
        // positioning reads the anchor's TRANSFORMED border box live, so
        // scaling it would drag the whole menu sideways. The element is a chip
        // being pressed or it is an anchor, never both. `aria-expanded` also
        // takes the hover fill, so it stays visibly the thing the list hangs
        // off while it cannot move.
        className={clsx(
          "ui-select__trigger focus-ring inline-flex w-auto select-none items-center gap-2 self-start",
          "rounded-button border border-edge bg-wash px-3 py-2",
          "font-ui text-[13px] font-medium text-ink",
          "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "transition-[scale,background-color,border-color,color] duration-140 ease-out-strong",
          "not-disabled:not-aria-disabled:hover:bg-wash-hover",
          "aria-expanded:bg-wash-hover",
          "not-disabled:not-aria-disabled:not-aria-expanded:active:scale-97",
          "motion-reduce:not-disabled:not-aria-disabled:not-aria-expanded:active:scale-100",
          "disabled:cursor-not-allowed disabled:text-ink-faint",
          "aria-disabled:cursor-not-allowed aria-disabled:text-ink-faint",
        )}
      >
        <span className={selectedLabel === null ? "text-ink-faint" : undefined}>
          {selectedLabel ?? placeholder}
        </span>
        {/* Functional affordance, not decoration: it scales with the type and
            rests one step down the ink ladder. */}
        <svg
          className="size-[0.7em] flex-none text-ink-faint"
          viewBox="0 0 12 12"
          aria-hidden="true"
        >
          <path
            d="M2.5 4.5 6 8l3.5-3.5"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.25"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </button>
      {open && (
        <ul
          id={listboxId}
          role="listbox"
          aria-labelledby={labelId}
          // Menu's material, so the app has one popup surface rather than two
          // that nearly match. The placement here is the FALLBACK — right
          // aligned under the trigger, which is where every trigger that opens
          // one sits — and the anchored block in src/index.css §3 overrides it
          // where the engine supports anchor positioning.
          //
          // The entrance is materialize, the same 220ms every other Grove
          // surface arrives on. There is deliberately NO exit: the list is
          // conditionally rendered, so closing unmounts it — which is what the
          // ARIA wiring, both bridge hooks and every "the list is gone"
          // assertion in Select.test.tsx rest on — and a fade-out would mean
          // keeping a listbox mounted at all times just to transition
          // `display`. A dismissal is allowed to be instant.
          className={clsx(
            "ui-select__list glass-overlay absolute top-full right-0 z-50 mt-2",
            "max-h-64 min-w-56 origin-top-right overflow-y-auto p-1.5 outline-hidden",
            "animate-materialize motion-reduce:animate-fade-in",
          )}
        >
          {options.length === 0 && (
            // Not a role=option: there is nothing to choose, so it must not be
            // reachable by the arrow keys or announced as selectable.
            <li className="px-2.5 py-2 text-[11.5px] text-ink-faint">{emptyLabel}</li>
          )}
          {options.map((option, index) => (
            <li
              key={option.value}
              id={optionId(index)}
              role="option"
              aria-selected={index === selectedIndex}
              onPointerDown={(event) => event.preventDefault()}
              onMouseMove={(event) => {
                const moved =
                  lastPointer.current?.x !== event.clientX ||
                  lastPointer.current?.y !== event.clientY;
                lastPointer.current = { x: event.clientX, y: event.clientY };
                if (moved && index !== active) setActiveIndex(index);
              }}
              onClick={() => choose(index)}
              // `data-highlighted` rather than a class of this file's own: it
              // is the attribute Menu's row recipe already keys its wash off,
              // and one attribute for two things — the pointer is over the row,
              // or the keyboard has walked to it — is exactly what keeps a list
              // from ever showing two rows lit at once. The highlight is driven
              // from JS here (onMouseMove sets the active index) rather than
              // from :hover, for that same reason.
              data-highlighted={index === active ? "" : undefined}
              // The chosen row reads through weight and a check — value, never
              // hue — and menuRow is already at font-medium, so the check is
              // the whole mark. What this call site adds is layout and nothing
              // else: `cursor` is the recipe's, resolving to the same arrow a
              // menu row shows, and a `cursor-pointer` here would have been
              // settled by emission order rather than by this className
              // (UI_CONVENTIONS §4).
              className={clsx(menuRow(), "justify-between")}
            >
              <span>{option.label}</span>
              {index === selectedIndex && (
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 14 14"
                  fill="none"
                  aria-hidden="true"
                  // `dim`, not `faint`. This glyph is drawn on the row it
                  // marks, and that row is very often the highlighted one,
                  // where the ground is the wash rather than the overlay plane:
                  // faint ink on that fill lands under even the 3:1 a graphic
                  // is held to (docs/DESIGN_SYSTEM.md §6).
                  className="flex-none text-ink-dim"
                >
                  <path
                    d="M2.5 7.5l3 3 6-7"
                    stroke="currentColor"
                    strokeWidth="1.5"
                    strokeLinecap="round"
                    strokeLinejoin="round"
                  />
                </svg>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
