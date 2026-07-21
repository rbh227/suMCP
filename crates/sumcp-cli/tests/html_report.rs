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
