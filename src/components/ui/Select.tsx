import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
} from "react";
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
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, [open]);

  // Keep the highlighted option in view as it walks past the list's edge.
  useEffect(() => {
    if (!open) return;
    document
      .getElementById(`${baseId}-option-${active}`)
      ?.scrollIntoView({ block: "nearest" });
  }, [open, active, baseId]);

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

    if (!open) {
      if (key === "ArrowDown" || key === "ArrowUp" || key === "Enter" || key === " ") {
        event.preventDefault();
        openList();
      } else if (isPrintable(event)) {
        openList();
        typeahead(key);
      }
      return;
    }

    if (key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex(Math.min(active + 1, options.length - 1));
    } else if (key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex(Math.max(active - 1, 0));
    } else if (key === "Home") {
      event.preventDefault();
      setActiveIndex(0);
    } else if (key === "End") {
      event.preventDefault();
      setActiveIndex(options.length - 1);
    } else if (key === "Enter" || key === " ") {
      event.preventDefault();
      choose(active);
    } else if (key === "Escape") {
      event.preventDefault();
      close();
    } else if (key === "Tab") {
      // Let focus leave, but collapse the list behind it.
      setOpen(false);
    } else if (isPrintable(event)) {
      typeahead(key);
    }
  };

  const selectedLabel = selectedIndex >= 0 ? options[selectedIndex].label : null;

  return (
    <div ref={rootRef} className="relative flex flex-col gap-2xs">
      <span id={labelId} className="text-cap text-text-soft">
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
        aria-activedescendant={open ? optionId(active) : undefined}
        onClick={() => (open ? setOpen(false) : openList())}
        onKeyDown={onKeyDown}
        className="ui-select__trigger flex w-full items-center justify-between rounded-md bg-surface px-xs py-2xs text-body text-text"
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
          className="ui-select__list absolute inset-x-0 top-full z-10 mt-3xs max-h-64 overflow-y-auto rounded-md bg-surface py-2xs"
        >
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
                index === active ? " is-active" : ""
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
