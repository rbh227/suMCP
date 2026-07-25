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
use sumcp_core::model::{Action, ActionKind, Idx};
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

    // Last Edit/Write action idx per file (edit_counts iteration order is
    // BTreeMap/path order, not time order, so this is a separate pass).
    let mut last_edit_idx: BTreeMap<&str, Idx> = BTreeMap::new();
    for a in &session.actions {
        if matches!(a.kind, ActionKind::Edit | ActionKind::Write)
            && let Some(f) = a.file_path.as_deref()
        {
            last_edit_idx.insert(f, a.idx);
        }
    }

    let files: Vec<_> = edit_counts
        .iter()
        .map(|(file, edits)| {
            let kinds: Vec<&String> = kinds_by_file
                .get(file)
                .map(|s| s.iter().collect())
                .unwrap_or_default();
            let verified = last_edit_idx
                .get(file)
                .map(|&idx| last_edit_verified(&session.actions, idx, file))
                .unwrap_or(false);
            json!({
                "file": file,
                "edits": edits,
                "kinds": kinds,
                "flagged_nr": nr_files.contains(file),
                "flagged_top3": top3_files.contains(file),
                "last_edit_verified": verified,
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

/// Study heuristic, NOT a product signal: `scripts/validity_sweep.py`'s
/// unverified-ending hypothesis (T5.4-followup, 2026-07-22) needs a per-file
/// proxy for "did anything in this session look back at this edit before it
/// ended." True when, strictly after `last_idx` (the file's last Edit/Write
/// action), the session contains any of:
///   (a) a CONFIRMED-successful Read of the same file,
///   (b) a CONFIRMED-successful Bash action whose command string contains the
///       file's basename,
///   (c) a CONFIRMED-successful Bash action whose command matches a common
///       test/build runner (cargo test, cargo build, pytest, npm test,
///       npm run build, go test, make).
///
/// "Confirmed-successful" means `is_error == Some(false)`. The two other
/// states are both rejected, and neither is evidence of a look-back:
/// `Some(true)` is an outright failure (a Read that errored never showed
/// anyone the file), and `None` means the action has no tool result at all --
/// a truncated or mid-flight session, where the outcome is unknown rather
/// than successful. Counting either would move files into the verified
/// stratum on no evidence, and this study's contingency cells are small
/// enough that such misclassification can change a reported conclusion.
///
/// This is intentionally cheap and textual (substring matching on the Bash
/// command), not a real test-coverage or CI signal -- it exists only to
/// stratify the predictive-validity study, and must not be read as "this
/// file was actually verified."
fn last_edit_verified(actions: &[Action], last_idx: Idx, file: &str) -> bool {
    const RUNNERS: [&str; 7] = [
        "cargo test",
        "cargo build",
        "pytest",
        "npm test",
        "npm run build",
        "go test",
        "make",
    ];
    let basename = file.rsplit('/').next().unwrap_or(file);
    actions.iter().filter(|a| a.idx > last_idx).any(|a| {
        // Gate BOTH kinds on a confirmed-successful result before looking
        // at anything else. Applying this once here (rather than inside
        // the Bash arm only) is what keeps a failed Read and a
        // result-less Bash from counting as verification.
        if a.is_error != Some(false) {
            return false;
        }
        match &a.kind {
            ActionKind::Read => a.file_path.as_deref() == Some(file),
            ActionKind::Bash => {
                let Some(cmd) = a.command.as_deref() else {
                    return false;
                };
                cmd.contains(basename) || RUNNERS.iter().any(|r| cmd.contains(r))
            }
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sumcp_core::ingest::ingest_str;
    use sumcp_core::model::Lane;

    /// Build a session from lines and run `last_edit_verified` for `file`,
    /// treating the file's LAST Edit/Write as the anchor (what the real dump
    /// loop does). Returns false when the file was never edited.
    fn verified(raw: &str, file: &str) -> bool {
        let s = ingest_str(raw, Lane::Main);
        let last = s
            .actions
            .iter()
            .filter(|a| {
                matches!(a.kind, ActionKind::Edit | ActionKind::Write)
                    && a.file_path.as_deref() == Some(file)
            })
            .map(|a| a.idx)
            .next_back();
        match last {
            Some(idx) => last_edit_verified(&s.actions, idx, file),
            None => false,
        }
    }

    /// An Edit of `/a.rs` at t=00, always with a successful result.
    const EDIT: &str = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","new_string":"x"}}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":false}]}}"#,
    );

    #[test]
    fn successful_read_of_the_file_verifies() {
        let raw = format!(
            "{EDIT}\n{}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"r1","is_error":false}]}}"#,
        );
        assert!(verified(&raw, "/a.rs"), "a real look-back still counts");
    }

    #[test]
    fn failed_read_does_not_verify() {
        // WHY: a Read that errored showed nobody the file. Before the fix the
        // Read arm never consulted `is_error`, so this counted as
        // verification and moved the file into the verified stratum.
        let raw = format!(
            "{EDIT}\n{}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"r1","is_error":true}]}}"#,
        );
        assert!(!verified(&raw, "/a.rs"), "an errored Read is not evidence");
    }

    #[test]
    fn read_with_no_tool_result_does_not_verify() {
        // WHY: `is_error` is None when the action has no result at all — a
        // truncated or mid-flight session. Unknown is not success.
        let raw = format!(
            "{EDIT}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
        );
        assert!(!verified(&raw, "/a.rs"), "unknown outcome is not evidence");
    }

    #[test]
    fn bash_with_no_tool_result_does_not_verify() {
        // WHY: the old guard rejected only `Some(true)`, so a result-less
        // Bash (`None`) fell through and counted as a passing test run.
        let raw = format!(
            "{EDIT}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        );
        assert!(
            !verified(&raw, "/a.rs"),
            "a test run with no result never reported passing"
        );
    }

    #[test]
    fn failed_bash_does_not_verify() {
        let raw = format!(
            "{EDIT}\n{}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#,
        );
        assert!(
            !verified(&raw, "/a.rs"),
            "a failing test run is not verification"
        );
    }

    #[test]
    fn successful_runner_bash_verifies_and_precedence_holds() {
        // A passing `cargo test` after the edit counts even though the command
        // never names the file (the RUNNERS branch).
        let raw = format!(
            "{EDIT}\n{}\n{}",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":false}]}}"#,
        );
        assert!(verified(&raw, "/a.rs"));
    }

    #[test]
    fn look_back_before_the_last_edit_does_not_count() {
        // The Read precedes the edit, so nothing looked back AFTER it.
        let raw = format!(
            "{}\n{}\n{EDIT}",
            r#"{"type":"assistant","timestamp":"2025-12-31T23:59:58Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/a.rs"}}]}}"#,
            r#"{"type":"user","timestamp":"2025-12-31T23:59:59Z","message":{"content":[{"type":"tool_result","tool_use_id":"r1","is_error":false}]}}"#,
        );
        assert!(
            !verified(&raw, "/a.rs"),
            "only look-backs AFTER the edit count"
        );
    }
}
