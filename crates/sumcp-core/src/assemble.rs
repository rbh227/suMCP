//! Assembly: the filesystem-facing entry point that turns a main transcript
//! path into a fully merged `Session` (main + subagents). Both binaries call
//! `load_session`; it is the only place that reads subagent files.

use crate::ingest::ingest_str;
use crate::locate::{discover_subagent_paths, is_within_or_root};
use crate::merge::merge_sessions;
use crate::model::{Lane, Session};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Hard ceiling on any single transcript we read (ADR A9(3)).
pub const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;

/// The outcome of assembly: the merged session plus the subagent files that
/// were actually read (the MCP store keys cache freshness on these).
pub struct Assembled {
    /// The fully merged session (main + subagents).
    pub session: Session,
    /// Absolute paths of the subagent transcripts that were read and merged.
    pub subagent_paths: Vec<PathBuf>,
}

/// Read a file up to `max_bytes`, rejecting non-regular files (ADR A9(3)).
/// Returns `Ok(None)` on any I/O problem — callers treat that as "missing".
fn read_bounded(path: &Path, max_bytes: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let mut raw = String::new();
    // `.take(max_bytes + 1)` caps the read one byte past the ceiling so an
    // exactly-at-limit file still fits but an over-limit one trips the check
    // below (metadata len can lie for pipes/sparse files, so we re-check).
    std::fs::File::open(path)
        .ok()?
        .take(max_bytes + 1)
        .read_to_string(&mut raw)
        .ok()?;
    if raw.len() as u64 > max_bytes {
        return None;
    }
    Some(raw)
}

/// Extract the first `sessionId` value seen in a transcript, if any. Used to
/// validate a 2.1.x subagent file belongs to the expected parent session.
fn first_session_id(raw: &str) -> Option<String> {
    for line in raw.lines() {
        // A let-chain: both patterns must bind for the body to run. Reads as
        // "if this line parses as JSON AND it has a string sessionId".
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(id) = v.get("sessionId").and_then(|x| x.as_str())
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Turn a main transcript path into a fully merged `Session`. Reads the main
/// file, discovers and reads subagent files, validates ownership, merges. Only
/// a failure to read the MAIN file is an error; subagent failures are counted
/// in `subagent_files_missing`, never fatal.
pub fn load_session(main_path: &Path, max_bytes: u64) -> std::io::Result<Assembled> {
    let raw = read_bounded(main_path, max_bytes)
        .ok_or_else(|| std::io::Error::other("main transcript unreadable or over size ceiling"))?;
    let main = ingest_str(&raw, Lane::Main);

    // The expected parent session id: the main file's stem (its uuid).
    let expected_id = main_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let candidates = discover_subagent_paths(main_path, &main.spawns);
    // How many subagent transcripts we set out to analyze. In the legacy layout
    // discovery resolves one sibling per spawn, so `spawns.len()` is the natural
    // expectation. In the 2.1.x layout discovery lists the namespaced directory,
    // which can hold MORE files than the main transcript has spawns (e.g. a
    // rejected wrong-session file). Taking the max of the two makes every file
    // we looked at but could not merge count as missing, in either layout.
    let attempted = main.spawns.len().max(candidates.len());
    let is_2_1_x = crate::locate::subagents_dir(main_path).is_dir();

    let mut subs: Vec<Session> = Vec::new();
    let mut read_paths: Vec<PathBuf> = Vec::new();
    // Count of successfully merged, non-empty subagent transcripts.
    let mut merged_ok = 0usize;

    for path in candidates {
        if !is_within_or_root(main_path, &path) {
            continue; // safety guard (also applied in discovery; belt-and-suspenders)
        }
        // `let ... else` binds on Some, and on None runs the `else` block —
        // here we skip this candidate (an unreadable/oversized file is missing).
        let Some(sub_raw) = read_bounded(&path, max_bytes) else { continue };
        // 2.1.x ownership check: reject a file whose sessionId is present AND
        // mismatched. Absent sessionId is accepted (the namespaced directory is
        // the primary ownership guarantee; the field's exact shape is not yet
        // verified against a real 2.1.x session — see spec open item #1).
        if is_2_1_x
            && let Some(sid) = first_session_id(&sub_raw)
            && sid != expected_id
        {
            continue; // belongs to another session — count as missing below
        }
        // Agent id from filename `agent-<id>.jsonl` for the lane label.
        let agent_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("agent-"))
            .unwrap_or("unknown")
            .to_string();
        let sub = ingest_str(&sub_raw, Lane::Sub(agent_id));
        if sub.actions.is_empty() {
            continue; // resolved but nothing usable — counts as missing
        }
        merged_ok += 1;
        subs.push(sub);
        read_paths.push(path);
    }

    // files_missing: subagent units we could not turn into analyzed actions.
    // Floored at zero with `saturating_sub` (a version/streaming artifact could
    // surface more merges than attempted, and an unsigned underflow would wrap
    // to a huge number).
    let files_missing = attempted.saturating_sub(merged_ok) as u64;

    let session = merge_sessions(main, subs, files_missing);
    Ok(Assembled { session, subagent_paths: read_paths })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_spawn_lines(id: &str, agent_id: &str) -> String {
        // A main-transcript Agent spawn whose result carries an agentId.
        format!(
            "{}\n{}",
            format!(r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"subagent_type":"x"}}}}]}}}}"#),
            format!(r#"{{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}},"toolUseResult":{{"agentId":"{agent_id}"}}}}"#),
        )
    }

    /// A subagent transcript line: one Edit, carrying a parent sessionId.
    fn sub_edit_line(parent: &str) -> String {
        format!(r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","sessionId":"{parent}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"/sub.rs","old_string":"a","new_string":"b"}}}}]}}}}"#)
    }

    #[test]
    fn legacy_layout_merges_present_counts_missing() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        // two spawns: "present" has a sibling file, "absent" does not.
        std::fs::write(
            &main,
            format!("{}\n{}", agent_spawn_lines("a1", "present"), agent_spawn_lines("a2", "absent")),
        )
        .unwrap();
        std::fs::write(td.path().join("agent-present.jsonl"), sub_edit_line(uuid)).unwrap();
        // decoy sibling for a different session — not named by any spawn.
        std::fs::write(td.path().join("agent-decoy.jsonl"), sub_edit_line("other")).unwrap();

        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        // The present subagent's Edit merged in (a sub-lane action exists).
        assert!(a.session.actions.iter().any(|x| matches!(x.lane, Lane::Sub(_))));
        // One spawn ("absent") could not be resolved.
        assert_eq!(a.session.subagent_files_missing, 1);
        // The decoy was never read.
        assert!(a.subagent_paths.iter().all(|p| !p.ends_with("agent-decoy.jsonl")));
    }

    #[test]
    fn v2_1_x_rejects_wrong_session_id() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, agent_spawn_lines("a1", "present")).unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        // one file belongs to this session, one carries a wrong sessionId.
        std::fs::write(subs.join("agent-ok.jsonl"), sub_edit_line(uuid)).unwrap();
        std::fs::write(subs.join("agent-wrong.jsonl"), sub_edit_line("SOMEONE-ELSE")).unwrap();

        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        let sub_actions = a.session.actions.iter().filter(|x| matches!(x.lane, Lane::Sub(_))).count();
        assert_eq!(sub_actions, 1, "only the matching-sessionId file merges");
        assert_eq!(a.session.subagent_files_missing, 1, "wrong-sessionId file counts missing");
    }

    #[test]
    fn no_subagents_is_main_only() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("flat.jsonl");
        std::fs::write(
            &main,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","new_string":"x"}}]}}"#,
        )
        .unwrap();
        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        assert_eq!(a.session.subagent_files_missing, 0);
        assert!(a.subagent_paths.is_empty());
    }
}
