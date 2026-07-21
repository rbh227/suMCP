//! `sumcp` — human CLI over the same Report the MCP server serves.
//!
//! `sumcp --file <path>` prints the overview + ranked struggle areas.
//! `--json` emits the `session_overview` payload (the frozen v0 contract).

mod install;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;
use sumcp_core::payloads::{SessionMeta, session_overview};
use sumcp_core::score::{Weights, rank};

/// Post-session forensics for Claude Code sessions.
#[derive(Parser)]
#[command(name = "sumcp", version, about)]
struct Args {
    /// Optional subcommand. When omitted, the legacy `--file` analysis path runs.
    #[command(subcommand)]
    command: Option<Command>,
    /// Path to a transcript `.jsonl` to analyze.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Emit the session_overview JSON payload instead of the text view.
    #[arg(long)]
    json: bool,
    /// Render the self-contained HTML report to stdout.
    #[arg(long)]
    html: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Register the MCP server, debrief skill, and Stop hook in `~/.claude`.
    /// Dry-run by default; pass `--apply` to write.
    Install {
        /// Actually perform the writes (default is a dry-run preview).
        #[arg(long)]
        apply: bool,
    },
    /// Remove everything a previous `install` created (manifest-tracked).
    /// Dry-run by default; pass `--apply` to write.
    Uninstall {
        /// Actually perform the removals (default is a dry-run preview).
        #[arg(long)]
        apply: bool,
    },
}

fn main() -> ExitCode {
    let args = Args::parse();

    // Subcommands (the write path) short-circuit the analysis flow.
    match args.command {
        Some(Command::Install { apply }) => {
            return match install::cmd_install(apply) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("install failed: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Some(Command::Uninstall { apply }) => {
            return match install::cmd_uninstall(apply) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("uninstall failed: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        None => {}
    }

    let Some(path) = args.file else {
        eprintln!("usage: sumcp --file <transcript.jsonl> [--json|--html]");
        eprintln!("       sumcp install [--apply]   |   sumcp uninstall [--apply]");
        return ExitCode::FAILURE;
    };

    // `load_session` does more than read one file: it ingests the main
    // transcript AND looks for sibling subagent transcripts next to it,
    // flat-merging any it finds into a single `Session`. What it can't find
    // it records honestly (see `flags.subagent_files_missing`) rather than
    // silently dropping. It returns an `Assembled { session, subagent_paths }`
    // (or an io::Error if the main file can't be read / is too large), so we
    // pull `.session` out and proceed exactly as before.
    let assembled =
        match sumcp_core::assemble::load_session(&path, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES)
        {
            Ok(a) => a,
            Err(e) => {
                eprintln!("could not load {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
    let session = assembled.session;
    let ranked = rank(&session, &Weights::default());
    // CLI resolves the session by path, so provenance is "explicit".
    let meta = SessionMeta {
        id: path
            .file_stem()
            .map(|s| s.to_string_lossy().into())
            .unwrap_or_default(),
        identified_by: "explicit".into(),
    };

    if args.html {
        print!(
            "{}",
            sumcp_core::html::render_html(&session, &ranked, &Weights::default(), &meta)
        );
        return ExitCode::SUCCESS;
    }

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
