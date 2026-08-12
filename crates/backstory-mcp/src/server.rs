//! The MCP server: six read-only tools over the backstory-core pipeline.
//!
//! This file is the thin rmcp wrapper (ADR A2): protocol types in, compact
//! JSON payloads out. All analysis lives in `backstory-core`; all identification
//! logic lives in `identify.rs`; caching lives in `store.rs`. Swapping the
//! SDK later should mean rewriting only this file and `main.rs`.

use backstory_core::model::Idx;
use backstory_core::payloads::{self, SessionMeta};
use backstory_core::score::rank;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::identify::{self, IdentifyError, Resolved};
use crate::store::SessionStore;

/// The server key this binary is registered under in `.mcp.json`.
pub const SERVER_KEY: &str = "backstory";

/// Bounded retry for transcript flush delay (ADR A4). Measured live
/// 2026-07-20 (Checkpoint D + /debug log): Claude Code usually flushes the
/// triggering tool_use line only AFTER our result returns — a write we
/// cannot outwait — so the scan is *opportunistic verification*, not
/// reliable discovery (1 hit in 4 live tries; the transcript mtime sat 8 s
/// stale through one full retry window). Two quick attempts catch the
/// lucky-flush case cheaply; the dependable path is an explicit
/// `session_id`, which the Stop-hook debrief flow always passes.
const SCAN_ATTEMPTS: u32 = 2;
const SCAN_RETRY_DELAY: Duration = Duration::from_millis(150);

/// The server: project directory to scan and parsed-session cache.
pub struct SumcpServer {
    /// `~/.claude/projects/<dashified-cwd>` for the cwd we were launched in.
    pub project_dir: PathBuf,
    /// Memoized transcript parser (ADR A3).
    pub store: SessionStore,
}

impl SumcpServer {
    /// Identify the calling session from its forwarded tool_use id — the
    /// ADR A4 scan, fail-closed. No forwarded id → no scan, straight to
    /// `ambiguous_session` (the explicit `session_id` param is the recovery
    /// path; there is deliberately no weaker marker to fall back to).
    async fn identify_caller(&self, tool_use_id: Option<&str>) -> Result<Resolved, IdentifyError> {
        let ambiguous = || IdentifyError::Ambiguous(identify::recent_candidates(&self.project_dir));
        let Some(id) = tool_use_id else {
            return Err(ambiguous());
        };
        for attempt in 0..SCAN_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(SCAN_RETRY_DELAY).await;
            }
            // The forwarded tool_use id is globally unique — a tail that
            // contains it *is* the caller.
            let hits = identify::scan_for_marker(&self.project_dir, id);
            match hits.len() {
                1 => {
                    let path = hits.into_iter().next().unwrap();
                    let id = identify::stem_id(&path);
                    return Ok(Resolved {
                        path,
                        id,
                        identified_by: "tool_use_id",
                    });
                }
                0 => continue, // possibly flush delay — retry
                _ => break,    // genuinely ambiguous — waiting can't help
            }
        }
        Err(ambiguous())
    }

    /// Resolve the session for one call: explicit `session_id` wins, else the
    /// verified scan. Never a recency guess (ADR A4).
    async fn resolve(
        &self,
        args: &serde_json::Map<String, serde_json::Value>,
        tool_use_id: Option<&str>,
    ) -> Result<Resolved, IdentifyError> {
        match args.get("session_id") {
            Some(serde_json::Value::String(raw)) => {
                identify::resolve_explicit(&self.project_dir, raw)
            }
            // Present but not a string (123, null, [...]): reject loudly.
            // Falling through to the scan here could answer about a
            // different session than the caller explicitly targeted.
            Some(other) => Err(IdentifyError::InvalidId(other.to_string())),
            None => self.identify_caller(tool_use_id).await,
        }
    }
}

/// Pull the forwarded Anthropic tool_use id out of `_meta` — the exact key
/// Claude Code sends. If the key is ever renamed the id simply isn't found
/// and the caller recovers via `ambiguous_session` + explicit `session_id`,
/// so there is nothing to future-proof here.
pub fn meta_tool_use_id(meta: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    let id = meta.get("claudecode/toolUseId")?.as_str()?;
    valid_tool_use_id(id).then(|| id.to_string())
}

/// `_meta` is caller-controlled, and the id becomes a substring probe over
/// transcript tails — constrain it to the real `toolu_…` shape so it can't
/// be used as a free-form content-existence oracle.
fn valid_tool_use_id(id: &str) -> bool {
    id.len() <= 64
        && id.starts_with("toolu_")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate `evidence`'s `idxs` argument strictly: every element must be a
/// non-negative integer that fits an action index. Silently dropping bad
/// elements would return a plausible-but-wrong empty result; rejecting names
/// the actual problem.
fn parse_idxs(args: &serde_json::Map<String, serde_json::Value>) -> Result<Vec<Idx>, String> {
    let arr = args
        .get("idxs")
        .and_then(|v| v.as_array())
        .ok_or("evidence requires an 'idxs' integer array")?;
    arr.iter()
        .map(|v| {
            v.as_u64()
                .and_then(|i| u32::try_from(i).ok())
                .map(Idx)
                .ok_or_else(|| format!("idxs element {v} is not a valid action index"))
        })
        .collect()
}

/// Map an identification failure to a tool-level error result the calling
/// agent can actually read (protocol errors render opaquely — see rmcp docs).
fn identify_error_result(err: IdentifyError) -> CallToolResult {
    let payload = match err {
        IdentifyError::Ambiguous(candidates) => identify::ambiguous_payload(&candidates),
        IdentifyError::InvalidId(raw) => serde_json::json!({
            "v": 2, "error": "invalid_session_id",
            "message": format!("'{raw}' is not a valid session id (36-char UUID expected)")
        }),
        IdentifyError::NotFound(id) => serde_json::json!({
            "v": 2, "error": "session_not_found",
            "message": format!("no transcript for session '{id}' in this project")
        }),
    };
    CallToolResult::error(vec![ContentBlock::text(payload.to_string())])
}

/// Build one tool descriptor. Every tool is read-only and closed-world; every
/// tool accepts an optional `session_id` (ADR A4's explicit escape hatch).
fn tool(
    name: &'static str,
    description: &'static str,
    mut props: serde_json::Value,
    required: &[&str],
) -> Tool {
    props["session_id"] = serde_json::json!({
        "type": "string",
        "description": "Explicit session UUID. Omit to let the server verify the calling session; required if the server answers ambiguous_session."
    });
    let mut schema = serde_json::json!({"type": "object", "properties": props});
    if !required.is_empty() {
        schema["required"] = serde_json::json!(required);
    }
    // rmcp's model structs are #[non_exhaustive] (new protocol fields may
    // appear), so construction goes through the provided builders.
    Tool::new(
        name,
        description,
        Arc::new(schema.as_object().cloned().unwrap_or_default()),
    )
    .annotate(
        ToolAnnotations::new()
            .read_only(true)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

/// The eight tools of the v1 contract (`docs/payload-schema.md`).
fn tool_list() -> Vec<Tool> {
    vec![
        tool(
            "session_overview",
            "Totals, token economics, and top-3 struggle files for the session. Start here.",
            serde_json::json!({}),
            &[],
        ),
        tool(
            "struggle_areas",
            "Ranked struggle files with per-category breakdown, the ranking rule used, and evidence-backed findings.",
            serde_json::json!({"n": {"type": "integer", "description": "Max files to return (default 5)."}}),
            &[],
        ),
        tool(
            "file_story",
            "Chronological event story for one file (head + tail kept, middle elided).",
            serde_json::json!({"path": {"type": "string", "description": "File path exactly as it appears in findings."}}),
            &["path"],
        ),
        tool(
            "blind_spots",
            "Blind-write attempts and large-write-instant-accept outliers, with suppression status for heuristic metrics.",
            serde_json::json!({}),
            &[],
        ),
        tool(
            "context_health",
            "Cache hit ratio and token economics (informational).",
            serde_json::json!({}),
            &[],
        ),
        tool(
            "evidence",
            "Dereference finding idxs into the raw actions that prove them (≤10 actions, excerpts ≤600 chars).",
            serde_json::json!({"idxs": {"type": "array", "items": {"type": "integer"}, "description": "Action indices from a finding's idxs field."}}),
            &["idxs"],
        ),
        tool(
            "review_context",
            "Recorded session context for reviewing a change: what was asked, what the human decided (and the other options that were offered), what was left unfinished, and what the agent claimed it did. Facts with citations, never judgments.",
            serde_json::json!({}),
            &[],
        ),
        // Deliberately no change-scoped selector argument (e.g. "only
        // requests touching these files/commits"). A reviewer raised it and
        // it is a real idea, but it needs a design decision this fix wave
        // does not make: what a selector would key on (file paths? a commit
        // range, which the server has no repo access to resolve, per
        // f4085cf's commit message? something else?). Adding one here would
        // be a new feature riding along on a bug-fix commit. Considered and
        // deferred.
        tool(
            "session_intent",
            "The full verbatim human requests for the session, large and unfiltered. Call this only after the diff and review_context have both been read and a specific ambiguity remains that neither one resolves. Loading it speculatively adds noise and degrades review quality rather than improving it.",
            serde_json::json!({"max_tokens": {"type": "integer", "description": "Optional smaller budget. Can only lower the default cap, never raise it."}}),
            &[],
        ),
    ]
}

impl ServerHandler for SumcpServer {
    fn get_info(&self) -> ServerInfo {
        // #[non_exhaustive] structs can't be built literally — start from
        // defaults and set the fields we own.
        let mut server_info = Implementation::from_build_env();
        server_info.name = SERVER_KEY.into();
        server_info.version = env!("CARGO_PKG_VERSION").into();
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info.instructions = Some(
            "Post-session forensics for Claude Code transcripts. Eight read-only tools \
             return compact JSON evidence (never narration). If a call returns \
             ambiguous_session, retry with an explicit session_id from the candidates."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tool_list(),
            ..ListToolsResult::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let args = request.arguments.clone().unwrap_or_default();

        // The forwarded tool_use id can arrive on the request params' _meta
        // or the transport-level request meta — check both.
        let tool_use_id = request
            .meta
            .as_ref()
            .and_then(|m| meta_tool_use_id(&m.0))
            .or_else(|| meta_tool_use_id(&context.meta.0));

        let resolved = match self.resolve(&args, tool_use_id.as_deref()).await {
            Ok(r) => r,
            // Identification failures are tool-level errors: the caller needs
            // to *see* the candidate list to recover (fail-closed, ADR A4).
            Err(e) => return Ok(identify_error_result(e)),
        };

        // Error detail is caller-visible: session id + error kind only —
        // never the absolute path (it leaks the home dir / username).
        let loaded = self.store.load(&resolved.path).map_err(|e| {
            ErrorData::internal_error(
                format!(
                    "could not read transcript for session '{}': {}",
                    resolved.id,
                    e.kind()
                ),
                None,
            )
        })?;
        let meta = SessionMeta {
            id: resolved.id,
            identified_by: resolved.identified_by.into(),
            // `loaded` now covers the whole work unit the caller's transcript
            // belongs to (see `store.rs`), so this is the real grouping
            // rather than a permanent `None`.
            unit: loaded.meta_unit.clone(),
        };
        let session = &loaded.session;
        let ranked = rank(session);

        let payload = match request.name.as_ref() {
            "session_overview" => payloads::session_overview(session, &ranked, &meta),
            "struggle_areas" => {
                let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                payloads::struggle_areas(session, &ranked, &meta, n)
            }
            "file_story" => {
                let path = args.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                    ErrorData::invalid_params("file_story requires a 'path' string", None)
                })?;
                payloads::file_story(session, path, &meta)
            }
            "blind_spots" => payloads::blind_spots(session, &meta),
            "context_health" => payloads::context_health(session, &meta),
            "evidence" => {
                let idxs = parse_idxs(&args).map_err(|msg| ErrorData::invalid_params(msg, None))?;
                payloads::evidence(session, &idxs, &meta)
            }
            "review_context" => payloads::review_context(session, &meta),
            "session_intent" => {
                let max = args
                    .get("max_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                payloads::session_intent(session, &meta, max)
            }
            other => {
                return Err(ErrorData::invalid_params(
                    format!("unknown tool '{other}'"),
                    None,
                ));
            }
        };

        // Compact JSON text (ADR A5) — the agent parses, the agent narrates.
        Ok(CallToolResult::success(vec![ContentBlock::text(
            payload.to_string(),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID_A: &str = "5717aaaa-1111-2222-3333-444455556666";
    const ID_B: &str = "80b9a169-624f-4880-a2c3-24b96e2b4ea2";

    fn server(dir: &std::path::Path) -> SumcpServer {
        SumcpServer {
            project_dir: dir.to_path_buf(),
            store: SessionStore::new(),
        }
    }

    fn meta_map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        serde_json::from_value(v).unwrap()
    }

    impl SumcpServer {
        /// Test-only mirror of `call_tool`. The real `call_tool` takes a
        /// `RequestContext<RoleServer>`, which rmcp only builds through a live
        /// connection, so it is awkward to construct by hand in a unit test.
        /// This helper walks the SAME steps `call_tool` does (resolve the
        /// session, load it through the store, dispatch to the right
        /// `payloads::` function) but hands back the parsed `serde_json::Value`
        /// directly instead of wrapping it in rmcp's `CallToolResult`. Every
        /// test in this module that needs to inspect an actual payload should
        /// go through this rather than reaching into `payloads::` itself, so
        /// the test exercises the real resolve-then-load path.
        async fn call_tool_for_test(
            &self,
            name: &str,
            args: &serde_json::Map<String, serde_json::Value>,
        ) -> Result<serde_json::Value, String> {
            let resolved = self
                .resolve(args, None)
                .await
                .map_err(|e| format!("identify failed: {e:?}"))?;
            let loaded = self
                .store
                .load(&resolved.path)
                .map_err(|e| format!("load failed: {}", e.kind()))?;
            let meta = SessionMeta {
                id: resolved.id,
                identified_by: resolved.identified_by.into(),
                unit: loaded.meta_unit.clone(),
            };
            let ranked = rank(&loaded.session);
            let payload = match name {
                "session_overview" => payloads::session_overview(&loaded.session, &ranked, &meta),
                "struggle_areas" => {
                    let n = args.get("n").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                    payloads::struggle_areas(&loaded.session, &ranked, &meta, n)
                }
                "file_story" => {
                    let path = args
                        .get("path")
                        .and_then(|v| v.as_str())
                        .ok_or("file_story requires a 'path' string")?;
                    payloads::file_story(&loaded.session, path, &meta)
                }
                "blind_spots" => payloads::blind_spots(&loaded.session, &meta),
                "context_health" => payloads::context_health(&loaded.session, &meta),
                "evidence" => {
                    let idxs = parse_idxs(args)?;
                    payloads::evidence(&loaded.session, &idxs, &meta)
                }
                "review_context" => payloads::review_context(&loaded.session, &meta),
                "session_intent" => {
                    let max = args
                        .get("max_tokens")
                        .and_then(|v| v.as_u64())
                        .map(|n| n as usize);
                    payloads::session_intent(&loaded.session, &meta, max)
                }
                other => return Err(format!("unknown tool '{other}'")),
            };
            Ok(payload)
        }
    }

    #[tokio::test]
    async fn tools_report_the_work_unit_not_one_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
        let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
        let line = |sess: &str, ts: &str, path: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
            )
        };
        std::fs::write(
            dir.path().join(format!("{id_a}.jsonl")),
            line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{id_b}.jsonl")),
            line(id_b, "2026-01-01T00:05:00Z", "/y.rs"),
        )
        .unwrap();

        let args = meta_map(serde_json::json!({"session_id": id_b}));
        let out = server(dir.path())
            .call_tool_for_test("session_overview", &args)
            .await
            .expect("a payload");
        assert_eq!(out["work_unit"]["sessions"], 2);
        assert_eq!(out["totals"]["edits"], 2);
    }

    #[test]
    fn meta_tool_use_id_takes_the_exact_key_and_validates_shape() {
        let m = meta_map(
            serde_json::json!({"claudecode/toolUseId": "toolu_01XyZ-9", "progressToken": "p"}),
        );
        assert_eq!(meta_tool_use_id(&m).as_deref(), Some("toolu_01XyZ-9"));
        // Other keys never match, even lookalikes.
        let other = meta_map(serde_json::json!({"x/toolUseId": "toolu_01X"}));
        assert_eq!(meta_tool_use_id(&other), None);
        // The id is caller-controlled: reject free-form probe strings.
        for bad in [
            "not_a_tool_use_id",
            "toolu_has spaces",
            "toolu_quote\"inject",
        ] {
            let m = meta_map(serde_json::json!({"claudecode/toolUseId": bad}));
            assert_eq!(meta_tool_use_id(&m), None, "must reject {bad:?}");
        }
        let long = format!("toolu_{}", "a".repeat(80));
        let m = meta_map(serde_json::json!({"claudecode/toolUseId": long}));
        assert_eq!(meta_tool_use_id(&m), None, "must reject over-long ids");
    }

    #[test]
    fn parse_idxs_rejects_garbage_instead_of_dropping_it() {
        let ok = meta_map(serde_json::json!({"idxs": [0, 42]}));
        assert_eq!(parse_idxs(&ok).unwrap(), vec![Idx(0), Idx(42)]);
        for bad in [
            serde_json::json!({"idxs": [-1]}),
            serde_json::json!({"idxs": ["5"]}),
            serde_json::json!({"idxs": [1.5]}),
            serde_json::json!({"idxs": [4294967296u64]}), // u32::MAX + 1
            serde_json::json!({"idxs": "not an array"}),
            serde_json::json!({}),
        ] {
            assert!(
                parse_idxs(&meta_map(bad.clone())).is_err(),
                "must reject {bad}"
            );
        }
    }

    #[tokio::test]
    async fn non_string_session_id_is_rejected_not_scanned() {
        let dir = tempfile::tempdir().unwrap();
        let args = meta_map(serde_json::json!({"session_id": 123}));
        let err = server(dir.path()).resolve(&args, None).await.unwrap_err();
        assert!(matches!(err, IdentifyError::InvalidId(_)));
    }

    #[test]
    fn every_tool_is_read_only_and_accepts_session_id() {
        let tools = tool_list();
        assert_eq!(tools.len(), 8);
        for t in &tools {
            let a = t.annotations.as_ref().unwrap();
            assert_eq!(a.read_only_hint, Some(true), "{} must be read-only", t.name);
            assert_eq!(a.open_world_hint, Some(false));
            assert!(
                t.input_schema["properties"]["session_id"].is_object(),
                "{} must accept session_id",
                t.name
            );
        }
    }

    #[tokio::test]
    async fn tool_use_id_in_one_tail_identifies_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(format!("{ID_A}.jsonl")),
            r#"{"id":"toolu_CALLER"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join(format!("{ID_B}.jsonl")), "{}").unwrap();
        let r = server(dir.path())
            .identify_caller(Some("toolu_CALLER"))
            .await
            .unwrap();
        assert_eq!(r.id, ID_A);
        assert_eq!(r.identified_by, "tool_use_id");
    }

    #[tokio::test]
    async fn duplicated_marker_in_two_tails_fails_closed_with_candidates() {
        let dir = tempfile::tempdir().unwrap();
        // Attacker-influenceable content: a second transcript QUOTING the
        // caller's id (e.g. via a pasted log) must force refusal, not a pick.
        for id in [ID_A, ID_B] {
            std::fs::write(
                dir.path().join(format!("{id}.jsonl")),
                r#"{"x":"toolu_DUP"}"#,
            )
            .unwrap();
        }
        let err = server(dir.path())
            .identify_caller(Some("toolu_DUP"))
            .await
            .unwrap_err();
        match err {
            IdentifyError::Ambiguous(c) => assert_eq!(c.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn no_forwarded_id_is_immediately_ambiguous_without_retry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("{ID_A}.jsonl")), "{}").unwrap();
        let started = std::time::Instant::now();
        let err = server(dir.path()).identify_caller(None).await.unwrap_err();
        // No marker to wait for → refusing must not burn the retry budget.
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(matches!(err, IdentifyError::Ambiguous(c) if c.len() == 1));
    }

    #[tokio::test]
    async fn zero_matches_fail_closed_after_bounded_retry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(format!("{ID_A}.jsonl")), "{}").unwrap();
        let started = std::time::Instant::now();
        let err = server(dir.path())
            .identify_caller(Some("toolu_NEVER"))
            .await
            .unwrap_err();
        // Proves the retry loop actually waited (attempts − 1 sleeps),
        // derived from the constants so tuning them doesn't break the test.
        assert!(started.elapsed() >= SCAN_RETRY_DELAY * (SCAN_ATTEMPTS - 1));
        // …and still refused to guess, listing the one session it saw.
        match err {
            IdentifyError::Ambiguous(c) => assert_eq!(c.len(), 1),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }
}
