import { useId, useState } from "react";
import { clsx } from "clsx";
import { noteKind } from "../../noteMeta";
import { useNavigation } from "../../useNavigation";
import { INBOX_PROJECT } from "../../useNotes";
import { folderHue, type FolderHue } from "../../useProjects";
import { useScrollIntoView } from "../../useScrollIntoView";
import {
  highlightTerms,
  parseSnippet,
  useNoteSearch,
  type SearchHit,
  type TextSegment,
} from "../../useSearch";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";

type Props = {
  query: string;
};

/** Written out rather than interpolated, so Tailwind's scanner sees each class
 * as a literal and emits it (the palette's `HUE_DOT` precedent). */
const HUE_TEXT: Record<FolderHue, string> = {
  coral: "text-coral",
  cobalt: "text-cobalt",
  teal: "text-teal",
  plum: "text-plum",
};

/** The idle hint and the no-hits line are the same voice at the same weight:
 * one quiet line where the rows would be, saying what this field does or what
 * it just did. */
const QUIET_LINE = "px-2.5 py-3.5 font-data text-[10.5px] text-ink-faint";

/** One shared empty list, so "no results yet" is one identity rather than a
 * fresh array on every render. */
const NO_HITS: SearchHit[] = [];

/**
 * Search over every note.
 *
 * The index does the matching (`useNoteSearch` → the `search_notes` command),
 * so a hit can come from deep in a body this view never listed, and the marks
 * in a snippet are the backend's own — the green says "the system found this",
 * which is one of the sanctioned uses of green (docs/DESIGN_SYSTEM.md §2).
 */
export function SearchView({ query }: Props) {
  const { navigate } = useNavigation();
  const [draft, setDraft] = useState(query);
  const { results, searchedQuery, loading, error } = useNoteSearch(draft);

  const baseId = useId();
  const listboxId = `${baseId}-results`;
  const hitId = (index: number) => `${baseId}-hit-${index}`;

  const hits = results?.hits ?? NO_HITS;

  // The walker resets whenever the result set changes underneath it: an index
  // into a list that just became a different list points at nothing the user
  // chose. Adjust-state-during-render rather than an effect, per
  // .claude/rules/no-use-effect.md (the NeedsAttentionView precedent).
  //
  // Keyed on the results OBJECT, which `useVaultQuery` holds in state and only
  // replaces when a response lands. Keying on `hits` instead compares an array
  // this component just derived, which is a new identity every render — the
  // reset then fires forever.
  const [activeIndex, setActiveIndex] = useState(-1);
  const [walkedResults, setWalkedResults] = useState(results);
  if (walkedResults !== results) {
    setWalkedResults(results);
    setActiveIndex(-1);
  }

  const active = activeIndex < hits.length ? activeIndex : -1;
  useScrollIntoView(active >= 0 ? hitId(active) : null);

  const openHit = (hit: SearchHit) =>
    navigate({
      kind: "noteEditor",
      noteId: hit.id,
      project: hit.project ?? INBOX_PROJECT,
    });

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    // Keys pressed mid-IME-composition belong to the composition (Select.tsx).
    if (event.nativeEvent.isComposing || hits.length === 0) return;

    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex(Math.min(active + 1, hits.length - 1));
    } else if (event.key === "ArrowUp") {
      // Clamped at -1, which is "back in the field with no row lit" — the state
      // the walk started from, so ArrowUp always has somewhere to go back to.
      event.preventDefault();
      setActiveIndex(Math.max(active - 1, -1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      // Enter with nothing walked opens the top hit: it is what the ranking
      // says you meant, and pressing Enter on a search field means "go".
      openHit(hits[Math.max(active, 0)]);
    }
  };

  return (
    <ViewFrame variant="search" title="Search">
      {/* The Grove glass field (Field.tsx's recipe at the search radius). No
          focus ring: the border step and the kodama caret ARE the focus
          signal, and a ring around a box that already changed says it twice. */}
      <div
        className={clsx(
          "mt-5 flex items-center gap-2.5 rounded-[12px] border border-edge bg-wash px-4 py-3",
          "shadow-[inset_0_1px_0_var(--color-edge-lit)]",
          "transition-[border-color] duration-140 ease-out-strong",
          "focus-within:border-ink-faint",
        )}
      >
        {/* The palette's glyph, at the palette's size — one magnifier in the
            app, so a query field is recognisable wherever it turns up. */}
        <span aria-hidden="true" className="flex-none text-[13px] text-ink-faint">
          ⌕
        </span>
        <input
          type="text"
          role="combobox"
          aria-label="Search notes"
          aria-expanded={hits.length > 0}
          aria-controls={listboxId}
          aria-activedescendant={active >= 0 ? hitId(active) : undefined}
          aria-autocomplete="list"
          autoComplete="off"
          autoFocus
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Search every note…"
          className={clsx(
            "w-full min-w-0 bg-transparent",
            "font-ui text-[13.5px] text-ink caret-kodama outline-hidden",
            "placeholder:text-ink-faint",
          )}
        />
        {/* Gated on `loading` as well as on there being a query: an ungated
            count reads "0 hits" for the length of every round trip. */}
        <span className="flex-none font-data text-[10.5px] text-ink-faint tabular-nums">
          {results && !loading ? countLabel(results.page.total_estimate, hits.length) : ""}
        </span>
      </div>

      {error ? (
        <StatusMessage variant="error">Couldn&apos;t search your notes: {error}</StatusMessage>
      ) : !searchedQuery ? (
        <p className={QUIET_LINE}>Searches titles and snippets across every note.</p>
      ) : hits.length === 0 ? (
        !loading &&
        results && <p className={QUIET_LINE}>Nothing matches &ldquo;{searchedQuery}&rdquo;.</p>
      ) : (
        <ul id={listboxId} role="listbox" aria-label="Search results" className="mt-3.5">
          {hits.map((hit, index) => (
            <li
              key={hit.path}
              id={hitId(index)}
              role="option"
              aria-selected={index === active}
              data-active={index === active}
              // Pointer and keyboard drive the same one attribute, so two rows
              // can never be lit at once (docs/UI_CONVENTIONS.md §4) — which is
              // also why there is no `hover:` here.
              onMouseMove={() => index !== active && setActiveIndex(index)}
              onClick={() => openHit(hit)}
              className={clsx(
                "block w-full cursor-default rounded-[10px] px-2.5 py-3 text-left",
                "transition-[background-color] duration-140 ease-out-strong",
                "data-[active=true]:bg-wash",
              )}
            >
              <span className="block truncate text-[14.5px] font-semibold text-ink">
                <Marked segments={highlightTerms(hit.title, searchedQuery)} />
              </span>
              {/* The folder leads, in the folder's own hue: what tells this
                  list apart from a project's is that it spans them. */}
              <span className="mt-1 block truncate font-data text-[10.5px] text-ink-faint">
                <span className={hit.project ? HUE_TEXT[folderHue(hit.project)] : undefined}>
                  {hit.project ?? INBOX_PROJECT.toLowerCase()}
                </span>
                {metaTail(hit)}
              </span>
              <span className="mt-1 block truncate text-[12.5px] text-ink-dim">
                <Marked segments={parseSnippet(hit.snippet)} />
              </span>
            </li>
          ))}
        </ul>
      )}
    </ViewFrame>
  );
}

/** Text with the matched runs in green. `mark`'s own default background is a
 * yellow that belongs to no theme here, so the element carries the tokens. */
function Marked({ segments }: { segments: TextSegment[] }) {
  return (
    <>
      {segments.map((segment, index) =>
        segment.marked ? (
          <mark key={index} className="rounded-[3px] bg-kodama/16 px-0.5 text-kodama-ink">
            {segment.text}
          </mark>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </>
  );
}

/** What follows the folder on a hit's meta line: the kind when it says
 * something, then the note's day. */
function metaTail(hit: SearchHit): string {
  const kind = noteKind(hit.type);
  return [kind, hit.date.slice(0, 10)].filter(Boolean).map((part) => ` · ${part}`).join("");
}

/**
 * The hit count, mono and right-aligned in the field.
 *
 * `total_estimate` is the exact fused total, and `null` once the index's
 * candidate pool came back full — past that it does not know, so the count says
 * "at least this many" rather than quoting the page size as if it were the
 * total.
 */
function countLabel(totalEstimate: number | null, shown: number): string {
  if (totalEstimate === null) return `${shown}+ hits`;
  return `${totalEstimate} ${totalEstimate === 1 ? "hit" : "hits"}`;
}
