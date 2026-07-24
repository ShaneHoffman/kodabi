//! `add_glossary_term`: upsert a project glossary term by normalized term.

use serde_json::{json, Value};

use kodabi_core::glossary::{GlossaryTerm, OnConflict};
use kodabi_core::vault;

use super::map_add_glossary_term_error;
use crate::envelope;
use crate::protocol::RpcError;
use crate::server::Server;

/// The wire spelling of the `on_conflict` enum. A local shim so the core
/// `OnConflict` stays free of a serde wire representation.
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum OnConflictArg {
    Update,
    Error,
}

impl From<OnConflictArg> for OnConflict {
    fn from(value: OnConflictArg) -> Self {
        match value {
            OnConflictArg::Update => OnConflict::Update,
            OnConflictArg::Error => OnConflict::Error,
        }
    }
}

fn default_on_conflict() -> OnConflictArg {
    OnConflictArg::Update
}

/// `add_glossary_term` arguments.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AddGlossaryTermParams {
    project: String,
    term: String,
    definition: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default = "default_on_conflict")]
    on_conflict: OnConflictArg,
}

/// Adds or updates a project glossary term through the shared
/// `vault::add_glossary_term` core function (upsert by normalized term into the
/// project's `_glossary.yml`). A malformed slug or field is a protocol error
/// (`invalid_params`); a missing project or an `on_conflict: "error"` hit are
/// business errors (`isError`).
pub fn call(server: &Server, arguments: Value) -> Result<Value, RpcError> {
    let backend = server.backend()?;
    let params: AddGlossaryTermParams = serde_json::from_value(arguments).map_err(|error| {
        RpcError::invalid_params(format!("invalid add_glossary_term arguments: {error}"))
    })?;

    let term = GlossaryTerm {
        term: params.term,
        definition: params.definition,
        aliases: params.aliases,
    };
    match vault::add_glossary_term(
        &backend.config.kb_root,
        &params.project,
        term,
        params.on_conflict.into(),
    ) {
        Ok(outcome) => {
            // The wire `GlossaryTerm` carries `project` (the on-disk struct does
            // not — it is implicit in the folder), so inject the canonical slug.
            let payload = json!({
                "term": {
                    "term": outcome.term.term,
                    "definition": outcome.term.definition,
                    "aliases": outcome.term.aliases,
                    "project": outcome.project,
                },
                "created": outcome.created,
            });
            Ok(envelope::success(&payload))
        }
        Err(error) => map_add_glossary_term_error(error),
    }
}
