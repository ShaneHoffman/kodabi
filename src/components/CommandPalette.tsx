import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useCommands, type Command } from "../useCommands";
import { useFilteredCommands } from "../useFilteredCommands";
import { useNavigation } from "../useNavigation";
import "./CommandPalette.css";

type Props = {
  onClose: () => void;
};

const LISTBOX_ID = "command-palette-listbox";

/** Option ids are index-based — command ids may hold slashes and spaces. */
function optionId(index: number): string {
  return `command-palette-option-${index}`;
}

/**
 * The primary navigation surface (FOUNDING_DOC §4), hand-rolled as a
 * combobox: focus never leaves the input (Tab is held); ↑/↓ move a virtual
 * highlight via aria-activedescendant, Enter runs it, Escape closes.
 */
export function CommandPalette({ onClose }: Props) {
  const { navigate } = useNavigation();
  const commands = useCommands();
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  // Whether the current pointer gesture started on the backdrop itself —
  // a press that starts inside the panel must never dismiss, even if it
  // ends (or retargets, via common-ancestor click) on the backdrop.
  const backdropPressed = useRef(false);
  // Last pointer position over the list: scrolling shifts rows under a
  // stationary cursor and Chromium re-fires boundary/move events, which
  // must not yank the keyboard highlight to whatever lands under the mouse.
  const lastPointer = useRef<{ x: number; y: number } | null>(null);

  const filtered = useFilteredCommands(commands, query);

  // When nothing matches a non-empty query, the query itself becomes the
  // command — a synthetic search row, so typed text always leads somewhere.
  const rows: Command[] = useMemo(() => {
    const trimmed = query.trim();
    if (filtered.length === 0 && trimmed) {
      return [
        {
          id: "search-fallback",
          title: `Search for “${trimmed}”`,
          run: () => navigate({ kind: "search", query: trimmed }),
        },
      ];
    }
    return filtered;
  }, [filtered, query, navigate]);

  const active = rows.length > 0 ? Math.min(activeIndex, rows.length - 1) : 0;

  // Focus the input on open; on close hand focus back to wherever it came
  // from — unless a run command unmounted that element in the meantime.
  useEffect(() => {
    const previous = document.activeElement;
    inputRef.current?.focus();
    return () => {
      if (previous instanceof HTMLElement && previous.isConnected) {
        previous.focus();
      }
    };
  }, []);

  // Keep the keyboard highlight visible when it walks past the list's edge.
  useEffect(() => {
    document.getElementById(optionId(active))?.scrollIntoView({ block: "nearest" });
  }, [active, rows]);

  const runCommand = (command: Command) => {
    command.run();
    onClose();
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    // Keys pressed mid-IME-composition belong to the composition (committing
    // with Enter, cancelling with Escape), never to the palette.
    if (event.nativeEvent.isComposing) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex(Math.min(active + 1, rows.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex(Math.max(active - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = rows[active];
      if (command) runCommand(command);
    } else if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "Tab") {
      // The dialog's only tabbable element is this input — hold focus here
      // so Tab can't reach controls hidden behind the overlay (aria-modal).
      event.preventDefault();
    }
  };

  return (
    // Backdrop dismiss happens on click, not pointerdown: unmounting the
    // overlay at pointerdown lets the rest of the gesture fall through to
    // whatever sat underneath (the unmount flushes before mousedown), so a
    // dismissing click would also press sidebar controls. The opening
    // gesture predates the overlay, so it can never self-close.
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-bg-sink/60 px-md pt-2xl"
      onPointerDown={(event) => {
        backdropPressed.current = event.target === event.currentTarget;
      }}
      onClick={(event) => {
        if (backdropPressed.current && event.target === event.currentTarget) {
          onClose();
        }
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        className="command-palette__panel w-full max-w-measure overflow-hidden rounded-md bg-surface"
      >
        <input
          ref={inputRef}
          role="combobox"
          aria-expanded="true"
          aria-controls={LISTBOX_ID}
          aria-activedescendant={rows.length > 0 ? optionId(active) : undefined}
          aria-label="Type a command or search"
          placeholder="Type a command or search…"
          autoComplete="off"
          spellCheck={false}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
          }}
          onKeyDown={onKeyDown}
          className="command-palette__input w-full bg-surface px-4 py-3 text-body text-text placeholder:text-text-faint"
        />
        <ul
          id={LISTBOX_ID}
          role="listbox"
          aria-label="Commands"
          className="max-h-80 overflow-y-auto py-2"
        >
          {rows.map((command, index) => (
            <li
              key={command.id}
              id={optionId(index)}
              role="option"
              aria-selected={index === active}
              onPointerDown={(event) => event.preventDefault()}
              onMouseMove={(event) => {
                const moved =
                  lastPointer.current?.x !== event.clientX ||
                  lastPointer.current?.y !== event.clientY;
                lastPointer.current = { x: event.clientX, y: event.clientY };
                if (moved && index !== active) setActiveIndex(index);
              }}
              onClick={() => runCommand(command)}
              className={`command-palette__row flex items-baseline justify-between px-4 py-2 text-body text-text-soft${
                index === active ? " is-active" : ""
              }`}
            >
              <span>{command.title}</span>
              {command.hint && (
                <span className="text-cap text-text-faint">{command.hint}</span>
              )}
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
