//! Binary entry point for the Kodabi stdio MCP server. All logic lives in the
//! `kodabi_mcp` library crate so it is unit-testable; this shell only forwards
//! the process exit code.

fn main() {
    std::process::exit(kodabi_mcp::run());
}
