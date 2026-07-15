//! `sumcp` — human CLI over the same Report the MCP server serves.
//!
//! Bare invocation will print the latest session's debrief (T2.1); this is
//! the zero-config first-60-seconds experience from docs/ideas/sumcp.md.

use clap::Parser;

/// Post-session forensics for Claude Code sessions.
#[derive(Parser)]
#[command(name = "sumcp", version, about)]
struct Args {}

fn main() {
    let _args = Args::parse();
    // T2.1 wires this to sumcp-core's locate → ingest → report pipeline.
    println!("sumcp v0.1 scaffold — analysis lands in T2.1");
}
