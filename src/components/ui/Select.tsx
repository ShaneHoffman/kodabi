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
   *   boxed — a form field, and reads like one. The default.
   *   quiet — an affordance sitting beside content it must not out-weigh (the
   *           Inbox's per-row "File to…"). Rests as text, becomes a real
   *           anchored control on hover and while open.
   *
   * Never `quiet` inside a form: a field that does not look like a field is a
   * field people do not fill in (docs/UI_CONVENTIONS.md).
   */
  variant?: "boxed" | "quiet";
};

/**
 * A token-styled dropdown, hand-rolled as a collapsible listbox
 * (WAI-ARIA active-descendant) rather than reaching for a headless
 * dependency — the same combobox know-how the command palette proves
 * (src/components/CommandPalette.tsx), minus the dep. Focus stays on the
 * trigger; ↑/↓ move a virtual highlight via aria-activedescendant,
 * Enter/Space selects, Escape closes, typing jumps. The active row is a
 * value wash, never the reserved green (docs/DESIGN.md).
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
        // Boxed fills its column and pushes the caret to the far edge, the way
        // a form field should. Quiet shrink-wraps instead: with no ring holding
        // the two ends together, a full-width trigger left its label and its
        // caret marooned at opposite sides of the column, reading as two
        // unrelated scraps of text rather than one control.
        className={`ui-select__trigger ui-focus-ring flex items-center rounded-md px-xs py-2xs text-body disabled:cursor-not-allowed disabled:text-text-faint ${
          variant === "quiet"
            ? "ui-select__trigger--quiet w-auto self-start gap-2xs text-text-soft"
            : "w-full justify-between bg-surface text-text"
        }`}
      >
        <span className={selectedLabel === null ? "text-text-faint" : undefined}>
          {selectedLabel ?? placeholder}
        </span>
        <svg
          className="ui-select__caret"
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
          className="ui-select__list absolute inset-x-0 top-full mt-3xs max-h-64 overflow-y-auto rounded-md bg-surface py-2xs"
        >
          {options.length === 0 && (
            // Not a role=option: there is nothing to choose, so it must not be
            // reachable by the arrow keys or announced as selectable.
            <li className="px-xs py-2xs text-cap text-text-faint">{emptyLabel}</li>
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
              className={`ui-select__option flex items-center justify-between px-xs py-2xs text-body text-text-soft${
                index === active ? " ui-wash is-active" : ""
              }${index === selectedIndex ? " is-selected" : ""}`}
            >
              <span>{option.label}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
