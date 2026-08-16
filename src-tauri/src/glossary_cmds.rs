//! Thin Tauri command wrappers over `kodabi_core::vault`'s glossary family.
//! Scope resolution, validation, conflict and rename semantics, and the atomic
//! file write all live in `kodabi-core`; these commands only own the serde IPC
//! DTOs, resolve the knowledge-base root, and broadcast the vault refresh.
//! Errors cross IPC as user-facing copy (see `user_errors`), with the raw
//! detail going to stderr, the convention `note_cmds` uses.
//!
//! A glossary has two scopes, and `project: None` is not "no scope" but the
//! **vault-wide** one: the `_glossary.yml` at the knowledge-base root, which is
//! what the transcription pipeline loads to bias every capture (a session is
//! transcribed before routing has picked a project). A `Some(slug)` scope is
//! that project's own glossary, which feeds routing signals and project context.

use kodabi_core::glossary::{GlossaryTerm, OnConflict};
use kodabi_core::vault::{self, GlossaryListing, GlossaryWrite};
use tauri::{AppHandle, Emitter};

use crate::events::VAULT_CHANGED_EVENT;
use crate::transcribe::knowledge_base_dir;
use crate::user_errors::glossary_error;

/// The I/O sentence for the three writing commands. A failed write leaves
/// `_glossary.yml` exactly as it was, which is the reassurance worth carrying.
const WRITE_FAILED: &str = "Couldn't save the glossary. The file is unchanged; try again.";

/// Broadcasts `vault:changed` after a glossary write. The file watcher filters
/// non-Markdown paths, so `_glossary.yml` never triggers a reconcile of its own
/// — this broadcast is the only thing that converges an open window.
fn broadcast_vault_changed(app: &AppHandle) {
    let _ = app.emit(VAULT_CHANGED_EVENT, ());
}

/// One glossary entry. Mirrors `kodabi_core::glossary::GlossaryTerm`.
#[derive(serde::Serialize)]
pub struct GlossaryTermDto {
    term: String,
    definition: String,
    aliases: Vec<String>,
}

/// A glossary's full contents. Mirrors `kodabi_core::vault::GlossaryListing`.
#[derive(serde::Serialize)]
pub struct GlossaryListingDto {
    /// `null` for the vault-wide glossary, else the canonical project slug.
    project: Option<String>,
    terms: Vec<GlossaryTermDto>,
}

/// The editable fields of a term, plus the scope to write it to. An input
/// struct rather than loose arguments so the wire keeps snake_case field names
/// (Tauri converts bare command arguments to camelCase).
///
/// `deny_unknown_fields` so a field renamed on one side of the boundary is a
/// loud failure rather than a value that silently never arrives — the MCP
/// tool's params struct takes the same position.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlossaryTermInput {
    /// `null` targets the vault-wide glossary at the knowledge-base root.
    project: Option<String>,
    term: String,
    definition: String,
    aliases: Vec<String>,
}

/// A term edit: the same fields plus which entry is being replaced, so a
/// rename can move `original_term` to a new `term`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateGlossaryTermInput {
    project: Option<String>,
    original_term: String,
    term: String,
    definition: String,
    aliases: Vec<String>,
}

impl From<GlossaryTermInput> for GlossaryTerm {
    fn from(input: GlossaryTermInput) -> Self {
        GlossaryTerm {
            term: input.term,
            definition: input.definition,
            aliases: input.aliases,
        }
    }
}

impl From<GlossaryTerm> for GlossaryTermDto {
    fn from(term: GlossaryTerm) -> Self {
        GlossaryTermDto {
            term: term.term,
            definition: term.definition,
            aliases: term.aliases,
        }
    }
}

impl From<GlossaryListing> for GlossaryListingDto {
    fn from(listing: GlossaryListing) -> Self {
        GlossaryListingDto {
            project: listing.project,
            terms: listing
                .terms
                .into_iter()
                .map(GlossaryTermDto::from)
                .collect(),
        }
    }
}

impl From<GlossaryWrite> for GlossaryTermDto {
    fn from(write: GlossaryWrite) -> Self {
        GlossaryTermDto::from(write.term)
    }
}

/// Reads a glossary's terms via `vault::list_glossary_terms`, in file order.
#[tauri::command]
pub async fn list_glossary_terms(
    app: AppHandle,
    project: Option<String>,
) -> Result<GlossaryListingDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let listing = vault::list_glossary_terms(&kb, project.as_deref()).map_err(|err| {
        glossary_error(
            "list_glossary_terms",
            err,
            "Couldn't read the glossary file. The file is untouched; reopen this view to try \
             again.",
        )
    })?;
    Ok(listing.into())
}

/// Adds a term via `vault::upsert_glossary_term`.
///
/// `OnConflict::Error`, deliberately: the editor's add is a create, so an
/// existing term surfaces as a conflict the user can see rather than silently
/// overwriting a definition they did not open.
#[tauri::command]
pub async fn add_glossary_term(
    app: AppHandle,
    input: GlossaryTermInput,
) -> Result<GlossaryTermDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let project = input.project.clone();
    let write =
        vault::upsert_glossary_term(&kb, project.as_deref(), input.into(), OnConflict::Error)
            .map_err(|err| glossary_error("add_glossary_term", err, WRITE_FAILED))?;
    broadcast_vault_changed(&app);
    Ok(write.into())
}

/// Edits a term in place via `vault::update_glossary_term`, preserving its
/// position in the file. A `term` differing from `original_term` is a rename.
#[tauri::command]
pub async fn update_glossary_term(
    app: AppHandle,
    input: UpdateGlossaryTermInput,
) -> Result<GlossaryTermDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let replacement = GlossaryTerm {
        term: input.term,
        definition: input.definition,
        aliases: input.aliases,
    };
    let write = vault::update_glossary_term(
        &kb,
        input.project.as_deref(),
        &input.original_term,
        replacement,
    )
    .map_err(|err| glossary_error("update_glossary_term", err, WRITE_FAILED))?;
    broadcast_vault_changed(&app);
    Ok(write.into())
}

/// Removes a term via `vault::remove_glossary_term`, echoing the entry removed.
#[tauri::command]
pub async fn delete_glossary_term(
    app: AppHandle,
    project: Option<String>,
    term: String,
) -> Result<GlossaryTermDto, String> {
    let kb = knowledge_base_dir(&app)?;
    let write = vault::remove_glossary_term(&kb, project.as_deref(), &term)
        .map_err(|err| glossary_error("delete_glossary_term", err, WRITE_FAILED))?;
    broadcast_vault_changed(&app);
    Ok(write.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact payloads `src/useGlossary.ts` sends. This is the one seam no
    /// other tier can see: `invokeParity` checks command *names* only, and the
    /// frontend suite mocks the IPC boundary, so a field renamed on one side
    /// would pass both and fail at runtime. `deny_unknown_fields` is what makes
    /// the assertion bite — without it a stale name would deserialize into the
    /// default and silently lose the value.
    #[test]
    fn add_input_deserializes_the_shape_the_frontend_sends() {
        let json = serde_json::json!({
            "project": null,
            "term": "MERIDIAN",
            "definition": "A systems-migration project.",
            "aliases": ["meridian", "mer-idian"],
        });

        let input: GlossaryTermInput = serde_json::from_value(json).unwrap();

        // `project: null` is the vault-wide scope, not a missing field.
        assert_eq!(input.project, None);
        let term: GlossaryTerm = input.into();
        assert_eq!(term.term, "MERIDIAN");
        assert_eq!(term.aliases, vec!["meridian", "mer-idian"]);
    }

    #[test]
    fn update_input_keeps_original_term_snake_case() {
        let json = serde_json::json!({
            "project": "Growth",
            // The field most at risk: an input struct is used precisely so this
            // stays snake_case instead of being camelCased like a bare command
            // argument would be.
            "original_term": "Meridan",
            "term": "MERIDIAN",
            "definition": "Fixed.",
            "aliases": [],
        });

        let input: UpdateGlossaryTermInput = serde_json::from_value(json).unwrap();

        assert_eq!(input.project.as_deref(), Some("Growth"));
        assert_eq!(input.original_term, "Meridan");
        assert_eq!(input.term, "MERIDIAN");
    }

    #[test]
    fn a_camel_cased_original_term_is_rejected_rather_than_silently_dropped() {
        let json = serde_json::json!({
            "project": null,
            "originalTerm": "Meridan",
            "term": "MERIDIAN",
            "definition": "Fixed.",
            "aliases": [],
        });

        assert!(serde_json::from_value::<UpdateGlossaryTermInput>(json).is_err());
    }

    #[test]
    fn a_listing_serializes_with_its_scope() {
        let listing = GlossaryListingDto::from(vault::GlossaryListing {
            project: Some("Growth".to_string()),
            terms: vec![GlossaryTerm {
                term: "TeeTrack".to_string(),
                definition: "Tee-sheet vendor.".to_string(),
                aliases: vec!["t-track".to_string()],
            }],
        });

        let json = serde_json::to_value(&listing).unwrap();

        assert_eq!(json["project"], "Growth");
        assert_eq!(json["terms"][0]["term"], "TeeTrack");
        assert_eq!(json["terms"][0]["aliases"][0], "t-track");
    }

    #[test]
    fn a_vault_wide_listing_serializes_its_scope_as_null() {
        let listing = GlossaryListingDto::from(vault::GlossaryListing {
            project: None,
            terms: Vec::new(),
        });

        let json = serde_json::to_value(&listing).unwrap();

        // The TS wire type reads `project: string | null`, so the absent scope
        // has to arrive as an explicit null rather than a dropped key.
        assert!(json["project"].is_null());
        assert!(json["terms"].as_array().unwrap().is_empty());
    }
}
