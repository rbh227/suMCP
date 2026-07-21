//! Failure signals (T3.2): failure loops with a four-step attribution chain,
//! tool error rates, and validation share.
//!
//! Attribution never touches the filesystem — it only matches paths that
//! already appear among the session's touched files (ADR A9), so a failure
//! can never be pinned to a file the agent never opened, and no path in a
//! command can escape to read the real disk.

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};
use std::collections::{BTreeMap, BTreeSet};

/// A file needs this many attributed failures to count as a failure *loop*.
const LOOP_MIN_FAILS: usize = 2;
/// How far back the proximity fallback looks for the last edited file.
const PROXIMITY_WINDOW: usize = 5;

/// Confidence with which a failing command was tied to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attribution {
    /// A touched file's path appeared in the command or its error output.
    PathMatch,
    /// No path evidence; blamed the most recently edited file nearby.
    Proximity,
}

/// Run the failure signals that produce findings.
pub fn failures(s: &Session) -> Vec<Finding> {
    failure_loops(s)
}

/// Failure loops: files that accumulate `LOOP_MIN_FAILS`+ failing Bash
/// commands, attributed via the four-step chain.
fn failure_loops(s: &Session) -> Vec<Finding> {
    let touched = touched_files(s);
    // file -> (failing bash idxs, weakest attribution seen)
    let mut by_file: BTreeMap<String, (Vec<Idx>, Attribution)> = BTreeMap::new();

    for (pos, a) in s.actions.iter().enumerate() {
        if !is_failed_bash(a) {
            continue;
        }
        if let Some((file, attr)) = attribute(s, pos, a, &touched) {
            let entry = by_file
                .entry(file)
                .or_insert((Vec::new(), Attribution::PathMatch));
            entry.0.push(a.idx);
            // keep the weakest attribution (proximity downgrades the finding)
            if attr == Attribution::Proximity {
                entry.1 = Attribution::Proximity;
            }
        }
        // step 4 (unattributed) is intentionally dropped from file-level
        // findings — we never pin a failure to a file without evidence.
    }

    by_file
        .into_iter()
        .filter(|(_, (idxs, _))| idxs.len() >= LOOP_MIN_FAILS)
        .map(|(file, (idxs, attr))| {
            let (confidence, note) = match attr {
                Attribution::PathMatch => (
                    Confidence::High,
                    "failing commands name this file in their output",
                ),
                Attribution::Proximity => (
                    Confidence::Low,
                    "attributed by proximity to the last edit (no path evidence)",
                ),
            };
            Finding {
                kind: FindingKind::FailureLoop,
                nums: Default::default(),
                tier: Tier::T2,
                exact: false, // attribution is always heuristic
                confidence,
                note: Some(format!("{} failing commands; {note}", idxs.len())),
                idxs,
                file: Some(file),
            }
        })
        .collect()
}

/// The four-step chain: (1) path in stderr/output, (2) path in command,
/// (3) last edit within the window, else (4) unattributed (`None`).
fn attribute(
    s: &Session,
    pos: usize,
    a: &Action,
    touched: &BTreeSet<String>,
) -> Option<(String, Attribution)> {
    // Steps 1+2: does any touched file's path appear in the command or error?
    let haystack = format!(
        "{} {}",
        a.command.as_deref().unwrap_or(""),
        a.error.as_deref().unwrap_or("")
    );
    // Prefer the longest matching path (most specific) for determinism.
    if let Some(file) = touched
        .iter()
        .filter(|f| haystack.contains(f.as_str()) || haystack.contains(basename(f)))
        .max_by_key(|f| f.len())
    {
        return Some((file.clone(), Attribution::PathMatch));
    }

    // Step 3: the most recent Edit/Write within PROXIMITY_WINDOW actions back,
    // IN THE SAME LANE (spec §5a). A failure in one lane is only ever caused by
    // an edit in that same lane; cross-lane adjacency in the merged order is
    // coincidental. Trade-off: heavy interleaving narrows the effective window.
    let start = pos.saturating_sub(PROXIMITY_WINDOW);
    if let Some(prev) = s.actions[start..pos].iter().rev().find(|p| {
        p.lane == a.lane
            && matches!(p.kind, ActionKind::Edit | ActionKind::Write)
            && p.file_path.is_some()
    }) {
        return Some((prev.file_path.clone().unwrap(), Attribution::Proximity));
    }

    None // step 4: unattributed
}

/// Distinct files the agent actually touched (read/edited/wrote). Attribution
/// is only ever allowed to pick from this set (no filesystem access).
fn touched_files(s: &Session) -> BTreeSet<String> {
    s.actions
        .iter()
        .filter_map(|a| a.file_path.clone())
        .collect()
}

fn is_failed_bash(a: &Action) -> bool {
    matches!(a.kind, ActionKind::Bash) && a.is_error == Some(true)
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Failing-over-total call count per tool (a session-level stat, not a finding).
pub fn tool_error_rates(s: &Session) -> BTreeMap<String, (u64, u64)> {
    let mut rates: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for a in &s.actions {
        let name = tool_name(&a.kind);
        let e = rates.entry(name).or_insert((0, 0));
        e.1 += 1;
        if a.is_error == Some(true) {
            e.0 += 1;
        }
    }
    rates
}

/// Share of Bash commands that look like validation (test/lint/build/typecheck).
pub fn validation_share(s: &Session) -> f64 {
    let bash: Vec<&Action> = s
        .actions
        .iter()
        .filter(|a| matches!(a.kind, ActionKind::Bash))
        .collect();
    if bash.is_empty() {
        return 0.0;
    }
    let hits = bash
        .iter()
        .filter(|a| is_validation(a.command.as_deref().unwrap_or("")))
        .count();
    hits as f64 / bash.len() as f64
}

fn is_validation(cmd: &str) -> bool {
    const NEEDLES: [&str; 8] = [
        "test",
        "lint",
        "build",
        "tsc",
        "typecheck",
        "cargo check",
        "pytest",
        "clippy",
    ];
    let c = cmd.to_lowercase();
    NEEDLES.iter().any(|n| c.contains(n))
}

fn tool_name(k: &ActionKind) -> String {
    match k {
        ActionKind::Read => "Read".into(),
        ActionKind::Edit => "Edit".into(),
        ActionKind::Write => "Write".into(),
        ActionKind::Bash => "Bash".into(),
        ActionKind::Other(n) => n.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    fn edit(id: &str, ts: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file}","new_string":"x"}}}}]}}}}"#
        )
    }
    // A failing Bash: tool_use then an errored tool_result carrying stderr.
    fn failed_bash(id: &str, ts: &str, cmd: &str, stderr: &str) -> String {
        let call = format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"{cmd}"}}}}]}}}}"#
        );
        let result = format!(
            r#"{{"type":"user","timestamp":"{ts}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":true,"content":"Exit code 1"}}]}},"toolUseResult":{{"stderr":"{stderr}"}}}}"#
        );
        format!("{call}\n{result}")
    }

    #[test]
    fn path_match_attribution_is_high_confidence() {
        let raw = format!(
            "{}\n{}\n{}",
            edit("e", "2026-01-01T00:00:00Z", "/src/DataStore.ts"),
            failed_bash(
                "b1",
                "2026-01-01T00:00:01Z",
                "npm test",
                "TypeError at /src/DataStore.ts:10"
            ),
            failed_bash(
                "b2",
                "2026-01-01T00:00:02Z",
                "npm test",
                "TypeError at /src/DataStore.ts:11"
            ),
        );
        let f = failures(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].file.as_deref(), Some("/src/DataStore.ts"));
        assert_eq!(f[0].confidence, Confidence::High);
        assert_eq!(f[0].idxs.len(), 2);
    }

    #[test]
    fn proximity_attribution_is_low_confidence() {
        // Two failing commands with no path in output, right after editing a file.
        let raw = format!(
            "{}\n{}\n{}",
            edit("e", "2026-01-01T00:00:00Z", "/src/a.ts"),
            failed_bash("b1", "2026-01-01T00:00:01Z", "make", "boom"),
            failed_bash("b2", "2026-01-01T00:00:02Z", "make", "boom again"),
        );
        let f = failures(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].confidence, Confidence::Low);
        assert_eq!(f[0].file.as_deref(), Some("/src/a.ts"));
    }

    #[test]
    fn single_failure_is_not_a_loop() {
        let raw = format!(
            "{}\n{}",
            edit("e", "2026-01-01T00:00:00Z", "/src/a.ts"),
            failed_bash("b1", "2026-01-01T00:00:01Z", "make", "boom"),
        );
        assert!(
            failures(&ingest_str(&raw, Lane::Main)).is_empty(),
            "one fail is not a loop"
        );
    }

    #[test]
    fn proximity_does_not_attribute_across_lanes() {
        // A main-lane failing Bash whose nearest prior Edit is in a SUB lane
        // must not be attributed to it; a same-lane prior edit is required.
        use crate::model::{Action, ActionKind, Idx, Lane, Session};
        let edit = |idx, lane, file: &str| Action {
            idx: Idx(idx),
            effective_ts: "2026-01-01T00:00:01Z".into(),
            ts_inherited: false,
            lane,
            line_no: idx as usize,
            kind: ActionKind::Edit,
            file_path: Some(file.into()),
            is_error: None,
            write_len: None,
            write_lines: None,
            read_total_lines: None,
            input_hash: None,
            error: None,
            hunks: vec![],
            command: None,
            user_modified: false,
            edit_old: None,
            edit_new: None,
            approval_latency_s: None,
        };
        let mut bash = edit(1, Lane::Main, "/x");
        bash.kind = ActionKind::Bash;
        bash.command = Some("run".into());
        bash.is_error = Some(true);
        bash.file_path = None;
        // A second identical failing Bash: two attributed failures are needed
        // to form a FailureLoop (LOOP_MIN_FAILS). Without both, no finding is
        // emitted and the assertion below would pass vacuously — this makes the
        // cross-lane misattribution actually surface as a finding to guard.
        let mut bash2 = bash.clone();
        bash2.idx = Idx(2);
        bash2.line_no = 2;

        // Only prior edit is in a sub lane → must fall through to unattributed.
        let s = Session {
            actions: vec![edit(0, Lane::Sub("z".into()), "/sub-file"), bash, bash2],
            user_texts: vec![],
            tokens: Default::default(),
            type_counts: Default::default(),
            parse_errors: 0,
            untimestamped_lines: 0,
            interrupts: 0,
            auto_accept: false,
            spawns: vec![],
            subagent_files_missing: 0,
        };
        let f = failures(&s);
        // No path match, no same-lane prior edit → the failure is unattributed,
        // i.e. carries no file.
        assert!(
            f.iter().all(|x| x.file.is_none()),
            "must not attribute to sub-lane edit"
        );
    }

    #[test]
    fn error_rate_and_validation_share() {
        let raw = format!(
            "{}\n{}",
            failed_bash("b1", "2026-01-01T00:00:01Z", "npm test", "boom"),
            // a passing validation command
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"npm run lint"}}]}}"#,
        );
        let s = ingest_str(&raw, Lane::Main);
        let rates = tool_error_rates(&s);
        assert_eq!(rates.get("Bash"), Some(&(1, 2)), "1 of 2 Bash calls failed");
        assert_eq!(validation_share(&s), 1.0, "both Bash calls are validation");
    }
}
