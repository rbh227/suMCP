//! `sumcp context` end to end against a fixture transcript.

use std::path::PathBuf;
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
