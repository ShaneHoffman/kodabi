import {
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { COMMAND_GROUP_LABEL, useCommands, type Command } from "../../useCommands";
import { useDialogFocus } from "../../useDialogFocus";
import { useFilteredCommands } from "../../useFilteredCommands";
import { useNavigation } from "../../useNavigation";
import { useScrollIntoView } from "../../useScrollIntoView";
import { Overlay } from "../ui/Overlay";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; this overlay's Grove ticket deletes it
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
          group: "action",
          title: `Search for “${trimmed}”`,
          run: () => navigate({ kind: "search", query: trimmed }),
        },
      ];
    }
    return filtered;
  }, [filtered, query, navigate]);

  const active = rows.length > 0 ? Math.min(activeIndex, rows.length - 1) : 0;

  // Sections, carrying each row's index in the flat `rows` array so the option
  // ids (and so aria-activedescendant and the arrow keys) stay a single
  // sequence over the whole list, whatever it is broken into visually.
  //
  // Only while unfiltered. A filtered list is a set of matches, not a table of
  // contents, and a heading over it would be labelling a section that no longer
  // exists; the per-row hint takes over there instead.
  const sections = useMemo(() => {
    const numbered = rows.map((command, index) => ({ command, index }));
    if (query.trim()) return [{ label: null, rows: numbered }];
    return (["jump", "action"] as const)
      .map((group) => ({
        label: COMMAND_GROUP_LABEL[group],
        rows: numbered.filter(({ command }) => command.group === group),
      }))
      .filter((section) => section.rows.length > 0);
  }, [rows, query]);

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

  // Bound to the PANEL, not to the input, and it reaches the input's keys all
  // the same because they bubble. Bound to the input it was one blur away from
  // being gone: clicking the palette's own padding moves focus off the input,
  // and Escape then closed nothing at all. The panel is `tabIndex={-1}` so it
  // can hold that focus rather than dropping it to <body>.
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
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
      // The dialog's only tabbable element is the query input — hold focus
      // here so Tab can't reach controls hidden behind the overlay
      // (aria-modal). Refocus it too: if the click that blurred it landed on
      // the panel, Tab should put the caret back rather than go nowhere.
      event.preventDefault();
      inputRef.current?.focus();
    }
  };

  return (
    <Overlay
      onDismiss={onClose}
      label="Command palette"
      className="overflow-hidden"
      onKeyDown={onKeyDown}
    >
      {/* Magnifier first, then the query. The same glyph at the same size as
          the Search view's field, so a place you type a query is recognisable
          wherever it turns up. */}
      <div className="command-palette__search">
        <svg
          width="17"
          height="17"
          viewBox="0 0 18 18"
          fill="none"
          aria-hidden="true"
          className="block flex-none text-text-faint"
        >
          <circle cx="8" cy="8" r="6.2" stroke="currentColor" strokeWidth="1.4" />
          <path
            d="M12.4 12.4L16 16"
            stroke="currentColor"
            strokeWidth="1.4"
            strokeLinecap="round"
          />
        </svg>
        <input
          ref={inputRef}
          role="combobox"
          // Reflects the list, rather than being pinned open. It was hardcoded
          // `"true"`, which told a screen reader there were options to walk
          // even on the branch that renders none.
          aria-expanded={rows.length > 0}
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
          className="command-palette__input text-input text-text placeholder:text-text-faint"
        />
      </div>
      <hr className="command-palette__rule" />
      <ul
        id={LISTBOX_ID}
        role="listbox"
        aria-label="Commands"
        className="command-palette__list"
      >
        {/* There is deliberately no empty row here. One used to sit at this
            spot saying "No commands available yet.", justified as the branch
            taken "when the project listing failed" — but `rows` cannot reach
            zero: useCommands unconditionally pushes four actions, and a query
            that matches nothing is answered by the synthetic search row. It
            was a state vocabulary for a state that does not exist, and it was
            a bare <li> with no role sitting directly inside role="listbox",
            which is not a valid owned element either. */}
        {sections.map((section, sectionIndex) => (
          // role="presentation" on the wrapper so the ul/li scaffolding does
          // not put listitem semantics between the listbox and its options;
          // the inner role="group" is what actually carries the section name.
          <li key={section.label ?? "matches"} role="presentation">
            {section.label && (
              <p
                className={`command-palette__section${
                  sectionIndex > 0 ? " command-palette__section--later" : ""
                } font-mono text-eyebrow uppercase tracking-eyebrow-menu text-text-faint`}
              >
                {section.label}
              </p>
            )}
            <ul role="group" aria-label={section.label ?? undefined}>
              {section.rows.map(({ command, index }) => (
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
                  className={`command-palette__row text-body ${
                    index === active ? "ui-wash" : "text-text"
                  }`}
                >
                  <span>{command.title}</span>
                  {/* The focused row says how to run it; any row with a real
                      global binding says what that is. Mono, because both are
                      keys. A row with neither shows nothing — the palette
                      teaches shortcuts, so filling this with a restatement of
                      the section heading taught nothing. */}
                  {(index === active || command.hint) && (
                    // --text-soft on the active row, --text-faint on the rest.
                    // The active row's ground is --menu-hover, not the overlay
                    // plane, and faint ink on that fill measures 3.07:1 in the
                    // day theme and 2.83:1 at night — under even the 3:1 floor
                    // a graphic gets, for the one row the user is looking at
                    // (docs/DESIGN_SYSTEM.md §6).
                    <span
                      className={`flex-none font-mono text-eyebrow ${
                        index === active ? "text-text-soft" : "text-text-faint"
                      }`}
                    >
                      {index === active ? "↵" : command.hint}
                    </span>
                  )}
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </Overlay>
  );
}
