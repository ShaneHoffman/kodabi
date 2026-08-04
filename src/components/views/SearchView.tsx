import { useMemo, useState, type ReactNode } from "react";
import { useNavigation } from "../../useNavigation";
import { INBOX_PROJECT, type NoteSummary } from "../../useNotes";
import { useSearchCorpus } from "../../useSearch";
import { StatusMessage } from "../ui/StatusMessage";
import { ViewFrame } from "../ui/ViewFrame";
// eslint-disable-next-line no-restricted-syntax -- pre-Grove; this view's Grove ticket deletes it
import "./SearchView.css";

type Props = {
  query: string;
};

/** A note plus where the query matched it. */
type Hit = {
  note: NoteSummary;
  /** The snippet split around the first match, so the term can be marked
   * without a regex running inside JSX. */
  before: string;
  match: string;
  after: string;
};

/** The project a note reads as, in the mono path vocabulary the filing menu
 * and the project view both use. An unfiled note is `inbox`. */
function crumb(note: NoteSummary): string {
  return `${note.project ?? INBOX_PROJECT.toLowerCase()} · ${note.date}`;
}

/**
 * Where the query lands in a note, if it lands at all.
 *
 * Matching is over the title, the snippet and the tags, but the EXCERPT is
 * always taken from the snippet: a result whose title matched still needs to
 * show you body text, or every row would just repeat its own heading back.
 * When the match is not in the snippet the snippet is shown unmarked, which is
 * honest — the row is telling you it matched elsewhere.
 */
function findHit(note: NoteSummary, needle: string): Hit | null {
  const haystacks = [note.title, note.snippet, note.tags.join(" ")];
  if (!haystacks.some((text) => text.toLowerCase().includes(needle))) return null;

  const index = note.snippet.toLowerCase().indexOf(needle);
  if (index < 0) return { note, before: note.snippet, match: "", after: "" };
  return {
    note,
    before: note.snippet.slice(0, index),
    match: note.snippet.slice(index, index + needle.length),
    after: note.snippet.slice(index + needle.length),
  };
}

/**
 * Search across every project.
 *
 * The matching runs in the frontend over the notes already listed from disk.
 * That is deliberate for now: the FTS5 index exists but has no query command
 * yet, and a real-feeling search over real notes is worth more than a stub
 * that says "arrives later". `useSearchCorpus` is the seam — when the backend
 * command lands, it changes and nothing here does.
 */
export function SearchView({ query }: Props) {
  const { navigate } = useNavigation();
  const [draft, setDraft] = useState(query);
  const { notes, loading, error } = useSearchCorpus();

  const needle = draft.trim().toLowerCase();
  const hits = useMemo(
    () => (needle ? notes.map((note) => findHit(note, needle)).filter(Boolean) : []),
    [notes, needle],
  ) as Hit[];

  // The scope line counts where the matches LANDED, so it renders only on the
  // branch that has hits. (It once claimed to count what was searched rather
  // than what matched, which the code never did — and a count of the corpus
  // would have to come from `useSearchCorpus`, not from `hits`.)
  const projectsHit = new Set(
    hits.map((hit) => hit.note.project).filter((project): project is string => !!project),
  ).size;
  const unfiledHit = hits.filter((hit) => hit.note.project === null).length;

  return (
    <ViewFrame variant="search">
      <div className="search__field ui-focus-ring-within">
        <MagnifierGlyph />
        <input
          value={draft}
          onChange={(event) => setDraft(event.target.value)}
          placeholder="Search notes…"
          aria-label="Search notes"
          autoFocus
          className="search__input ui-writing text-input text-text placeholder:text-text-faint"
        />
        {/* Gated on `loading` for the same reason the empty state below is:
            the corpus is empty until the listing lands, so an ungated count
            flashed "0 results" on every cold start while the branch beneath
            it correctly rendered nothing (docs/DESIGN_SYSTEM.md §3). */}
        <span className="ui-tnum flex-none font-mono text-cap text-text-faint">
          {needle && !loading
            ? `${hits.length} ${hits.length === 1 ? "result" : "results"}`
            : ""}
        </span>
      </div>

      {needle && hits.length > 0 && (
        <p className="mt-sm font-mono text-micro uppercase text-text-faint">
          Across {projectsHit} {projectsHit === 1 ? "project" : "projects"}
          {unfiledHit > 0 && ` · ${unfiledHit} unfiled`}
        </p>
      )}

      {error ? (
        <StatusMessage variant="error">Couldn&apos;t search your notes: {error}</StatusMessage>
      ) : !needle ? (
        <StatusMessage variant="empty">
          Type to search every note in every project, plus the unfiled ones.
        </StatusMessage>
      ) : hits.length === 0 ? (
        !loading && (
          <StatusMessage variant="empty">
            Nothing matches &ldquo;{draft.trim()}&rdquo;. Search reads note titles,
            snippets and tags, so a word from deeper in the body will not match yet.
          </StatusMessage>
        )
      ) : (
        <ul className="search__results">
          {hits.map((hit) => (
            <li key={hit.note.path}>
              <button
                type="button"
                className="search__row ui-focus-ring"
                onClick={() =>
                  navigate({
                    kind: "noteEditor",
                    noteId: hit.note.id,
                    project: hit.note.project ?? INBOX_PROJECT,
                    // The live draft, not the `query` prop: the prop is what
                    // this view was opened with, and back has to return to the
                    // results the reader was actually looking at.
                    origin: { kind: "search", query: draft.trim() },
                  })
                }
              >
                {/* Breadcrumb first — the signal that this list spans
                    projects, and the thing that tells it apart from the
                    Inbox at a glance. */}
                <span className="block font-mono text-micro text-text-faint">
                  {crumb(hit.note)}
                </span>
                <span className="mt-3xs block truncate text-lead font-semibold text-text">
                  {hit.note.title}
                </span>
                <span className="mt-3xs block font-serif text-snippet-sm leading-snippet text-text-soft">
                  {hit.before}
                  {hit.match && <mark className="search__mark">{hit.match}</mark>}
                  {hit.after}
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </ViewFrame>
  );
}

/** The same glyph the command palette uses, at the same size — one magnifier
 * in the app, so a query field is recognisable wherever it turns up. */
function MagnifierGlyph(): ReactNode {
  return (
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
  );
}
