//! `sumcp` — human CLI over the same Report the MCP server serves.
//!
//! `sumcp --file <path>` prints the overview + ranked struggle areas.
//! `--json` emits the `session_overview` payload (the frozen v0 contract).

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;
use sumcp_core::ingest::ingest_str;
use sumcp_core::model::Lane;
use sumcp_core::payloads::{SessionMeta, session_overview};
use sumcp_core::score::{Weights, rank};

/// Post-session forensics for Claude Code sessions.
#[derive(Parser)]
#[command(name = "sumcp", version, about)]
struct Args {
    /// Path to a transcript `.jsonl` to analyze.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Emit the session_overview JSON payload instead of the text view.
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let Some(path) = args.file else {
        eprintln!("usage: sumcp --file <transcript.jsonl> [--json]");
        return ExitCode::FAILURE;
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("could not read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };

    let session = ingest_str(&raw, Lane::Main);
    let ranked = rank(&session, &Weights::default());
    // CLI resolves the session by path, so provenance is "explicit".
    let meta = SessionMeta {
        id: path
            .file_stem()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_default(),
        identified_by: "explicit".into(),
    };

    if args.json {
        let payload = session_overview(&session, &ranked, &meta);
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return ExitCode::SUCCESS;
    }

    print!(
        "{}",
        sumcp_core::report::Overview::from_session(&session).to_text()
    );
    if ranked.is_empty() {
        println!("no struggle signals fired.");
    } else {
        println!("\n── struggle areas ──");
        for (i, f) in ranked.iter().take(5).enumerate() {
            let cats: Vec<String> = f
                .breakdown
                .iter()
                .map(|(k, v)| format!("{k} {v}"))
                .collect();
            println!(
                "{}. {}  (score {:.1}: {})",
                i + 1,
                f.file,
                f.score,
                cats.join(", ")
            );
        }
    }
    ExitCode::SUCCESS
}
