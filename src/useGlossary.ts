import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useVaultQuery } from "./useVaultQuery";

/**
 * One glossary entry. Mirrors `GlossaryTermDto` in
 * src-tauri/src/glossary_cmds.rs (itself `kodabi_core::glossary::GlossaryTerm`).
 */
export type GlossaryTerm = {
  term: string;
  definition: string;
  aliases: string[];
};

/**
 * A glossary's full contents. Mirrors `GlossaryListingDto` in
 * src-tauri/src/glossary_cmds.rs.
 *
 * `project: null` is the VAULT-WIDE glossary at the knowledge-base root — the
 * one the transcription pipeline loads to bias every capture, since a session
 * is transcribed before routing has picked a project. A slug is that project's
 * own glossary, which feeds routing signals and project context instead.
 */
export type GlossaryListing = {
  project: string | null;
  terms: GlossaryTerm[];
};

/**
 * The editable fields of a term, as the dialog collects them. Deliberately not
 * a wire type: the scope is the caller's argument, not something the form
 * holds, so the two commands below splice it in.
 */
export type GlossaryTermFields = {
  term: string;
  definition: string;
  aliases: string[];
};

/** Mirrors `GlossaryTermInput` in src-tauri/src/glossary_cmds.rs. */
type GlossaryTermInput = GlossaryTermFields & {
  project: string | null;
};

/**
 * Mirrors `UpdateGlossaryTermInput` in src-tauri/src/glossary_cmds.rs.
 *
 * `original_term` is snake_case because it is a field inside a struct
 * argument, which Tauri hands to serde verbatim — only bare command arguments
 * get camelCased. Spelled as a type rather than built inline so `tsc` fails
 * when the Rust field is renamed, instead of the value quietly never arriving.
 */
type UpdateGlossaryTermInput = GlossaryTermInput & {
  original_term: string;
};

/**
 * The empty listing, as one shared value rather than a fresh `[]` per render.
 * Consumers compare `terms` by identity to prune per-row state when the list
 * changes (the adjust-state-during-render pattern), and a new array every
 * render makes that comparison always true — which is an infinite render loop,
 * not a slow one.
 */
const NO_TERMS: GlossaryTerm[] = [];

/**
 * Aliases as the dialog stores them: one comma-separated line in, a clean list
 * out. Blank entries are dropped rather than persisted as empty strings (the
 * backend trims too, but the count shown beside the field should already be
 * honest).
 */
export function parseAliases(raw: string): string[] {
  return raw
    .split(",")
    .map((alias) => alias.trim())
    .filter((alias) => alias.length > 0);
}

/** The inverse, for prefilling the field when editing an existing term. */
export function formatAliases(aliases: string[]): string {
  return aliases.join(", ");
}

/**
 * Adds a term to a glossary. The backend uses `OnConflict::Error`, so adding a
 * term that already exists rejects rather than silently overwriting the
 * definition the user did not open. Refreshes ride the backend's
 * `vault:changed` broadcast.
 */
export function addGlossaryTerm(
  project: string | null,
  fields: GlossaryTermFields,
): Promise<GlossaryTerm> {
  const input: GlossaryTermInput = { project, ...fields };
  return invoke<GlossaryTerm>("add_glossary_term", { input });
}

/**
 * Replaces the entry named by `originalTerm`, preserving its position in the
 * file. An `input.term` that differs from `originalTerm` is a rename, which
 * rejects if it would land on another entry's term.
 */
export function updateGlossaryTerm(
  project: string | null,
  originalTerm: string,
  fields: GlossaryTermFields,
): Promise<GlossaryTerm> {
  const input: UpdateGlossaryTermInput = {
    project,
    original_term: originalTerm,
    ...fields,
  };
  return invoke<GlossaryTerm>("update_glossary_term", { input });
}

/** Removes a term, echoing the entry that was removed. */
export function deleteGlossaryTerm(
  project: string | null,
  term: string,
): Promise<GlossaryTerm> {
  return invoke<GlossaryTerm>("delete_glossary_term", { project, term });
}

/**
 * One glossary's terms, in file order. `slug` is `null` for the vault-wide
 * glossary. Fetched (and response-sequenced) via `useVaultQuery` and refetched
 * on every vault change, so a term added in another window — or the backend's
 * own broadcast after a write here — converges without caller wiring.
 *
 * `error` must be surfaced by the consumer: a hand-edited file with duplicate
 * terms fails to load, and an unreported failure looks exactly like an empty
 * glossary.
 */
export function useGlossary(slug: string | null): {
  terms: GlossaryTerm[];
  loading: boolean;
  error: string | null;
} {
  const { data, loading, error } = useVaultQuery(
    useCallback(
      () => invoke<GlossaryListing>("list_glossary_terms", { project: slug }),
      [slug],
    ),
  );

  return { terms: data?.terms ?? NO_TERMS, loading, error };
}
