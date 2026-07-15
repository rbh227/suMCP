//! The six MCP tool payloads (T3.5), built to the frozen v0 contract
//! (`docs/payload-schema.md`) and enforced by `scripts/check_payloads.py`.
//!
//! Compact JSON, hard token caps, `truncated` markers. The tool returns
//! evidence; the connected agent narrates. Every payload carries the ADR A4
//! provenance in `session.identified_by`.

use crate::model::{Action, ActionKind, Idx, Session};
use crate::report::Overview;
use crate::score::{FileScore, Weights};
use serde_json::{Value, json};

/// Token-cap headroom uses chars/3.5 (compact JSON tokenizes hot).
const CHARS_PER_TOKEN: f64 = 3.5;
/// Max findings shown per file in `struggle_areas`.
const FINDINGS_PER_FILE: usize = 4;
/// Max actions returned by `evidence`.
const EVIDENCE_MAX: usize = 10;
/// Max excerpt chars per evidence action.
const EXCERPT_MAX: usize = 600;
/// `file_story` keeps this many head and tail events, eliding the middle.
const STORY_EDGE: usize = 8;

/// Session identity + how it was resolved (ADR A4 provenance).
pub struct SessionMeta {
    /// Session id.
    pub id: String,
    /// `tool_use_id` | `explicit` | `cli_latest`.
    pub identified_by: String,
}

/// Approximate token count of a serialized payload.
pub fn est_tokens(v: &Value) -> usize {
    (v.to_string().len() as f64 / CHARS_PER_TOKEN).ceil() as usize
}

/// `session_overview()` — totals + top-3 struggle files.
pub fn session_overview(s: &Session, ranked: &[FileScore], meta: &SessionMeta) -> Value {
    let o = Overview::from_session(s);
    let top: Vec<Value> = ranked
        .iter()
        .take(3)
        .map(|f| json!({"file": f.file, "score": round1(f.score), "breakdown": f.breakdown}))
        .collect();
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "totals": {
            "actions": o.actions, "edits": o.edits, "writes": o.writes,
            "reads": o.reads, "bash": o.bash, "files_touched": o.files_touched,
            "interrupts": s.interrupts
        },
        "tokens": {
            "output": o.output_tokens, "cache_read": o.cache_read_tokens,
            "cache_hit_ratio": o.cache_hit_ratio.map(round2)
        },
        "top_struggles": top,
        "flags": {
            "unknown_event_types": s.type_counts,
            "parse_errors": o.parse_errors,
            "untimestamped_lines": o.untimestamped_lines
        },
        "truncated": false
    })
}

/// `struggle_areas(n)` — ranked files with breakdown, weights, findings.
pub fn struggle_areas(
    ranked: &[FileScore],
    weights: &Weights,
    meta: &SessionMeta,
    n: usize,
) -> Value {
    let truncated = ranked.len() > n || ranked.iter().any(|f| f.findings.len() > FINDINGS_PER_FILE);
    let files: Vec<Value> = ranked
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, f)| {
            let findings: Vec<&_> = f.findings.iter().take(FINDINGS_PER_FILE).collect();
            json!({
                "rank": i + 1, "file": f.file, "score": round1(f.score),
                "breakdown": f.breakdown, "findings": findings
            })
        })
        .collect();
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "weights": weights,
        "files": files,
        "findings_per_file_cap": FINDINGS_PER_FILE,
        "truncated": truncated
    })
}

/// `file_story(path)` — chronological events for one file, elided middle-out.
pub fn file_story(s: &Session, path: &str, meta: &SessionMeta) -> Value {
    let events: Vec<&Action> = s
        .actions
        .iter()
        .filter(|a| a.file_path.as_deref() == Some(path))
        .collect();
    let render = |a: &Action| {
        json!({
            "idx": a.idx, "t": a.effective_ts, "action": kind_str(&a.kind),
            "outcome": a.is_error.map(|e| if e {"fail"} else {"ok"})
        })
    };
    let (head, tail, elided) = if events.len() > 2 * STORY_EDGE {
        let head: Vec<Value> = events[..STORY_EDGE].iter().map(|a| render(a)).collect();
        let tail: Vec<Value> = events[events.len() - STORY_EDGE..]
            .iter()
            .map(|a| render(a))
            .collect();
        let between = json!({
            "count": events.len() - 2 * STORY_EDGE,
            "note": "middle events elided; fetch via evidence(idxs)"
        });
        (head, tail, Some(between))
    } else {
        (events.iter().map(|a| render(a)).collect(), Vec::new(), None)
    };
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "file": path,
        "events": head,
        "elided": elided,
        "tail": tail,
        "truncated": elided.is_some()
    })
}

/// `blind_spots()` — blind-write attempts and approval outliers.
pub fn blind_spots(s: &Session, meta: &SessionMeta) -> Value {
    use crate::model::FindingKind;
    let all = crate::score::all_findings(s);
    let blind: Vec<_> = all
        .iter()
        .filter(|f| matches!(f.kind, FindingKind::BlindWriteAttempt))
        .collect();
    let approval: Vec<_> = all
        .iter()
        .filter(|f| matches!(f.kind, FindingKind::LargeWriteInstantAccept))
        .collect();
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "blind_write_attempts": blind,
        "approval_outliers": approval,
        "suppression": {
            "approval_latency": if crate::signals::comprehension::approval_latency_active(s) {"active"} else {"suppressed"},
            "suppressed_when": "permissionMode grants auto-accept"
        },
        "truncated": false
    })
}

/// `context_health()` — cache ratio and token economics (informational only).
pub fn context_health(s: &Session, meta: &SessionMeta) -> Value {
    let o = Overview::from_session(s);
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "cache_hit_ratio": o.cache_hit_ratio.map(round2),
        "tokens": {
            "output": s.tokens.output, "input_uncached": s.tokens.input,
            "cache_read": s.tokens.cache_read, "cache_creation": s.tokens.cache_creation
        },
        "note": "v0.1 reports context economics as information only; token-level waste deferred",
        "truncated": false
    })
}

/// `evidence(idxs)` — raw actions behind findings, capped.
pub fn evidence(s: &Session, idxs: &[Idx], meta: &SessionMeta) -> Value {
    let mut found = Vec::new();
    let mut not_found = Vec::new();
    for &Idx(i) in idxs.iter().take(EVIDENCE_MAX) {
        match s.actions.get(i as usize) {
            Some(a) => found.push(json!({
                "idx": a.idx, "t": a.effective_ts, "tool": kind_str(&a.kind),
                "file": a.file_path,
                "excerpt": excerpt(a)
            })),
            None => not_found.push(i),
        }
    }
    json!({
        "v": 0,
        "session": {"id": meta.id, "identified_by": meta.identified_by},
        "actions": found,
        "not_found": not_found,
        "caps": {"max_actions": EVIDENCE_MAX, "max_excerpt_chars": EXCERPT_MAX},
        "truncated": idxs.len() > EVIDENCE_MAX
    })
}

fn excerpt(a: &Action) -> String {
    let raw = a
        .command
        .as_deref()
        .or(a.error.as_deref())
        .or(a.edit_new.as_deref())
        .unwrap_or("");
    raw.chars().take(EXCERPT_MAX).collect()
}

fn kind_str(k: &ActionKind) -> String {
    match k {
        ActionKind::Read => "Read".into(),
        ActionKind::Edit => "Edit".into(),
        ActionKind::Write => "Write".into(),
        ActionKind::Bash => "Bash".into(),
        ActionKind::Other(n) => n.clone(),
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;
    use crate::score::{Weights, rank};

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "abc".into(),
            identified_by: "explicit".into(),
        }
    }

    fn busy_session() -> Session {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
            ));
        }
        ingest_str(&lines.join("\n"), Lane::Main)
    }

    #[test]
    fn all_payloads_are_valid_json_with_provenance_and_under_cap() {
        let s = busy_session();
        let w = Weights::default();
        let r = rank(&s, &w);
        let m = meta();
        let caps = [
            (session_overview(&s, &r, &m), 1000),
            (struggle_areas(&r, &w, &m, 10), 1500),
            (file_story(&s, "/a.ts", &m), 1500),
            (blind_spots(&s, &m), 1000),
            (context_health(&s, &m), 1000),
            (evidence(&s, &[Idx(0), Idx(1)], &m), 1500),
        ];
        for (payload, cap) in caps {
            assert_eq!(payload["v"], 0);
            assert_eq!(payload["session"]["identified_by"], "explicit");
            assert!(
                payload.get("truncated").is_some(),
                "truncated flag required"
            );
            assert!(est_tokens(&payload) <= cap, "over cap: {}", payload);
        }
    }

    #[test]
    fn struggle_areas_echoes_weights_and_breakdown() {
        let s = busy_session();
        let w = Weights::default();
        let p = struggle_areas(&rank(&s, &w), &w, &meta(), 5);
        assert_eq!(p["weights"]["source"], "defaults");
        assert!(p["files"][0]["breakdown"]["churn"].as_u64().unwrap() >= 2);
    }
}
