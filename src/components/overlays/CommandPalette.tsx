import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { clsx } from "clsx";
import { Command, useCommandState } from "cmdk";
import { useState } from "react";
import { useCommands, type Command as PaletteCommand, type CommandKind } from "../../useCommands";
import { useNavigation } from "../../useNavigation";
import type { FolderHue } from "../../useProjects";

type Props = {
  /** Whether the palette is on screen. `useCommandPalette` owns this. */
  open: boolean;
  onClose: () => void;
};

/*
 * The primary navigation surface (FOUNDING_DOC §4), on cmdk inside base-ui's
 * dialog parts. Two libraries because they answer two different questions and
 * neither answers both:
 *
 *   cmdk owns the list — fuzzy scoring, the arrow keys and Home/End, the
 *   listbox/option ARIA, and keeping one row highlighted whether the pointer or
 *   the keyboard put it there. The hand-rolled version of that was 255 lines
 *   and still filtered by substring only.
 *
 *   base-ui owns the modal — the focus trap, focus returning to whatever opened
 *   the palette, Escape, the outside press, and the inert page behind. This is
 *   the `Menu` precedent (docs/UI_CONVENTIONS.md §4), and it is why cmdk's own
 *   `Command.Dialog` is NOT used: that one wraps @radix-ui/react-dialog, which
 *   would put a second dialog implementation in the app beside base-ui's.
 *
 * What stays ours is the whole appearance: the glass, the rows, the motion.
 */

/** The panel. Placement is `inset-x-0 … mx-auto`, NOT a translate: the
 * `materialize` keyframe animates `transform`, so a translate-centred panel is
 * overwritten by the animation's `scale()` and opens in the corner. The same
 * trap is documented on `Dialog`, which centres by margin for this reason.
 *
 * Durations are spelled here rather than taken from `--animate-materialize`
 * because this one is faster than the shared 220ms: Ctrl K is a hundred-times-
 * a-day action, and theatrics on the way in are what make a tool feel slow.
 * Which is what DESIGN_SYSTEM §4 means by durations being call-site utilities.
 * The keyframes are still the shared ones, named rather than redefined —
 * Tailwind emits every `@keyframes` declared inside the `@theme` block whether
 * or not its `animate-*` utility is used, so an arbitrary value may reference
 * one by name without the surface that owns it. */
const PALETTE_SURFACE = clsx(
  "glass-palette fixed inset-x-0 top-[12%] mx-auto h-fit w-[min(480px,calc(100vw-2rem))]",
  "origin-top p-2 font-ui outline-hidden",
  "animate-[materialize_200ms_var(--ease-out-strong)] motion-reduce:animate-[fade-in_200ms_ease_both]",
  "data-ending-style:animate-[dissolve_110ms_ease_forwards] motion-reduce:data-ending-style:animate-[fade-out_110ms_ease_forwards]",
);

/** A row. `data-[selected=true]` and not `data-selected`, because cmdk writes
 * the attribute on every row and sets it to `"false"` on the unselected ones —
 * the bare variant would light the whole list.
 *
 * There is deliberately no `hover:` here. cmdk moves the selection on pointer
 * move, so the pointer and the keyboard drive the same one attribute, which is
 * the never-two-rows-lit-at-once invariant of docs/UI_CONVENTIONS.md §4. */
const PALETTE_ROW = clsx(
  "group flex w-full cursor-default select-none items-center gap-2.5 rounded-[9px] px-2.5 py-2.5",
  "text-[13px] font-medium leading-none text-ink-dim outline-hidden",
  "transition-[background-color,color] duration-140 ease-out-strong",
  "data-[selected=true]:bg-wash data-[selected=true]:text-ink",
);

/** The mono slot on the right. Faint ink on the wash of a selected row measures
 * under the 3:1 floor a graphic gets, so it steps up for the one row the user
 * is looking at (docs/DESIGN_SYSTEM.md §6).
 *
 * `uppercase` belongs to the kind tag alone, and is added by the call site: a
 * kind is an eyebrow, but a shortcut is a literal, and "CTRL+ALT+SPACE" is not
 * how Windows writes the binding the user has to recognise. */
const PALETTE_TAG = clsx(
  "ml-auto flex-none font-data text-[10px] tracking-[0.14em] text-ink-faint",
  "group-data-[selected=true]:text-ink-dim",
);

/** Written out rather than interpolated, so Tailwind's scanner sees each class
 * as a literal and emits it. */
const HUE_DOT: Record<FolderHue, string> = {
  coral: "bg-coral",
  cobalt: "bg-cobalt",
  teal: "bg-teal",
  plum: "bg-plum",
};

export function CommandPalette({ open, onClose }: Props) {
  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next) => {
        // One direction only: Ctrl K and the sidebar's Commands row own opening.
        if (!next) onClose();
      }}
    >
      <BaseDialog.Portal>
        {/* A veil, not a blackout. The palette's glass can only read as glass
            if the app stays visible enough beneath it to blur — and the scrim
            only fades, so it needs no reduced-motion partner (that swap turns
            everything else INTO a fade). Same recipe and same testid as
            `Dialog`: it is the one part of a modal with no role and no
            accessible name to find it by. */}
        <BaseDialog.Backdrop
          data-testid="dialog-scrim"
          className="glass-scrim fixed inset-0 animate-fade-in data-ending-style:animate-fade-out"
        />
        <BaseDialog.Popup aria-label="Command palette" className={PALETTE_SURFACE}>
          {/* Inside the popup, so it unmounts with it and every open starts on
              a blank query rather than resuming the last one. */}
          <PaletteContent onClose={onClose} />
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}

function PaletteContent({ onClose }: { onClose: () => void }) {
  const commands = useCommands();
  const [query, setQuery] = useState("");

  const runCommand = (command: PaletteCommand) => {
    command.run();
    onClose();
  };

  // Partitioned, never re-sorted: useCommands already emits Inbox first and the
  // folders in backend slug order.
  const rowsOfKind = (kind: CommandKind) => commands.filter((command) => command.kind === kind);
  const navigateRows = rowsOfKind("navigate");
  const folderRows = rowsOfKind("folder");
  const actionRows = rowsOfKind("action");

  return (
    // `vimBindings` off: cmdk binds Ctrl+K to "move the selection up", and the
    // app binds it to "toggle the palette" (useCommandPalette). Left on, one
    // press would do both.
    <Command label="Command palette" vimBindings={false}>
      <div className="flex items-center gap-2.5 border-b border-edge px-2.5 pt-2 pb-2.5">
        <span aria-hidden="true" className="flex-none text-[13px] text-ink-faint">
          ⌕
        </span>
        {/* The one place in Grove a focus ring is dropped on purpose: the input
            is the dialog's only tab stop, it takes focus the moment the palette
            opens, and it already shows focus the way a text field does — with a
            caret (docs/DESIGN_SYSTEM.md §2). The caret is kodama green, which
            is green's own job: the system is listening for your words. */}
        <Command.Input
          value={query}
          onValueChange={setQuery}
          placeholder="Type a command or a place…"
          className="w-full bg-transparent text-[14px] text-ink caret-kodama outline-hidden placeholder:text-ink-faint"
        />
      </div>
      {/* No `Command.Empty`. The list cannot come back empty: a query that
          matches nothing is answered by the fallback row below, so an empty
          state here would be vocabulary for a state that does not exist.

          60vh is what is left under a panel that starts at 12%, and it is sized
          so an ordinary vault does not scroll at all: the action rows are last,
          they are always there, and a max-height that hides them behind a
          scroll on a five-project vault would hide the two commands that never
          depend on what is in the vault. A large vault still scrolls, which is
          what the filter is for. */}
      {/* `label` and not `aria-label`: cmdk spreads the caller's props before
          its own `aria-label`, so a hand-passed one is overwritten by the
          library's default — and that default is the word "Suggestions", which
          is not what this list is. */}
      <Command.List label="Commands" className="max-h-[60vh] overflow-y-auto pt-1.5">
        {/* Headings are screen-reader only. Sighted users get the hairlines,
            which say the same thing in less space, but a group of options with
            no name is a group a screen reader cannot describe. cmdk hides both
            heading and separator while filtering, which is the old behaviour
            exactly: a filtered list is a set of matches, not a table of
            contents. */}
        <Command.Group heading={<span className="sr-only">Jump to</span>}>
          {navigateRows.map((command) => (
            <PaletteRow key={command.id} command={command} onRun={runCommand} />
          ))}
        </Command.Group>
        {folderRows.length > 0 && (
          <>
            <Command.Separator className="my-1.5 h-px bg-edge" />
            <Command.Group heading={<span className="sr-only">Folders</span>}>
              {folderRows.map((command) => (
                <PaletteRow key={command.id} command={command} onRun={runCommand} />
              ))}
            </Command.Group>
          </>
        )}
        <Command.Separator className="my-1.5 h-px bg-edge" />
        <Command.Group heading={<span className="sr-only">Actions</span>}>
          {actionRows.map((command) => (
            <PaletteRow key={command.id} command={command} onRun={runCommand} />
          ))}
        </Command.Group>
        {query.trim() !== "" && <SearchFallbackRow query={query.trim()} onClose={onClose} />}
      </Command.List>
    </Command>
  );
}

function PaletteRow({
  command,
  onRun,
}: {
  command: PaletteCommand;
  onRun: (command: PaletteCommand) => void;
}) {
  return (
    // Scored on the title, not the id: `jump:` and the slashes in a nested slug
    // are filing, not something anybody types. `keywords` keeps the hint
    // searchable, which is how "ctrl alt space" still finds Quick capture.
    <Command.Item
      value={command.title}
      keywords={command.hint ? [command.hint] : undefined}
      onSelect={() => onRun(command)}
      className={PALETTE_ROW}
    >
      {command.hue && (
        <span
          aria-hidden="true"
          className={clsx("size-2 flex-none rounded-[2px]", HUE_DOT[command.hue])}
        />
      )}
      <span className="truncate">{command.title}</span>
      {/* A row with a real global binding says what it is; every other row says
          what kind of thing it is. The shortcut wins the slot because the
          palette is where people learn the bindings, and a row that teaches one
          is worth more than a row that repeats its own group's name. */}
      {command.hint ? (
        <span className={PALETTE_TAG}>{command.hint}</span>
      ) : (
        <span className={clsx(PALETTE_TAG, "uppercase")}>{command.kind}</span>
      )}
    </Command.Item>
  );
}

/**
 * When nothing matches, the query itself becomes the command, so typed text
 * always leads somewhere.
 *
 * `forceMount` is what makes this work: a force-mounted item is not registered
 * with cmdk's filter store, so it can never inflate the very count it is
 * reading, but it is still a real `[cmdk-item]` in the DOM and so still
 * reachable by the arrow keys and still eligible to be selected first.
 *
 * It lives in its own component because `useCommandState` has to be called
 * inside the `Command` context.
 */
function SearchFallbackRow({ query, onClose }: { query: string; onClose: () => void }) {
  const { navigate } = useNavigation();
  const matchCount = useCommandState((state) => state.filtered.count);

  if (matchCount > 0) return null;

  return (
    <Command.Item
      forceMount
      // Distinct from any command title, so the selection cannot land on a real
      // row's value by accident.
      value={`search-fallback ${query}`}
      onSelect={() => {
        navigate({ kind: "search", query });
        onClose();
      }}
      className={PALETTE_ROW}
    >
      <span className="truncate">Search for “{query}”</span>
    </Command.Item>
  );
}
