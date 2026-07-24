//! The MCP `CallToolResult` envelope builders.
//!
//! Per the cross-cutting contract in `docs/MCP_TOOL_SURFACE.md`, a successful
//! call returns `structuredContent` (conforming to the tool's `outputSchema`)
//! *and* the same payload serialized as JSON in a `content` text block, so a
//! client that does not parse `structuredContent` still receives the full data.
//! A business error returns a human-readable `content` text block with
//! `isError: true` and no `structuredContent`.

use serde::Serialize;
use serde_json::{json, Value};

/// A successful result: the structured payload plus its JSON serialization as a
/// text content block.
pub fn success<T: Serialize>(payload: &T) -> Value {
    let structured = serde_json::to_value(payload).expect("tool payload serializes");
    let text = serde_json::to_string_pretty(&structured).expect("tool payload serializes");
    json!({
        "content": [ { "type": "text", "text": text } ],
        "structuredContent": structured,
        "isError": false
    })
}

/// A business error: a human-readable message with `isError: true` and no
/// `structuredContent`, so each tool's `outputSchema` stays the clean success
/// shape.
pub fn business_error(message: impl Into<String>) -> Value {
    json!({
        "content": [ { "type": "text", "text": message.into() } ],
        "isError": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_serializes_payload_into_both_channels() {
        let payload = json!({ "a": 1, "b": ["x", "y"] });
        let result = success(&payload);

        assert_eq!(result["isError"], Value::Bool(false));
        assert_eq!(result["structuredContent"], payload);
        assert_eq!(result["content"][0]["type"], "text");
        // The text block round-trips to exactly the structured payload.
        let text = result["content"][0]["text"].as_str().unwrap();
        let reparsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(reparsed, payload);
    }

    #[test]
    fn business_error_sets_flag_and_omits_structured_content() {
        let result = business_error("note not found: n_missing");
        assert_eq!(result["isError"], Value::Bool(true));
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result["content"][0]["text"], "note not found: n_missing");
    }
}
