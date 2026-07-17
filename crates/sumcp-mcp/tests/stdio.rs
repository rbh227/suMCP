//! End-to-end stdio smoke test (T4.1 acceptance).
//!
//! Spawns the real `sumcp-mcp` binary (cargo builds it for us and exposes the
//! path as `CARGO_BIN_EXE_sumcp-mcp`), speaks line-delimited JSON-RPC over its
//! stdin/stdout — exactly what Claude Code does — and checks the frozen v0
//! contract on real fixture data:
//!
//! - handshake works, six tools listed, every one `readOnlyHint: true`;
//! - every tool answers under its token cap with `v`, provenance, `truncated`;
//! - `evidence(idxs)` dereferences a Finding taken from `struggle_areas`;
//! - self-identification verifies a forwarded tool_use id (ADR A4);
//! - no verifiable caller and no `session_id` → `ambiguous_session`, never
//!   a guess.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const SESSION_ID: &str = "5717aaaa-1111-2222-3333-444455556666";
/// chars-per-token estimate — the same headroom rule as `check_payloads.py`.
const CHARS_PER_TOKEN: f64 = 3.5;

/// A tiny JSON-RPC client around the spawned server.
struct Rpc {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl Rpc {
    /// Spawn the server with `SUMCP_CLAUDE_HOME` pointed at `claude_home`
    /// and cwd set to `project_cwd` (how Claude Code launches it).
    fn spawn(claude_home: &Path, project_cwd: &Path) -> Rpc {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sumcp-mcp"))
            .env("SUMCP_CLAUDE_HOME", claude_home)
            .current_dir(project_cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("server should spawn");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Rpc {
            child,
            stdin,
            stdout,
            next_id: 0,
        }
    }

    /// Send a request, read lines until its response arrives.
    fn call(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        self.next_id += 1;
        let id = self.next_id;
        let req =
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{req}").unwrap();
        self.stdin.flush().unwrap();
        // Skip anything that isn't our response (server-initiated messages).
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line).unwrap() == 0 {
                panic!("server closed stdout before answering {method}");
            }
            let v: serde_json::Value = serde_json::from_str(&line).expect("valid JSON line");
            if v["id"] == serde_json::json!(id) {
                assert!(
                    v.get("error").is_none(),
                    "{method} returned protocol error: {}",
                    v["error"]
                );
                return v["result"].clone();
            }
        }
    }

    fn notify(&mut self, method: &str) {
        let n = serde_json::json!({"jsonrpc": "2.0", "method": method});
        writeln!(self.stdin, "{n}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// MCP handshake: initialize + initialized notification.
    fn handshake(&mut self) -> serde_json::Value {
        let init = self.call(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "smoke-test", "version": "0"}
            }),
        );
        self.notify("notifications/initialized");
        init
    }

    /// Call one of our six tools; returns the payload parsed from the text
    /// content, plus the isError flag.
    fn tool(&mut self, name: &str, args: serde_json::Value) -> (serde_json::Value, bool) {
        self.tool_with_meta(name, args, None)
    }

    fn tool_with_meta(
        &mut self,
        name: &str,
        args: serde_json::Value,
        meta: Option<serde_json::Value>,
    ) -> (serde_json::Value, bool) {
        let mut params = serde_json::json!({"name": name, "arguments": args});
        if let Some(m) = meta {
            params["_meta"] = m;
        }
        let result = self.call("tools/call", params);
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name} returned no text content: {result}"));
        let payload: serde_json::Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("{name} payload is not JSON ({e}): {text}"));
        (payload, result["isError"] == serde_json::json!(true))
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Build a fake `~/.claude` containing the real donor fixture as one session
/// of a fake project, and return (claude_home, project_cwd).
fn fixture_home(root: &Path) -> (PathBuf, PathBuf) {
    let project_cwd = root.join("proj");
    std::fs::create_dir_all(&project_cwd).unwrap();
    // macOS tempdirs live behind a /var → /private/var symlink; the server
    // sees the canonical form via current_dir(), so derive names from it too.
    let project_cwd = project_cwd.canonicalize().unwrap();
    let claude_home = root.join("claude-home");
    let project_dir = sumcp_core::locate::project_dir(&claude_home, &project_cwd);
    std::fs::create_dir_all(&project_dir).unwrap();
    // CARGO_MANIFEST_DIR = crates/sumcp-mcp → repo root is two levels up.
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/session-2_1_210-subagents.jsonl");
    std::fs::copy(&fixture, project_dir.join(format!("{SESSION_ID}.jsonl"))).unwrap();
    (claude_home, project_cwd)
}

fn cap_ok(name: &str, payload: &serde_json::Value, cap: usize) {
    let tokens = (payload.to_string().len() as f64 / CHARS_PER_TOKEN).ceil() as usize;
    assert!(tokens <= cap, "{name} over cap: ~{tokens} > {cap}");
}

#[test]
fn six_tools_answer_the_frozen_contract_over_stdio() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, cwd) = fixture_home(tmp.path());
    let mut rpc = Rpc::spawn(&home, &cwd);

    let init = rpc.handshake();
    assert_eq!(init["serverInfo"]["name"], "sumcp");

    // --- tools/list: six tools, all read-only ---
    let listed = rpc.call("tools/list", serde_json::json!({}));
    let tools = listed["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 6);
    for t in tools {
        assert_eq!(
            t["annotations"]["readOnlyHint"], true,
            "{} must advertise readOnlyHint",
            t["name"]
        );
    }

    // --- every tool answers under its cap, with provenance ---
    let sid = serde_json::json!({"session_id": SESSION_ID});
    let (overview, err) = rpc.tool("session_overview", sid.clone());
    assert!(!err);
    assert_eq!(overview["v"], 0);
    assert_eq!(overview["session"]["identified_by"], "explicit");
    assert!(overview["truncated"].is_boolean());
    cap_ok("session_overview", &overview, 1000);

    let (struggles, err) = rpc.tool(
        "struggle_areas",
        serde_json::json!({"n": 3, "session_id": SESSION_ID}),
    );
    assert!(!err);
    assert!(struggles["weights"]["source"].is_string(), "weights echoed");
    cap_ok("struggle_areas", &struggles, 1500);
    let top = &struggles["files"][0];
    let top_file = top["file"].as_str().expect("donor fixture has struggles");

    let (story, err) = rpc.tool(
        "file_story",
        serde_json::json!({"path": top_file, "session_id": SESSION_ID}),
    );
    assert!(!err);
    assert_eq!(story["file"], top_file);
    cap_ok("file_story", &story, 1500);

    let (blind, err) = rpc.tool("blind_spots", sid.clone());
    assert!(!err);
    assert!(blind["suppression"]["approval_latency"].is_string());
    cap_ok("blind_spots", &blind, 1000);

    let (health, err) = rpc.tool("context_health", sid.clone());
    assert!(!err);
    assert!(health["cache_hit_ratio"].is_number());
    cap_ok("context_health", &health, 1000);

    // --- evidence(idxs) dereferences a Finding from struggle_areas ---
    let idxs = top["findings"][0]["idxs"].clone();
    assert!(idxs.as_array().is_some_and(|a| !a.is_empty()));
    let (evidence, err) = rpc.tool(
        "evidence",
        serde_json::json!({"idxs": idxs, "session_id": SESSION_ID}),
    );
    assert!(!err);
    let actions = evidence["actions"].as_array().unwrap();
    assert!(!actions.is_empty(), "finding idxs must dereference");
    assert!(actions[0]["idx"].is_number());
    cap_ok("evidence", &evidence, 1500);
}

#[test]
fn forwarded_tool_use_id_verifies_the_calling_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, cwd) = fixture_home(tmp.path());
    // The calling session appends our tool_use before the call reaches us —
    // simulate that by planting the id in the transcript tail.
    let project_dir = sumcp_core::locate::project_dir(&home, &cwd);
    let transcript = project_dir.join(format!("{SESSION_ID}.jsonl"));
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&transcript)
        .unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","id":"toolu_SMOKE_9f3","name":"mcp__sumcp__session_overview","input":{{}}}}]}}}}"#
    )
    .unwrap();
    drop(f);

    let mut rpc = Rpc::spawn(&home, &cwd);
    rpc.handshake();
    let (payload, err) = rpc.tool_with_meta(
        "session_overview",
        serde_json::json!({}),
        Some(serde_json::json!({"claudecode/toolUseId": "toolu_SMOKE_9f3"})),
    );
    assert!(!err, "verified caller must not error: {payload}");
    assert_eq!(payload["session"]["id"], SESSION_ID);
    assert_eq!(payload["session"]["identified_by"], "tool_use_id");
}

#[test]
fn unverifiable_caller_fails_closed_with_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, cwd) = fixture_home(tmp.path());
    let mut rpc = Rpc::spawn(&home, &cwd);
    rpc.handshake();
    // No session_id, no forwarded id, and no fresh sumcp marker in any tail:
    // the server must refuse to guess (ADR A4), listing what it saw.
    let (payload, err) = rpc.tool("session_overview", serde_json::json!({}));
    assert!(err, "unverifiable caller must be a tool-level error");
    assert_eq!(payload["error"], "ambiguous_session");
    assert_eq!(payload["candidates"][0]["id"], SESSION_ID);
    assert!(payload["hint"].as_str().unwrap().contains("session_id"));
}
