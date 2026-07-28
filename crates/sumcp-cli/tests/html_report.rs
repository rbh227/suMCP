//! End-to-end: the real binary renders self-contained HTML from the donor.
use std::process::Command;

fn donor() -> std::path::PathBuf {
    // The sanitized 2.1.210 donor fixture used across the suite.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join("session-2_1_210-subagents.jsonl")
}

#[test]
fn html_output_is_self_contained_and_structured() {
    let path = donor();
    assert!(path.exists(), "donor fixture missing at {}", path.display());
    let out = Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .args(["--file", path.to_str().unwrap(), "--html"])
        .output()
        .expect("run sumcp");
    assert!(out.status.success(), "sumcp --html exited non-zero");
    let html = String::from_utf8(out.stdout).expect("utf8 html");
    assert!(html.starts_with("<!DOCTYPE html>"), "not an html doc");
    assert!(html.contains("timeline"), "no timeline");
    // Hard zero-network invariant on real data. Robust escaped-attribute form:
    // won't false-fail if a sanitized evidence excerpt contains a literal URL.
    assert!(!html.contains("=\"http"), "external URL in an attribute");
    assert!(
        !html.contains("<script src") && !html.contains("<link") && !html.contains("<img"),
        "external-loading element leaked"
    );
    // No secret leaks in evidence excerpts (redaction wired).
    assert!(!html.contains("BEGIN RSA PRIVATE KEY"), "unredacted secret");
}

#[test]
fn work_unit_flag_merges_adjacent_transcripts() {
    let td = tempfile::tempdir().unwrap();
    let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
    let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
    let line = |sess: &str, ts: &str, path: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
        )
    };
    std::fs::write(
        td.path().join(format!("{id_a}.jsonl")),
        line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
    )
    .unwrap();
    let b = td.path().join(format!("{id_b}.jsonl"));
    std::fs::write(&b, line(id_b, "2026-01-01T00:05:00Z", "/y.rs")).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .arg("--work-unit")
        .arg(&b)
        .arg("--json")
        .output()
        .expect("run sumcp");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["work_unit"]["sessions"], 2);
    assert_eq!(v["totals"]["edits"], 2, "both transcripts' edits counted");
}

#[test]
fn file_flag_notes_that_the_transcript_is_part_of_a_unit() {
    let td = tempfile::tempdir().unwrap();
    let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
    let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
    let line = |sess: &str, ts: &str, path: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
        )
    };
    std::fs::write(
        td.path().join(format!("{id_a}.jsonl")),
        line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
    )
    .unwrap();
    let b = td.path().join(format!("{id_b}.jsonl"));
    std::fs::write(&b, line(id_b, "2026-01-01T00:05:00Z", "/y.rs")).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .arg("--file")
        .arg(&b)
        .arg("--json")
        .output()
        .expect("run sumcp");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("1 of 2 in a work unit"),
        "expected a stderr hint, got: {stderr}"
    );
    // The note must NOT contaminate stdout, which stays pipeable.
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(
        v["totals"]["edits"], 1,
        "--file still reports one transcript"
    );
}
