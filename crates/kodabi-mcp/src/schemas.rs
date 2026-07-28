//! The eight tools' static metadata and their committed JSON Schemas.
//!
//! Six read tools (`search_notes`, `get_note`, `get_meeting_transcript`,
//! `list_outstanding_items`, `list_projects`, `get_project_context`) and two
//! write tools (`file_note_to_project`, `add_glossary_term`); the write tools
//! differ only in carrying `readOnlyHint: false`.
//!
//! Each schema is a verbatim copy of the matching block in
//! `docs/MCP_TOOL_SURFACE.md` (one file per tool per direction, under
//! `schemas/`), self-contained with the transitive subset of the shared `$defs`
//! inlined and `$ref`'d as `#/$defs/<Name>`. The files are parsed once and
//! assembled into the `tools/list` result. Committing them as literal JSON keeps
//! them diffable against the spec, and the tests below guard `$ref` resolution
//! and the open-`NoteSummary` invariant.

use serde_json::{json, Value};
use std::sync::OnceLock;

// The tool descriptions, verbatim from the spec. Named so a compile-time assert
// can hold each under 2 KB — Claude Code truncates a description past that size.
const SEARCH_NOTES_DESCRIPTION: &str = "Hybrid full-text + semantic search across all notes. Returns ranked hits with snippets. Filter by project (and subtree), note type, tags, and date range; page with limit + cursor.";
const GET_NOTE_DESCRIPTION: &str = "Fetch a note's full distilled content by stable id: frontmatter metadata plus the rendered markdown body. For meetings, also returns extracted decisions and action items. Use after search_notes to read a hit in full.";
const GET_MEETING_TRANSCRIPT_DESCRIPTION: &str = "Fetch the per-channel transcript (you/them attribution, millisecond offsets) and metadata for a meeting note by stable id. Returns transcript_available=false with empty segments when no transcript is stored; errors if the id is not a meeting note.";
const LIST_OUTSTANDING_ITEMS_DESCRIPTION: &str = "List action items that are not done (open/overdue), extracted from meetings and linked to their source note. Filter by project subtree, owner, status, due-before date, or source meeting.";
const LIST_PROJECTS_DESCRIPTION: &str = "Enumerate routing-target projects with hierarchy (parent + slug), display name, note/meeting counts, and last activity. Use to resolve a project name to its slug before filtering other tools.";
const GET_PROJECT_CONTEXT_DESCRIPTION: &str = "Aggregate context for one project in a single call: description, glossary, recent notes, outstanding items, and counts. Toggle and limit each section. Ideal for grounding a chat about a project.";
const FILE_NOTE_TO_PROJECT_DESCRIPTION: &str = "Route or re-route a note to a project (the human correction loop). Moves the file, updates its frontmatter project + confidence, preserves the stable id, and returns the new path. Mutating but reversible.";
const ADD_GLOSSARY_TERM_DESCRIPTION: &str = "Add or update a glossary term (term, definition, aliases) for a project. Upsert by normalized term. Used for transcription biasing and post-pass cleanup.";

const _: () = assert!(SEARCH_NOTES_DESCRIPTION.len() < 2048);
const _: () = assert!(GET_NOTE_DESCRIPTION.len() < 2048);
const _: () = assert!(GET_MEETING_TRANSCRIPT_DESCRIPTION.len() < 2048);
const _: () = assert!(LIST_OUTSTANDING_ITEMS_DESCRIPTION.len() < 2048);
const _: () = assert!(LIST_PROJECTS_DESCRIPTION.len() < 2048);
const _: () = assert!(GET_PROJECT_CONTEXT_DESCRIPTION.len() < 2048);
const _: () = assert!(FILE_NOTE_TO_PROJECT_DESCRIPTION.len() < 2048);
const _: () = assert!(ADD_GLOSSARY_TERM_DESCRIPTION.len() < 2048);

/// One tool's immutable definition.
struct ToolSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: &'static str,
    output_schema: &'static str,
    /// Drives the `readOnlyHint` annotation: `true` for the read tools, `false`
    /// for the two write tools (`file_note_to_project`, `add_glossary_term`).
    read_only: bool,
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "search_notes",
        title: "Search notes",
        description: SEARCH_NOTES_DESCRIPTION,
        input_schema: include_str!("../schemas/search_notes.input.json"),
        output_schema: include_str!("../schemas/search_notes.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "get_note",
        title: "Get note",
        description: GET_NOTE_DESCRIPTION,
        input_schema: include_str!("../schemas/get_note.input.json"),
        output_schema: include_str!("../schemas/get_note.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "get_meeting_transcript",
        title: "Get meeting transcript",
        description: GET_MEETING_TRANSCRIPT_DESCRIPTION,
        input_schema: include_str!("../schemas/get_meeting_transcript.input.json"),
        output_schema: include_str!("../schemas/get_meeting_transcript.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "list_outstanding_items",
        title: "List outstanding items",
        description: LIST_OUTSTANDING_ITEMS_DESCRIPTION,
        input_schema: include_str!("../schemas/list_outstanding_items.input.json"),
        output_schema: include_str!("../schemas/list_outstanding_items.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "list_projects",
        title: "List projects",
        description: LIST_PROJECTS_DESCRIPTION,
        input_schema: include_str!("../schemas/list_projects.input.json"),
        output_schema: include_str!("../schemas/list_projects.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "get_project_context",
        title: "Get project context",
        description: GET_PROJECT_CONTEXT_DESCRIPTION,
        input_schema: include_str!("../schemas/get_project_context.input.json"),
        output_schema: include_str!("../schemas/get_project_context.output.json"),
        read_only: true,
    },
    ToolSpec {
        name: "file_note_to_project",
        title: "File note to project",
        description: FILE_NOTE_TO_PROJECT_DESCRIPTION,
        input_schema: include_str!("../schemas/file_note_to_project.input.json"),
        output_schema: include_str!("../schemas/file_note_to_project.output.json"),
        read_only: false,
    },
    ToolSpec {
        name: "add_glossary_term",
        title: "Add glossary term",
        description: ADD_GLOSSARY_TERM_DESCRIPTION,
        input_schema: include_str!("../schemas/add_glossary_term.input.json"),
        output_schema: include_str!("../schemas/add_glossary_term.output.json"),
        read_only: false,
    },
];

/// The `readOnlyHint`/`destructiveHint`/`idempotentHint`/`openWorldHint` block.
/// `readOnlyHint` tracks the tool's `read_only` flag; the other three hints are
/// uniform across all eight tools (both write tools are reversible upserts, so
/// neither is destructive — see `docs/MCP_TOOL_SURFACE.md` §7/§8).
fn tool_annotations(read_only: bool) -> Value {
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false
    })
}

/// The assembled `tools/list` result — parsed and built once, then reused.
pub fn tools_list() -> &'static Value {
    static CELL: OnceLock<Value> = OnceLock::new();
    CELL.get_or_init(|| {
        let tools: Vec<Value> = TOOLS
            .iter()
            .map(|spec| {
                json!({
                    "name": spec.name,
                    "title": spec.title,
                    "description": spec.description,
                    "inputSchema": parse_schema(spec.input_schema),
                    "outputSchema": parse_schema(spec.output_schema),
                    "annotations": tool_annotations(spec.read_only)
                })
            })
            .collect();
        json!({ "tools": tools })
    })
}

/// Parses a committed schema file. The files are validated by the tests below,
/// so a parse failure here would be a build-time bug, not a runtime input fault.
fn parse_schema(raw: &str) -> Value {
    serde_json::from_str(raw).expect("committed schema is valid JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_schema_sources() -> Vec<(&'static str, &'static str)> {
        TOOLS
            .iter()
            .flat_map(|spec| {
                [
                    (spec.name, spec.input_schema),
                    (spec.name, spec.output_schema),
                ]
            })
            .collect()
    }

    fn collect_refs(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if key == "$ref" {
                        if let Some(reference) = child.as_str() {
                            out.push(reference.to_string());
                        }
                    } else {
                        collect_refs(child, out);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|item| collect_refs(item, out)),
            _ => {}
        }
    }

    /// The embedded terminal pre-approves a hard-coded list of tool names so
    /// chat-over-the-KB never prompts for a read. That list lives in
    /// kodabi-core (which cannot see this crate), so this is the only place the
    /// two can be compared — a read tool added here without a matching entry
    /// there would ship working but prompting on every call.
    #[test]
    fn every_read_tool_is_pre_approved_for_the_embedded_terminal() {
        use kodabi_core::terminal::READ_TOOL_PERMISSIONS;

        let expected: Vec<String> = TOOLS
            .iter()
            .filter(|spec| spec.read_only)
            .map(|spec| format!("mcp__kodabi__{}", spec.name))
            .collect();
        let approved: Vec<String> = READ_TOOL_PERMISSIONS
            .iter()
            .map(|name| (*name).to_string())
            .collect();

        assert_eq!(
            approved, expected,
            "READ_TOOL_PERMISSIONS must list exactly the read tools, in table order"
        );
        // And no write tool may sneak into the pre-approved set.
        for spec in TOOLS.iter().filter(|spec| !spec.read_only) {
            let namespaced = format!("mcp__kodabi__{}", spec.name);
            assert!(
                !approved.contains(&namespaced),
                "{namespaced} is a write tool and must still prompt"
            );
        }
    }

    #[test]
    fn every_schema_parses_and_all_refs_resolve() {
        for (name, raw) in all_schema_sources() {
            let schema: Value = serde_json::from_str(raw)
                .unwrap_or_else(|error| panic!("{name} schema is not valid JSON: {error}"));
            let defs = schema.get("$defs").and_then(Value::as_object);

            let mut refs = Vec::new();
            collect_refs(&schema, &mut refs);
            assert!(!refs.is_empty(), "{name}: expected at least one $ref");

            for reference in refs {
                let key = reference
                    .strip_prefix("#/$defs/")
                    .unwrap_or_else(|| panic!("{name}: unexpected $ref {reference:?}"));
                assert!(
                    defs.is_some_and(|d| d.contains_key(key)),
                    "{name}: $ref {reference:?} has no matching $defs entry"
                );
            }
        }
    }

    #[test]
    fn note_summary_stays_open_and_its_extensions_stay_closed() {
        for (name, raw) in all_schema_sources() {
            let schema: Value = serde_json::from_str(raw).unwrap();
            let Some(defs) = schema.get("$defs").and_then(Value::as_object) else {
                continue;
            };

            // The base being extended must carry neither keyword, or the
            // allOf-composed instances are rejected (see spec Conventions).
            if let Some(note_summary) = defs.get("NoteSummary") {
                assert!(
                    note_summary.get("additionalProperties").is_none(),
                    "{name}: NoteSummary must stay open (no additionalProperties)"
                );
                assert!(
                    note_summary.get("unevaluatedProperties").is_none(),
                    "{name}: NoteSummary must stay open (no unevaluatedProperties)"
                );
            }

            // The extensions re-close the full shape with unevaluatedProperties.
            for extension in ["SearchHit", "MeetingMeta"] {
                if let Some(schema_ext) = defs.get(extension) {
                    assert_eq!(
                        schema_ext.get("unevaluatedProperties"),
                        Some(&Value::Bool(false)),
                        "{name}: {extension} must set unevaluatedProperties: false"
                    );
                }
            }
        }
    }

    #[test]
    fn tools_list_has_every_tool_with_annotations_and_bounded_descriptions() {
        let list = tools_list();
        let tools = list["tools"].as_array().unwrap();

        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "search_notes",
                "get_note",
                "get_meeting_transcript",
                "list_outstanding_items",
                "list_projects",
                "get_project_context",
                "file_note_to_project",
                "add_glossary_term",
            ]
        );

        // The two write tools carry readOnlyHint: false; the reads carry true.
        // Every other hint is uniform across the eight.
        let write_tools = ["file_note_to_project", "add_glossary_term"];
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            let expected_read_only = !write_tools.contains(&name);
            assert!(tool["title"].is_string());
            assert!(tool["description"].as_str().unwrap().len() < 2048);
            assert!(tool["inputSchema"].is_object());
            assert!(tool["outputSchema"].is_object());
            assert_eq!(
                tool["annotations"]["readOnlyHint"],
                Value::Bool(expected_read_only),
                "{name}: unexpected readOnlyHint"
            );
            assert_eq!(tool["annotations"]["destructiveHint"], Value::Bool(false));
            assert_eq!(tool["annotations"]["idempotentHint"], Value::Bool(true));
            assert_eq!(tool["annotations"]["openWorldHint"], Value::Bool(false));
        }
    }
}
