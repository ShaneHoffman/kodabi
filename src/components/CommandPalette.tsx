import {
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useCommands, type Command } from "../useCommands";
import { useDialogFocus } from "../useDialogFocus";
import { useFilteredCommands } from "../useFilteredCommands";
import { useNavigation } from "../useNavigation";
import { useScrollIntoView } from "../useScrollIntoView";
import { Overlay } from "./ui/Overlay";
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
  useDialogFocus(() => inputRef.current);

  // Keep the keyboard highlight visible when it walks past the list's edge.
  // `rows` is the refresh key: re-filtering can swap what sits at a given
  // option id while the highlight stays put.
  useScrollIntoView(optionId(active), rows);

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
    <Overlay onDismiss={onClose} label="Command palette" className="overflow-hidden">
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
        className="command-palette__input w-full bg-surface px-sm py-xs text-body text-text placeholder:text-text-faint"
      />
      <ul
        id={LISTBOX_ID}
        role="listbox"
        aria-label="Commands"
        className="max-h-80 overflow-y-auto py-2xs"
      >
        {rows.length === 0 && (
          // Only reachable with an empty query and no commands at all, which
          // means the project listing failed — never a blank pane
          // (docs/DESIGN_SYSTEM.md §3).
          <li className="px-sm py-2xs text-body text-text-soft">
            No commands available yet.
          </li>
        )}
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
            className={`command-palette__row flex items-baseline justify-between px-sm py-2xs text-body text-text-soft${
              index === active ? " ui-wash" : ""
            }`}
          >
            <span>{command.title}</span>
            {command.hint && (
              <span className="text-cap text-text-faint">{command.hint}</span>
            )}
          </li>
        ))}
      </ul>
    </Overlay>
  );
}
