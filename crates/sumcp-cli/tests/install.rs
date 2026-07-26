//! End-to-end: drive the real `sumcp` binary's `install`/`uninstall` against a
//! throwaway `$HOME`. This exercises the one path the in-crate unit tests can't:
//! resolving the running binary via `current_exe` and copying its real sibling
//! `sumcp-mcp` into place.

use std::fs;
use std::path::Path;
use std::process::Command;
// Only the hook test uses these, and that test is Unix only because it spawns
// /bin/sh. Importing them unconditionally warns on Windows.
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::process::Stdio;

fn sumcp() -> &'static str {
    env!("CARGO_BIN_EXE_sumcp")
}

/// Run the binary with `HOME` pointed at `home`, returning (success, stdout).
fn run(home: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new(sumcp())
        .args(args)
        .env("HOME", home)
        .output()
        .expect("spawn sumcp");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn dry_run_writes_nothing_then_apply_and_uninstall_roundtrip() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    let sumcp_dir = home.path().join(".claude/sumcp");

    // 1. Dry-run must not create anything.
    let (ok, out) = run(home.path(), &["install"]);
    assert!(ok, "dry-run install failed: {out}");
    assert!(out.contains("dry-run"), "no dry-run banner: {out}");
    assert!(!sumcp_dir.exists(), "dry-run wrote to disk");

    // 2. Apply: the whole tree + registrations land.
    let (ok, out) = run(home.path(), &["install", "--apply"]);
    assert!(ok, "apply install failed: {out}");
    let exe = std::env::consts::EXE_SUFFIX;
    assert!(
        sumcp_dir.join(format!("bin/sumcp{exe}")).exists(),
        "sumcp binary missing"
    );
    assert!(
        sumcp_dir.join(format!("bin/sumcp-mcp{exe}")).exists(),
        "mcp binary missing"
    );
    // The hook is a /bin/sh script, installed on Unix and deliberately skipped
    // on Windows. Assert each platform's real contract, so "correctly skipped"
    // and "silently broken" cannot be confused.
    #[cfg(unix)]
    assert!(
        sumcp_dir.join("hooks/stop-nudge.sh").exists(),
        "hook missing"
    );
    #[cfg(not(unix))]
    assert!(
        !sumcp_dir.join("hooks/stop-nudge.sh").exists(),
        "there is no /bin/sh here, so the hook must not be written"
    );
    assert!(sumcp_dir.join("manifest.json").exists(), "manifest missing");
    assert!(
        home.path().join(".claude/skills/debrief/SKILL.md").exists(),
        "skill missing"
    );
    let cj: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.path().join(".claude.json")).unwrap())
            .unwrap();
    assert!(
        cj["mcpServers"]["sumcp"].is_object(),
        "server not registered"
    );

    // 3. Uninstall: our tree is gone.
    let (ok, out) = run(home.path(), &["uninstall", "--apply"]);
    assert!(ok, "uninstall failed: {out}");
    assert!(!sumcp_dir.exists(), "uninstall left the sumcp tree");
    assert!(
        !home.path().join(".claude/skills/debrief").exists(),
        "uninstall left the skill"
    );
}

/// The installed-system path the debrief contract depends on: Claude Code's
/// Stop event pipes JSON (with `session_id` + `transcript_path`) into the
/// installed hook, which must nudge WITH the session id — the skill passes it
/// explicitly to every tool call — and must not re-nudge on the next Stop
/// when nothing new happened (codex review 2026-07-22 P0).
// Spawns the hook with /bin/sh, which does not exist on Windows, and the hook
// is not installed there for exactly that reason. Unix only by nature, not by
// convenience: there is nothing on Windows for this test to drive.
#[cfg(unix)]
#[test]
fn installed_hook_nudges_with_session_id_and_does_not_spam() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join(".claude")).unwrap();
    let (ok, out) = run(home.path(), &["install", "--apply"]);
    assert!(ok, "install failed: {out}");
    let hook = home.path().join(".claude/sumcp/hooks/stop-nudge.sh");

    // A transcript with 4 edits — over the hook's threshold of 3.
    let transcript = home.path().join("transcript.jsonl");
    let lines: Vec<String> = (0..4)
        .map(|i| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.rs","old_string":"a","new_string":"b"}}}}]}}}}"#
            )
        })
        .collect();
    fs::write(&transcript, lines.join("\n")).unwrap();

    // What Claude Code actually pipes to a Stop hook (subset of fields).
    let stop_payload = format!(
        r#"{{"session_id":"sess-e2e-1234","transcript_path":"{}","hook_event_name":"Stop"}}"#,
        transcript.display()
    );
    // Isolated TMPDIR so the nudge marker never leaks between test runs.
    let tmp = tempfile::tempdir().unwrap();
    let run_hook = || {
        let mut child = Command::new("/bin/sh")
            .arg(&hook)
            .env("TMPDIR", tmp.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn hook");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stop_payload.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "hook must never fail the Stop event");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };

    // First Stop past the threshold: one nudge, carrying the session id.
    let first = run_hook();
    assert!(first.contains("systemMessage"), "no nudge emitted: {first}");
    assert!(
        first.contains("sess-e2e-1234"),
        "nudge must carry the session id the skill will pass on: {first}"
    );
    assert!(first.contains("4 edits"), "edit count missing: {first}");

    // Second Stop, no new edits: silent — Stop fires after every response,
    // and a repeated nudge is exactly the spam the review flagged.
    let second = run_hook();
    assert!(
        second.trim().is_empty(),
        "hook re-nudged with no new edits: {second}"
    );
}
