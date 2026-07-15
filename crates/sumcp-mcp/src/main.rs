//! `sumcp-mcp` — MCP server over stdio (T4.1).
//!
//! Will expose the six read-only tools with fail-closed session
//! identification (ADR A4). Async runtime stays confined to this binary.

fn main() {
    // T4.1 wires rmcp here; the scaffold only proves the crate builds.
    eprintln!("sumcp-mcp scaffold — server lands in T4.1");
}
