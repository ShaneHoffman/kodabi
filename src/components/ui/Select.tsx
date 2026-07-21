import {
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
import { useOutsidePointerDown } from "../../useOutsidePointerDown";
import { useScrollIntoView } from "../../useScrollIntoView";
import "./Select.css";

export type SelectOption = { value: string; label: string };

type Props = {
  label: ReactNode;
  value: string | null;
  onChange: (value: string) => void;
  options: SelectOption[];
  /** Optional explicit id; one is generated (useId) when omitted. */
  id?: string;
  /**
   * What the trigger reads when nothing is chosen. On `token` it is the verb
   * rather than a placeholder — the arrow after it is drawn by the control.
   */
  placeholder?: string;
  /** Hide the label visually (still an accessible name) — for a control whose
   * purpose is clear from context, e.g. a per-row picker in a list. */
  hideLabel?: boolean;
  /** Inert while a write is in flight, or when there is nothing to choose. */
  disabled?: boolean;
  /** What the list says when there are no options. It opens and says this
   * rather than refusing to open, so the control never looks broken. */
  emptyLabel?: string;
  /**
   * How much the resting control weighs.
   *
   *   boxed — a form or settings control, and reads like one: a raised chip
   *           carrying its value and a chevron. The default.
   *   token — an affordance sitting beside content it must not out-weigh (a
   *           queue row's "File →"). Rests as quiet mono text with no box at
   *           all and takes a soft pill only while the pointer or the
   *           keyboard is on it.
   *
   * Never `token` inside a form: a field that does not look like a field is a
   * field people do not fill in (docs/UI_CONVENTIONS.md).
   */
  variant?: "boxed" | "token";
};

/**
 * A token-styled dropdown, hand-rolled as a collapsible listbox
 * (WAI-ARIA active-descendant) rather than reaching for a headless
 * dependency — the same combobox know-how the command palette proves
 * (src/components/CommandPalette.tsx), minus the dep. Focus stays on the
 * trigger; ↑/↓ move a virtual highlight via aria-activedescendant,
 * Enter/Space selects, Escape closes, typing jumps.
 *
 * Both variants share one open list, on the overlay plane. The active row is
 * the menu-hover fill, never the reserved green (docs/DESIGN.md).
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
  emptyLabel = "Nothing to choose yet.",
  variant = "boxed",
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
  const token = variant === "token";

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
      className={`relative flex flex-col${hideLabel ? "" : " gap-2xs"}`}
    >
      <span
        id={labelId}
        className={hideLabel ? "sr-only" : "text-cap text-text-soft"}
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
        onClick={() => (open ? setOpen(false) : openList())}
        onKeyDown={onKeyDown}
        // Boxed shrink-wraps to its value: it is a chip, and a chip that
        // stretched to its column would be a field pretending to be a chip.
        // Token shrink-wraps too — with no ring holding its two ends together,
        // a full-width trigger left the label and the arrow marooned at
        // opposite sides of the column, reading as two unrelated scraps.
        className={`ui-select__trigger ui-select__trigger--${variant} ui-focus-ring flex w-auto items-center self-start disabled:cursor-not-allowed disabled:text-text-faint ${
          token
            ? "gap-2xs font-mono text-cap tracking-token text-text-faint"
            : "gap-2xs text-label text-text"
        }`}
      >
        <span
          className={
            selectedLabel === null && !token ? "text-text-faint" : undefined
          }
        >
          {selectedLabel ?? placeholder}
        </span>
        {token ? (
          // The arrow IS the state: → at rest, ↓ the moment the menu is under
          // it. A chevron would have said "there is a list here"; this says
          // "the list is open", which is the only thing worth saying twice.
          <span aria-hidden="true">{open ? "↓" : "→"}</span>
        ) : (
          <svg className="ui-select__caret" viewBox="0 0 12 12" aria-hidden="true">
            <path
              d="M2.5 4.5 6 8l3.5-3.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.25"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </button>
      {open && (
        <ul
          id={listboxId}
          role="listbox"
          aria-labelledby={labelId}
          className="ui-select__list absolute right-0 top-full mt-2xs max-h-64 overflow-y-auto"
        >
          {options.length === 0 && (
            // Not a role=option: there is nothing to choose, so it must not be
            // reachable by the arrow keys or announced as selectable.
            <li className="ui-select__option text-cap text-text-faint">{emptyLabel}</li>
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
              // Mono in a token menu, because what it lists are paths — the
              // filing destination is a location, and mono is how this app
              // writes locations everywhere else.
              className={`ui-select__option flex cursor-pointer items-center justify-between gap-2xs ${
                token ? "font-mono text-label" : "text-label"
              } ${index === active ? "ui-wash is-active" : "text-text-soft"}${
                index === selectedIndex ? " is-selected" : ""
              }`}
            >
              <span>{option.label}</span>
              {index === selectedIndex && (
                <svg
                  width="12"
                  height="12"
                  viewBox="0 0 14 14"
                  fill="none"
                  aria-hidden="true"
                  className="flex-none text-text-faint"
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
