//! Integration test for CLI flag conflicts.

use std::process::Command;

fn backstory() -> &'static str {
    env!("CARGO_BIN_EXE_backstory")
}

/// Passing both `--file` and `--work-unit` must be rejected because they
/// conflict (the Args struct declares `conflicts_with = "file"`).
/// This test ensures that a future clap upgrade or refactor cannot silently
/// drop the conflict check.
#[test]
fn file_and_work_unit_flags_conflict() {
    let td = tempfile::tempdir().unwrap();
    let file = td.path().join("transcript.jsonl");
    let work_unit = td.path().join("other.jsonl");

    let out = Command::new(backstory())
        .arg("--file")
        .arg(&file)
        .arg("--work-unit")
        .arg(&work_unit)
        .output()
        .expect("spawn backstory");

    // The binary should fail, not succeed.
    assert!(
        !out.status.success(),
        "backstory should reject conflicting flags, but succeeded"
    );

    // stderr should mention that the flags cannot be used together.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.to_lowercase().contains("cannot be used with"),
        "stderr should explain the conflict: {stderr}"
    );
}
