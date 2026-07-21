//! End-to-end: drive the real `sumcp` binary's `install`/`uninstall` against a
//! throwaway `$HOME`. This exercises the one path the in-crate unit tests can't:
//! resolving the running binary via `current_exe` and copying its real sibling
//! `sumcp-mcp` into place.

use std::fs;
use std::path::Path;
use std::process::Command;

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
    assert!(sumcp_dir.join("bin/sumcp").exists(), "sumcp binary missing");
    assert!(
        sumcp_dir.join("bin/sumcp-mcp").exists(),
        "mcp binary missing"
    );
    assert!(
        sumcp_dir.join("hooks/stop-nudge.sh").exists(),
        "hook missing"
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
