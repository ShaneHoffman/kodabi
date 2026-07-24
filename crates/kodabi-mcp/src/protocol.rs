//! JSON-RPC 2.0 value types, error codes, and the newline-delimited stdio loop.
//!
//! MCP over stdio frames each message as one line of compact JSON on stdin, and
//! each response as one line on stdout. Stdout is the exclusive protocol
//! channel: diagnostics go to stderr only, or the JSON-RPC stream is corrupted.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::dispatch;
use crate::server::Server;

/// The JSON-RPC version string every message carries.
pub const JSONRPC_VERSION: &str = "2.0";

/// Emitted only if a response somehow fails to serialize — should never happen,
/// but keeps the stream well-formed if it does.
const FALLBACK_ERROR: &str = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"failed to serialize response"}}"#;

/// A JSON-RPC error object.
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// `-32700`: the request was not valid JSON.
    pub fn parse_error() -> Self {
        Self::new(-32700, "Parse error: request was not valid JSON")
    }

    /// `-32600`: the request was not a valid JSON-RPC object.
    pub fn invalid_request() -> Self {
        Self::new(-32600, "Invalid Request")
    }

    /// `-32601`: no such method.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("Method not found: {method}"))
    }

    /// `-32602`: malformed or schema-invalid parameters (also unknown tool, bad
    /// cursor/date, oversized filter).
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    /// `-32603`: an internal server fault (storage error, unavailable backend).
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }
}

/// Builds a JSON-RPC success response.
pub fn success_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "result": result })
}

/// Builds a JSON-RPC error response.
pub fn error_response(id: Value, error: RpcError) -> Value {
    let mut body = json!({ "code": error.code, "message": error.message });
    if let Some(data) = error.data {
        body["data"] = data;
    }
    json!({ "jsonrpc": JSONRPC_VERSION, "id": id, "error": body })
}

/// Reads newline-delimited JSON-RPC requests from `reader` and writes each
/// response to `writer`, flushing per message. Notifications (no `id`) draw no
/// response. Returns when the reader reaches EOF.
pub fn serve<R: BufRead, W: Write>(server: &Server, reader: R, writer: &mut W) -> io::Result<()> {
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(message) => dispatch::handle_message(server, message),
            Err(_) => Some(error_response(Value::Null, RpcError::parse_error())),
        };
        if let Some(response) = response {
            let serialized =
                serde_json::to_string(&response).unwrap_or_else(|_| FALLBACK_ERROR.to_string());
            writer.write_all(serialized.as_bytes())?;
            writer.write_all(b"\n")?;
            writer.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn responses_for(input: &str) -> Vec<Value> {
        let server = Server::without_backend();
        let mut output = Vec::new();
        serve(&server, Cursor::new(input.as_bytes().to_vec()), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn serve_answers_requests_skips_notifications_and_reports_parse_errors() {
        let input = concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
            "\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
            "this is not json\n",
        );
        let responses = responses_for(input);

        // initialize + ping + parse-error; the notification and blank line draw
        // no response.
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0]["id"], 1);
        assert_eq!(responses[0]["result"]["serverInfo"]["name"], "kodabi");
        assert_eq!(responses[1]["id"], 2);
        assert_eq!(responses[1]["result"], json!({}));
        assert_eq!(responses[2]["id"], Value::Null);
        assert_eq!(responses[2]["error"]["code"], -32700);
    }
}
