//! Per-session dump for the predictive-validity study (`scripts/validity_sweep.py`).
//!
//! Mirrors the CLI's real `--file` pipeline exactly (see `sumcp-cli/src/main.rs`):
//! `assemble::load_session` (main transcript + subagent merge) →
//! `score::rank` with `Weights::default()` → `review::needs_review`. Frozen
//! weights, no tuning: this binary only observes what the product already
//! computes.
//!
//! Usage: `validity_dump <transcript.jsonl>`: prints one JSON object to
//! stdout. Never panics on a weird transcript: ingest already tolerates parse
//! anomalies, and a zero-action session prints a valid object with an empty
//! `files` array.

use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use sumcp_core::assemble::{MAX_TRANSCRIPT_BYTES, load_session};
use sumcp_core::model::{Action, ActionKind};
use sumcp_core::review::needs_review;
use sumcp_core::score::{Weights, all_findings, rank};

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        eprintln!("usage: validity_dump <transcript.jsonl>");
        return std::process::ExitCode::FAILURE;
    };
    let path = PathBuf::from(arg);

    // Same assembly path the CLI's --file flow uses: reads the main
    // transcript, discovers and flat-merges any sibling subagent transcripts.
    let assembled = match load_session(&path, MAX_TRANSCRIPT_BYTES) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not load {}: {e}", path.display());
            return std::process::ExitCode::FAILURE;
        }
    };
    let session = assembled.session;

    let ranked = rank(&session, &Weights::default());
    let all = all_findings(&session);
    let review_candidates = needs_review(&ranked, &all);

    // Files flagged by each of the two definitions the study compares.
    let top3_files: BTreeSet<&str> = ranked.iter().take(3).map(|f| f.file.as_str()).collect();
    let nr_files: BTreeSet<&str> = review_candidates.iter().map(|c| c.file.as_str()).collect();

    // Every file with at least one Edit or Write action, in first-seen order
    // (BTreeMap keeps the output deterministic regardless of action order).
    let mut edit_counts: BTreeMap<&str, u64> = BTreeMap::new();
    for a in &session.actions {
        if matches!(a.kind, ActionKind::Edit | ActionKind::Write)
            && let Some(f) = a.file_path.as_deref()
        {
            *edit_counts.entry(f).or_insert(0) += 1;
        }
    }

    // File-scoped finding kinds, deduped, per file: serialized to the same
    // snake_case strings the payload contract uses (e.g. "churn",
    // "user_corrected"). Sorted for determinism.
    let mut kinds_by_file: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for f in &all {
        let Some(file) = f.file.as_deref() else {
            continue;
        };
        if !edit_counts.contains_key(file) {
            continue; // only files that were actually edited/written are reported
        }
        let kind_str = serde_json::to_value(&f.kind)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        kinds_by_file.entry(file).or_default().insert(kind_str);
    }

    let files: Vec<_> = edit_counts
        .iter()
        .map(|(file, edits)| {
            let kinds: Vec<&String> = kinds_by_file
                .get(file)
                .map(|s| s.iter().collect())
                .unwrap_or_default();
            json!({
                "file": file,
                "edits": edits,
                "kinds": kinds,
                "flagged_nr": nr_files.contains(file),
                "flagged_top3": top3_files.contains(file),
            })
        })
        .collect();

    let project = session
        .cwd
        .clone()
        .unwrap_or_else(|| parent_dir_name(&path));

    let start_ts = first_effective_ts(&session.actions);

    let out = json!({
        "path": path.display().to_string(),
        "project": project,
        "start_ts": start_ts,
        "actions": session.actions.len(),
        "files": files,
    });

    println!("{}", serde_json::to_string(&out).unwrap());
    std::process::ExitCode::SUCCESS
}

/// The transcript's parent directory name: the `project` fallback when the
/// session carries no `cwd`.
fn parent_dir_name(path: &std::path::Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The first action's `effective_ts`, or an empty string when there are no
/// actions at all (actions are already in total order: see `model.rs`).
fn first_effective_ts(actions: &[Action]) -> String {
    actions
        .first()
        .map(|a| a.effective_ts.clone())
        .unwrap_or_default()
}
