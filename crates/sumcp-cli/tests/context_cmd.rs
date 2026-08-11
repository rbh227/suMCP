//! `sumcp context` end to end against a fixture transcript.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The binary under test, as cargo built it.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sumcp")
}

/// The sanitized demo fixture, addressed by `CARGO_MANIFEST_DIR` rather than
/// a plain relative path: cargo runs test binaries with the crate root as
/// the working directory, not the workspace root, and the fixture lives at
/// the workspace root (`html_report.rs`'s `donor()` does the same join).
fn demo_session() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join("demo/demo-session.jsonl")
}

#[test]
fn context_prints_a_v3_payload_for_an_explicit_file() {
    let path = demo_session();
    assert!(path.exists(), "fixture missing at {}", path.display());
    let out = Command::new(bin())
        .args(["context", "--file"])
        .arg(&path)
        .output()
        .expect("ran");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is valid JSON");
    assert_eq!(v["v"], 3);
    assert!(v["totals"].is_object());
}

#[test]
fn context_intent_prints_the_requests_payload() {
    let path = demo_session();
    let out = Command::new(bin())
        .args(["context", "--file"])
        .arg(&path)
        .arg("--intent")
        .output()
        .expect("ran");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["requests"].is_array());
}

#[test]
fn context_stdout_is_pure_json_even_when_a_note_would_otherwise_print() {
    // The demo fixture is a single transcript with no siblings, so this also
    // exercises the plain "no work_unit block" path; the point of the test
    // is that stdout parses as strict JSON on its own, with nothing but the
    // payload in it, regardless of whatever informational text went to
    // stderr.
    let path = demo_session();
    let out = Command::new(bin())
        .args(["context", "--file"])
        .arg(&path)
        .output()
        .expect("ran");
    assert!(out.status.success());
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout must be nothing but the JSON payload");
}

#[test]
fn context_rejects_an_unresolvable_range_instead_of_falling_back_to_the_whole_session() {
    // A range git cannot resolve must be a hard error, never a silent
    // full-session answer: answering about the wrong sessions is the exact
    // failure this feature exists to prevent.
    let path = demo_session();
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let out = Command::new(bin())
        .args(["context", "--file"])
        .arg(&path)
        .args(["--range", "no-such-rev..HEAD"])
        .current_dir(&repo_root)
        .output()
        .expect("ran");
    assert!(
        !out.status.success(),
        "an unresolvable range must fail, not silently succeed"
    );
    assert!(out.stdout.is_empty(), "no payload on the failing path");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no-such-rev"),
        "stderr should name the bad range: {stderr}"
    );
}

// --- --range as a scope guard (not a filter) ---
//
// These build a throwaway git repo with two commits at known, fixed times
// (mirroring `sumcp_core::git`'s own `fixture_repo` pattern) and a throwaway
// transcript with a controlled `effective_ts`, then run the real `sumcp
// context --range` binary against both. `HEAD~1..HEAD` resolves to the
// half-open window (COMMIT_1_TIME, COMMIT_2_TIME]: it opens when the
// PREVIOUS commit (COMMIT_1) landed and closes when the range's own commit
// (COMMIT_2) landed. See `git::range_window`'s doc comment.

const COMMIT_1_TIME: &str = "2026-01-01T10:00:00Z";
const COMMIT_2_TIME: &str = "2026-01-02T15:30:00Z";

/// A git repo with exactly two commits, at `COMMIT_1_TIME` and
/// `COMMIT_2_TIME`, so `HEAD~1..HEAD` always resolves to the same window.
fn range_guard_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    let run = |args: &[&str], env: &[(&str, &str)]| {
        let mut c = Command::new("git");
        c.args(args).current_dir(p);
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        assert!(out.status.success(), "git {args:?} failed: {out:?}");
    };
    run(&["init", "-q"], &[]);
    run(&["config", "user.email", "t@example.com"], &[]);
    run(&["config", "user.name", "t"], &[]);
    std::fs::write(p.join("a.txt"), "one").unwrap();
    run(&["add", "."], &[]);
    run(
        &["commit", "-q", "-m", "first"],
        &[
            ("GIT_AUTHOR_DATE", COMMIT_1_TIME),
            ("GIT_COMMITTER_DATE", COMMIT_1_TIME),
        ],
    );
    std::fs::write(p.join("a.txt"), "two").unwrap();
    run(&["add", "."], &[]);
    run(
        &["commit", "-q", "-m", "second"],
        &[
            ("GIT_AUTHOR_DATE", COMMIT_2_TIME),
            ("GIT_COMMITTER_DATE", COMMIT_2_TIME),
        ],
    );
    dir
}

/// A minimal single-action transcript whose only action's `effective_ts` is
/// `ts`, so the session's whole epoch span collapses to that one instant.
/// One `assistant`/`tool_use` line is enough for `assemble::load_session` to
/// produce a session with a non-empty `actions` list.
fn write_single_action_transcript(dir: &Path, ts: &str) -> PathBuf {
    let path = dir.join("5717aaaa-1111-2222-3333-444455556666.jsonl");
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"tu1","name":"Bash","input":{{"command":"echo hi"}}}}]}}}}"#
        ) + "\n",
    )
    .unwrap();
    path
}

/// Run `sumcp context --file <transcript> --range HEAD~1..HEAD` inside
/// `repo`, returning the process output.
fn run_context_range(repo: &Path, transcript: &Path) -> std::process::Output {
    Command::new(bin())
        .args(["context", "--file"])
        .arg(transcript)
        .args(["--range", "HEAD~1..HEAD"])
        .current_dir(repo)
        .output()
        .expect("ran")
}

#[test]
fn a_session_entirely_before_the_range_window_is_rejected() {
    let repo = range_guard_repo();
    // Strictly before COMMIT_1_TIME (the window's open start).
    let transcript = write_single_action_transcript(repo.path(), "2025-12-31T00:00:00Z");
    let out = run_context_range(repo.path(), &transcript);
    assert!(
        !out.status.success(),
        "a session entirely before the window must fail, not silently succeed"
    );
    assert!(out.stdout.is_empty(), "no payload on the failing path");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not overlap"),
        "stderr should explain the mismatch: {stderr}"
    );
}

#[test]
fn a_session_entirely_after_the_range_window_is_rejected() {
    let repo = range_guard_repo();
    // Strictly after COMMIT_2_TIME (the window's closed end).
    let transcript = write_single_action_transcript(repo.path(), "2026-01-03T00:00:00Z");
    let out = run_context_range(repo.path(), &transcript);
    assert!(
        !out.status.success(),
        "a session entirely after the window must fail, not silently succeed"
    );
    assert!(out.stdout.is_empty(), "no payload on the failing path");
}

#[test]
fn a_session_overlapping_the_range_window_proceeds_and_reports_the_overlap() {
    let repo = range_guard_repo();
    // Strictly between COMMIT_1_TIME and COMMIT_2_TIME.
    let transcript = write_single_action_transcript(repo.path(), "2026-01-02T00:00:00Z");
    let out = run_context_range(repo.path(), &transcript);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is valid JSON");
    assert_eq!(v["v"], 3);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("overlaps"),
        "stderr should report the overlap: {stderr}"
    );
    assert!(
        stderr.contains("WHOLE session"),
        "stderr must say plainly that the payload is NOT filtered to the \
         overlapping part, only confirmed to overlap it: {stderr}"
    );
}

#[test]
fn a_session_touching_only_the_windows_open_start_instant_does_not_overlap() {
    // The window is (COMMIT_1_TIME, COMMIT_2_TIME]: open at the start. A
    // session whose entire span is exactly COMMIT_1_TIME was not itself
    // happening after the window opened, so this must be rejected exactly
    // like the "entirely before" case above.
    let repo = range_guard_repo();
    let transcript = write_single_action_transcript(repo.path(), COMMIT_1_TIME);
    let out = run_context_range(repo.path(), &transcript);
    assert!(
        !out.status.success(),
        "touching only the window's open start instant must not count as overlap"
    );
    assert!(out.stdout.is_empty(), "no payload on the failing path");
}

#[test]
fn a_session_touching_only_the_windows_closed_end_instant_does_overlap() {
    // The window is (COMMIT_1_TIME, COMMIT_2_TIME]: closed at the end. A
    // session whose entire span is exactly COMMIT_2_TIME DOES overlap.
    let repo = range_guard_repo();
    let transcript = write_single_action_transcript(repo.path(), COMMIT_2_TIME);
    let out = run_context_range(repo.path(), &transcript);
    assert!(
        out.status.success(),
        "touching the window's closed end instant must count as overlap; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .expect("stdout must be nothing but the JSON payload");
}
