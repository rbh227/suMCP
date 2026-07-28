//! `sumcp`: a human CLI over the same Report the MCP server serves.
//!
//! Three entry points, three scopes. Bare `sumcp` needs no arguments: it
//! analyzes the whole work unit (every transcript in the same continuous
//! stretch of work) containing the most recent session of the current
//! project. `--work-unit <path>` analyzes the work unit containing that
//! specific transcript. `--file <path>` is the odd one out on purpose: an
//! explicit path names an explicit scope, so it stays single-transcript even
//! when that transcript is part of a larger unit (it prints a stderr note
//! when that happens). Either way it prints the overview + ranked struggle
//! areas, or the `session_overview` payload (the v1 contract) under `--json`.

mod install;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use sumcp_core::payloads::{SessionMeta, session_overview, unit_meta_from};
use sumcp_core::score::rank;

/// Post-session forensics for Claude Code sessions.
#[derive(Parser)]
#[command(name = "sumcp", version, about)]
struct Args {
    /// Optional subcommand. When omitted, the analysis path runs.
    #[command(subcommand)]
    command: Option<Command>,
    /// Path to a transcript `.jsonl` to analyze. Defaults to the most recent
    /// session of the current directory's project. An explicit path names an
    /// explicit scope, so this stays single-transcript even when the named
    /// file is part of a larger work unit (see `--work-unit`).
    #[arg(long)]
    file: Option<PathBuf>,
    /// Analyze the whole work unit containing this transcript: every
    /// transcript in the same continuous stretch of work.
    #[arg(long, conflicts_with = "file")]
    work_unit: Option<PathBuf>,
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

/// The transcript we are going to analyze, plus how we chose it.
#[derive(Debug)]
struct Target {
    /// Path to the main transcript `.jsonl`.
    path: PathBuf,
    /// ADR A4 provenance: `"explicit"` when the user named the file,
    /// `"cli_latest"` when we inferred it from recency.
    identified_by: &'static str,
}

/// Why there is nothing to analyze. An enum rather than a string so the
/// caller (which owns all the printing) cannot mix the two cases up: they
/// need very different advice.
#[derive(Debug, PartialEq)]
enum NoTarget {
    /// We could not work out where to look at all (no `$HOME`, or the cwd is
    /// unreadable), so we never got as far as a project directory.
    NowhereToLook,
    /// We looked in this project dir and it holds no session transcripts.
    NoSessions(PathBuf),
}

/// Decide which transcript to analyze.
///
/// `explicit_path` is whichever of `--file` or `--work-unit` the caller gave
/// (clap's `conflicts_with` guarantees at most one is `Some`); either always
/// wins, because the user naming a path is the strongest signal there is.
/// It is also resolved BEFORE we look at `claude_home`, so `sumcp --file
/// x.jsonl` keeps working where there is no `~/.claude` and no `$HOME` at all
/// (a CI container, say): hence the `Option` arguments. This function only
/// picks the PATH; whether that path is read as one transcript or as its
/// whole work unit is decided afterward, by which flag supplied it. With
/// neither flag we fall back to "the session I last worked in here", which
/// is what a human at a terminal means by a bare `sumcp`.
///
/// Pure: no env, no printing, so tests can drive it against a temp tree. The
/// `NoSessions` case carries the directory we searched, because that path is
/// the only thing that makes the failure actionable (it usually means the
/// user is one directory below the root Claude Code was launched from).
fn resolve_target(
    file: Option<PathBuf>,
    claude_home: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<Target, NoTarget> {
    if let Some(path) = file {
        return Ok(Target {
            path,
            identified_by: "explicit",
        });
    }
    // Both are needed to name the project directory; either one missing means
    // we have no question to ask the filesystem.
    let (Some(home), Some(cwd)) = (claude_home, cwd) else {
        return Err(NoTarget::NowhereToLook);
    };
    let project_dir = sumcp_core::locate::project_dir(home, cwd);
    match sumcp_core::locate::newest_transcript(&project_dir) {
        Some(path) => Ok(Target {
            path,
            identified_by: "cli_latest",
        }),
        None => Err(NoTarget::NoSessions(project_dir)),
    }
}

/// The session id a transcript path carries as its file stem. A `--file` the
/// user renamed has no uuid stem, so this is a label, never a validated id.
fn stem_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `~/.claude`, overridable via `SUMCP_CLAUDE_HOME` (tests point this at a
/// fixture tree; there is no other reason to set it). Mirrors the same
/// resolution the MCP server does, so both halves read the same transcripts.
fn claude_home() -> Option<PathBuf> {
    claude_home_from(
        std::env::var_os("SUMCP_CLAUDE_HOME").map(PathBuf::from),
        sumcp_core::home_dir(),
    )
}

/// The pure core of [`claude_home`] (env-free, so tests can drive it).
fn claude_home_from(override_dir: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    override_dir.or_else(|| home.map(|h| h.join(".claude")))
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

    // These two lookups can fail (an environment with no `$HOME`, a deleted
    // cwd); `resolve_target` decides whether that actually mattered, since
    // `--file` and `--work-unit` both need neither.
    let home = claude_home();
    let cwd = std::env::current_dir().ok();
    // `--file` and `--work-unit` conflict (clap enforces it, see `Args`), so
    // at most one of these is `Some`. `resolve_target` only decides the
    // PATH; which of the two scopes below reads that path is decided just
    // below, by which flag (if either) supplied it.
    let explicit_path = args.file.clone().or_else(|| args.work_unit.clone());
    let target = match resolve_target(explicit_path, home.as_deref(), cwd.as_deref()) {
        Ok(t) => t,
        Err(why) => {
            match why {
                NoTarget::NowhereToLook => {
                    eprintln!("sumcp: cannot tell which project this is (no HOME, or the current");
                    eprintln!("       directory is unreadable).");
                }
                NoTarget::NoSessions(searched) => {
                    eprintln!("sumcp: no Claude Code sessions found for this project.");
                    // `expect` is safe: NoSessions is only built once cwd is Some.
                    let cwd = cwd.expect("cwd was resolved to reach NoSessions");
                    eprintln!("  cwd:      {}", cwd.display());
                    eprintln!("  searched: {}", searched.display());
                    eprintln!("Claude Code stores transcripts per project directory, so run sumcp");
                    eprintln!("from the directory you launched Claude Code in.");
                }
            }
            eprintln!("To analyze a specific transcript instead:");
            eprintln!("  sumcp --file <transcript.jsonl> [--json|--html]");
            eprintln!("  sumcp install [--apply]   |   sumcp uninstall [--apply]");
            return ExitCode::FAILURE;
        }
    };
    let path = target.path;
    // The user never named this file, so say which session we picked. It goes
    // to stderr on purpose: `--json`/`--html` stdout must stay pipeable.
    if target.identified_by == "cli_latest" {
        eprintln!(
            "sumcp: analyzing most recent session {} ({})",
            stem_id(&path),
            path.display()
        );
    }

    // Three entry points, three scopes. `--file` is explicit and stays
    // single-transcript, because an explicit path means an explicit scope.
    // Everything else (bare `sumcp`, or `--work-unit`) reports the whole
    // stretch of work, which is what the user actually just did.
    let (session, unit_meta) = if args.file.is_some() {
        // `load_session` does more than read one file: it ingests the main
        // transcript AND looks for sibling subagent transcripts next to it,
        // flat-merging any it finds into a single `Session`. What it can't
        // find it records honestly (see `flags.subagent_files_missing`)
        // rather than silently dropping. It returns an
        // `Assembled { session, subagent_paths }` (or an io::Error if the
        // main file can't be read / is too large), so we pull `.session`
        // out and proceed exactly as before.
        let assembled = match sumcp_core::assemble::load_session(
            &path,
            sumcp_core::assemble::MAX_TRANSCRIPT_BYTES,
        ) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("could not load {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        // Tell the user, on stderr so stdout stays pipeable, when the
        // transcript they named is only part of a larger stretch. Without
        // this a user can read a 51-edit report with 224 more edits beside
        // it and no indication they exist.
        let unit = sumcp_core::work_unit::discover_work_unit(&path);
        if unit.members.len() > 1 {
            // `unit.members` is ordered oldest first. Position in the work unit
            // counts chronologically: the oldest transcript is 1, the newest is N.
            let at = unit
                .members
                .iter()
                .position(|m| m.path == path)
                .map(|i| i + 1)
                .unwrap_or(1);
            eprintln!(
                "note: this transcript is {at} of {} in a work unit; use --work-unit to analyze all of it",
                unit.members.len()
            );
        }
        (assembled.session, None)
    } else {
        // Bare `sumcp` and `--work-unit` both want the whole stretch of
        // work, so both go through `load_work_unit`: it discovers every
        // transcript in the same continuous stretch as `path` and merges
        // them into one total order (see `assemble::load_work_unit`'s doc).
        let assembled = match sumcp_core::assemble::load_work_unit(
            &path,
            sumcp_core::assemble::MAX_TRANSCRIPT_BYTES,
        ) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("could not load {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        };
        let meta = unit_meta_from(&assembled);
        (assembled.session, Some(meta))
    };
    let ranked = rank(&session);
    let meta = SessionMeta {
        id: stem_id(&path),
        identified_by: target.identified_by.into(),
        unit: unit_meta,
    };

    if args.html {
        print!(
            "{}",
            sumcp_core::html::render_html(&session, &ranked, &meta)
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
                "{}. {}  ({}, edited {}x: {})",
                i + 1,
                f.file,
                f.class.as_str(),
                f.edits,
                cats.join(", ")
            );
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    const ID_OLD: &str = "5717aaaa-1111-2222-3333-444455556666";
    const ID_NEW: &str = "80b9a169-624f-4880-a2c3-24b96e2b4ea2";

    /// Build a fake `~/.claude` holding transcripts for `cwd`'s project, each
    /// with an explicit mtime so "newest" is deterministic rather than a race
    /// between two writes in the same filesystem timestamp tick.
    fn fake_home(claude_home: &Path, cwd: &Path, sessions: &[(&str, u64)]) {
        let dir = sumcp_core::locate::project_dir(claude_home, cwd);
        std::fs::create_dir_all(&dir).unwrap();
        for (id, mtime_secs) in sessions {
            let path = dir.join(format!("{id}.jsonl"));
            std::fs::write(&path, "{}").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(*mtime_secs))
                .unwrap();
        }
    }

    #[test]
    fn bare_invocation_targets_the_newest_session_for_this_cwd() {
        let td = tempfile::tempdir().unwrap();
        let (home, cwd) = (td.path().join("claude"), td.path().join("proj"));
        fake_home(&home, &cwd, &[(ID_OLD, 1_000_000), (ID_NEW, 2_000_000)]);

        let target = resolve_target(None, Some(&home), Some(&cwd)).unwrap();
        assert!(target.path.ends_with(format!("{ID_NEW}.jsonl")));
        // ADR A4 provenance: recency is a guess, and the payload must say so.
        assert_eq!(target.identified_by, "cli_latest");
    }

    #[test]
    fn explicit_file_wins_over_recency_and_never_touches_the_project_dir() {
        let td = tempfile::tempdir().unwrap();
        let (home, cwd) = (td.path().join("claude"), td.path().join("proj"));
        fake_home(&home, &cwd, &[(ID_NEW, 2_000_000)]);
        let chosen = td.path().join("elsewhere.jsonl");

        let target = resolve_target(Some(chosen.clone()), Some(&home), Some(&cwd)).unwrap();
        assert_eq!(target.path, chosen);
        assert_eq!(target.identified_by, "explicit");
    }

    #[test]
    fn explicit_file_needs_neither_a_home_nor_a_cwd() {
        // A container with no $HOME must still be able to run `sumcp --file`.
        let target = resolve_target(Some("t.jsonl".into()), None, None).unwrap();
        assert_eq!(target.path, PathBuf::from("t.jsonl"));
    }

    #[test]
    fn no_sessions_for_this_project_reports_the_dir_it_searched() {
        let td = tempfile::tempdir().unwrap();
        let (home, cwd) = (td.path().join("claude"), td.path().join("proj"));
        // Never opened in Claude Code: the project dir does not even exist.
        assert_eq!(
            resolve_target(None, Some(&home), Some(&cwd)).unwrap_err(),
            NoTarget::NoSessions(sumcp_core::locate::project_dir(&home, &cwd))
        );
    }

    #[test]
    fn recency_without_a_home_has_nowhere_to_look() {
        let td = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_target(None, None, Some(td.path())).unwrap_err(),
            NoTarget::NowhereToLook
        );
    }

    #[test]
    fn claude_home_prefers_the_test_override_then_falls_back_to_home() {
        assert_eq!(
            claude_home_from(Some("/tmp/fixture".into()), Some("/Users/dev".into())),
            Some(PathBuf::from("/tmp/fixture"))
        );
        assert_eq!(
            claude_home_from(None, Some("/Users/dev".into())),
            Some(PathBuf::from("/Users/dev/.claude"))
        );
        assert_eq!(claude_home_from(None, None), None);
    }
}
