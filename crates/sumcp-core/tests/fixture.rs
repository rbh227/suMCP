//! Integration tests against the committed sanitized fixtures.
//!
//! These run the real ingest pipeline on real (sanitized) transcript data —
//! the parse gate for Checkpoint B. Integration tests live in `tests/` and see
//! the crate as an external user would (only its public API).

use std::path::PathBuf;
use sumcp_core::ingest::ingest_str;
use sumcp_core::model::Lane;

fn fixture(name: &str) -> String {
    // CARGO_MANIFEST_DIR is this crate's dir; the fixtures live at repo root.
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "fixtures", name]
        .iter()
        .collect();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn donor_fixture_parses_without_errors() {
    let s = ingest_str(&fixture("session-2_1_210-subagents.jsonl"), Lane::Main);
    assert_eq!(s.parse_errors, 0, "sanitized donor should be clean JSON");
    assert!(s.actions.len() > 100, "should extract many actions");
    // The donor is known to carry untimestamped events (amendment 5).
    assert!(s.untimestamped_lines > 0, "donor has untimestamped lines");
    // Unknown 2.1.2xx types are counted as data, not errors.
    assert!(s.type_counts.contains_key("queue-operation"));
    assert!(s.type_counts.contains_key("permission-mode"));
}

#[test]
fn ingest_is_deterministic() {
    let raw = fixture("session-2_1_210-subagents.jsonl");
    let a = ingest_str(&raw, Lane::Main);
    let b = ingest_str(&raw, Lane::Main);
    // Same input ⇒ identical Idx order and counts, run to run.
    let idxs_a: Vec<_> = a.actions.iter().map(|x| (x.idx, x.line_no)).collect();
    let idxs_b: Vec<_> = b.actions.iter().map(|x| (x.idx, x.line_no)).collect();
    assert_eq!(idxs_a, idxs_b);
}

#[test]
fn edge_cases_bad_line_is_counted_not_fatal() {
    let s = ingest_str(&fixture("edge-cases.jsonl"), Lane::Main);
    assert_eq!(s.parse_errors, 1, "the one non-JSON line is counted");
    assert!(!s.actions.is_empty(), "the good lines still parse");
}

#[test]
fn donor_reports_all_subagents_missing() {
    // The real 2.1.210 donor spawns subagents whose child transcript files
    // never made it into the sanitized fixture (T1.2: they were unrecoverable).
    // An *honest* assembly must therefore report every spawn as a missing
    // subagent file rather than silently pretending the work didn't happen.
    //
    // `load_session` is the filesystem-facing entry point: it reads the main
    // transcript, tries to discover+read subagent files, and records how many
    // it could not turn into analyzed actions. Here that count should equal the
    // number of spawns (12) — cross-checked by `grep -c '"agentId"'` on the
    // fixture, which finds 12 spawn results.
    let path: std::path::PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "fixtures",
        "session-2_1_210-subagents.jsonl",
    ]
    .iter()
    .collect();
    let a = sumcp_core::assemble::load_session(&path, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES)
        .unwrap();
    assert_eq!(
        a.session.subagent_files_missing, 12,
        "12 spawns, no child files on disk"
    );
}

#[test]
fn synthetic_2_1_x_merges_subagent_actions() {
    // The synthetic 2.1.x fixture tree DOES ship the child transcript, so the
    // merge should succeed end-to-end: two subagent `Edit` actions get folded
    // into the main session's timeline (interleaved by timestamp), and nothing
    // is left missing.
    //
    // Layout under fixtures/subagent-merge/:
    //   <uuid>.jsonl                       (main: one Agent spawn + one Edit)
    //   <uuid>/subagents/agent-helper.jsonl (two Edits, sessionId == parent)
    let path: std::path::PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "..",
        "..",
        "fixtures",
        "subagent-merge",
        "5717aaaa-1111-2222-3333-444455556666.jsonl",
    ]
    .iter()
    .collect();
    let a = sumcp_core::assemble::load_session(&path, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES)
        .unwrap();
    // Count actions whose lane is `Sub(_)` — i.e. came from a subagent file.
    // `matches!` is a compact "does this value fit that pattern?" test.
    let sub = a
        .session
        .actions
        .iter()
        .filter(|x| matches!(x.lane, Lane::Sub(_)))
        .count();
    assert_eq!(sub, 2, "both subagent edits merged");
    assert_eq!(a.session.subagent_files_missing, 0, "the one spawn resolved");
}

#[test]
fn edit_shape_signals_fire_on_the_real_donor() {
    use sumcp_core::signals::edit_shape;
    let s = ingest_str(&fixture("session-2_1_210-subagents.jsonl"), Lane::Main);
    let findings = edit_shape(&s);
    // A real 1682-line session should surface at least some struggle.
    assert!(
        !findings.is_empty(),
        "edit-shape signals should fire on a real working session"
    );
    // Every finding must carry evidence — the honesty invariant.
    assert!(findings.iter().all(|f| !f.idxs.is_empty()));
}
