//! `sumcp` — human CLI over the same Report the MCP server serves.
//!
//! T2.1 slice: `sumcp --file <path>` parses a transcript and prints its
//! overview. Bare-invocation "latest session" discovery (ADR A4 CLI mode) and
//! the HTML report arrive in later tasks.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

/// Post-session forensics for Claude Code sessions.
#[derive(Parser)]
#[command(name = "sumcp", version, about)]
struct Args {
    /// Path to a transcript `.jsonl` to analyze.
    #[arg(long)]
    file: Option<PathBuf>,
}

// `ExitCode` lets `main` return a process status without calling exit().
fn main() -> ExitCode {
    let args = Args::parse();

    let Some(path) = args.file else {
        // `let ... else` handles the None case by diverging (here: return).
        eprintln!(
            "usage: sumcp --file <transcript.jsonl>  (latest-session mode lands in a later task)"
        );
        return ExitCode::FAILURE;
    };

    match sumcp_core::overview_of_file(&path) {
        Ok(overview) => {
            print!("{}", overview.to_text());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("could not read {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}
