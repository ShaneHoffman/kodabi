import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { NoteType } from "./useNotes";
import { useDebouncedValue } from "./useDebouncedValue";
import { useVaultQuery } from "./useVaultQuery";

/*
 * Search, over the real note index.
 *
 * The backend does the matching: `search_notes` (src-tauri/src/index_cmds.rs)
 * runs kodabi-core's hybrid FTS5 + vector search — the same one the MCP tool
 * serves — and hands back ranked hits with the matched terms already marked.
 * The frontend's job is to debounce the typing and render what comes back.
 */

/** How long the field must hold still before a query goes to the backend. Long
 * enough that a typed word is one search rather than five, short enough that
 * the results feel attached to the keyboard. */
export const SEARCH_DEBOUNCE_MS = 250;

/** Below this, the field shows its hint instead of searching. A single letter
 * matches most of the vault, which is a slow way to say nothing. */
export const MIN_QUERY_LENGTH = 2;

/** One page, at the contract's ceiling (`SearchParams.limit` clamps to 50).
 * There is no pagination UI: past fifty hits the answer is a better query. */
const SEARCH_LIMIT = 50;

/** The delimiters `search_notes` wraps matched terms in — `SNIPPET_MARK_OPEN`/
 * `SNIPPET_MARK_CLOSE` in src-tauri/src/index_cmds.rs. Private-use codepoints,
 * written as escapes rather than pasted, so they survive an editor and stay
 * visible in a diff. */
const MARK_OPEN = "\uE000";
const MARK_CLOSE = "\uE001";

/**
 * A ranked hit — the Rust `SearchHit`
 * (`crates/kodabi-core/src/index/search.rs`), which is a `NoteSummary` plus
 * `score`, `rank` and `snippet`. `project: null` means unfiled (Inbox).
 */
export type SearchHit = {
  id: string;
  path: string;
  title: string;
  type: NoteType;
  project: string | null;
  date: string;
  tags: string[];
  source: string;
  confidence: number | null;
  score: number;
  rank: number;
  /** The matching passage, with matched terms wrapped in the mark sentinels. */
  snippet: string;
};

/** The Rust `PageInfo`. `total_estimate` is `null` once the candidate pool came
 * back full, which is the index saying it no longer knows the exact count. */
export type SearchPage = {
  has_more: boolean;
  next_cursor: string | null;
  total_estimate: number | null;
};

/** The Rust `SearchResults` — one page of ranked hits. */
export type SearchResults = {
  hits: SearchHit[];
  page: SearchPage;
};

/** A run of snippet or title text, and whether it is part of a match. */
export type TextSegment = {
  text: string;
  marked: boolean;
};

/**
 * Splits a hit's snippet on the mark sentinels.
 *
 * A vector-only hit has no marks at all (its snippet is its nearest chunk's own
 * words), and comes back as one unmarked segment — which is honest: that row
 * matched by meaning, not by a word you can point at.
 */
export function parseSnippet(snippet: string): TextSegment[] {
  const segments: TextSegment[] = [];
  let rest = snippet;

  while (rest) {
    const open = rest.indexOf(MARK_OPEN);
    if (open < 0) break;
    const close = rest.indexOf(MARK_CLOSE, open);
    // An unpaired opener means a truncated snippet; treat the remainder as
    // plain text rather than dropping it.
    if (close < 0) break;

    if (open > 0) segments.push({ text: rest.slice(0, open), marked: false });
    const match = rest.slice(open + MARK_OPEN.length, close);
    if (match) segments.push({ text: match, marked: true });
    rest = rest.slice(close + MARK_CLOSE.length);
  }

  if (rest) segments.push({ text: rest, marked: false });
  return segments;
}

/**
 * Marks `query`'s terms in text the backend did not mark for us.
 *
 * FTS5's `snippet()` marks one column — the best-matching one — so a hit found
 * by its title arrives with the title bare. This puts the same green on it, by
 * the same rule the backend used: match each term where it starts, and treat
 * the last one as a prefix, since that is what the user is still typing.
 */
export function highlightTerms(text: string, query: string): TextSegment[] {
  const terms = query
    .split(/\s+/)
    .filter((term) => term.length > 0)
    .map((term) => term.toLowerCase());
  if (terms.length === 0) return [{ text, marked: false }];

  const haystack = text.toLowerCase();
  // One pass over the text, taking the earliest term that starts here. Ranges
  // can't overlap because each match advances past itself.
  const segments: TextSegment[] = [];
  let plainFrom = 0;
  let index = 0;

  while (index < text.length) {
    const term = terms.find((candidate) => haystack.startsWith(candidate, index));
    if (!term) {
      index += 1;
      continue;
    }
    if (index > plainFrom) {
      segments.push({ text: text.slice(plainFrom, index), marked: false });
    }
    segments.push({ text: text.slice(index, index + term.length), marked: true });
    index += term.length;
    plainFrom = index;
  }

  if (plainFrom < text.length) {
    segments.push({ text: text.slice(plainFrom), marked: false });
  }
  return segments;
}

export type NoteSearch = {
  /** The page the backend returned, or `null` while idle or in flight. */
  results: SearchResults | null;
  /** The query `results` answers — empty while idle. Every piece of copy about
   * the outcome ("Nothing matches …") reads from this rather than the field, so
   * it can never quote a query that was not searched. */
  searchedQuery: string;
  loading: boolean;
  error: string | null;
};

/**
 * Searches the note index for `query`, debounced.
 *
 * Two blessed bridge hooks compose into this and no effect is written here:
 * `useDebouncedValue` waits for the typing to settle, and `useVaultQuery` runs
 * the call, sequences overlapping responses, and refetches on `vault:changed`
 * — so a note saved in another window updates the results underneath you.
 */
export function useNoteSearch(query: string): NoteSearch {
  const trimmed = query.trim();
  const settled = useDebouncedValue(trimmed, SEARCH_DEBOUNCE_MS);
  // Below the minimum there is nothing to ask, and the empty key is what parks
  // the query in its idle state rather than firing a fetch that returns nothing.
  const active = settled.length >= MIN_QUERY_LENGTH ? settled : "";

  const { data, loading, error } = useVaultQuery<SearchResults | null>(
    useCallback(async () => {
      if (!active) return null;
      return invoke<SearchResults>("search_notes", {
        params: { query: active, limit: SEARCH_LIMIT },
      });
    }, [active]),
  );

  return {
    results: active ? data : null,
    searchedQuery: active,
    // `useVaultQuery` opens every query in its loading state, including the
    // idle no-op; the field must not report work it isn't doing.
    loading: loading && !!active,
    error: active ? error : null,
  };
}
