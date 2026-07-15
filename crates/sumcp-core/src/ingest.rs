//! Permissive ingestion: raw JSONL text → an ordered [`Session`].
//!
//! Design rules (SPEC §1, metrics-spec parser rules):
//! - Never fail the file on one bad line — count it and move on.
//! - Every field is optional; the schema is undocumented and drifts.
//! - Unknown event types are data, not errors.
//! - Three dedup layers: (a) usage summed once per `message.id`,
//!   (b) actions deduped by `tool_use` id so resumed-session replays and
//!   streaming duplicates don't inflate counts, (c) content preserved.
//!
//! We parse each line into a `serde_json::Value` and navigate it by hand
//! rather than into strict structs. Against a drifting, undocumented schema
//! that is the more robust choice: a surprising shape in one field costs us
//! that field, not the whole line's type/uuid/timestamp.

use crate::model::{Action, ActionKind, Idx, Lane, Session, Tokens};
use serde_json::Value;
use std::collections::BTreeMap;

/// Parse raw transcript text (one JSON object per line) into a [`Session`].
///
/// `default_lane` is `Lane::Main` for the primary transcript; subagent files
/// pass their own lane so a later merge can interleave them.
pub fn ingest_str(raw: &str, default_lane: Lane) -> Session {
    // Mutable accumulators. `mut` is required to reassign/insert — Rust makes
    // mutability explicit, so a reader knows exactly what changes.
    let mut type_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut usage_by_msg: BTreeMap<String, Tokens> = BTreeMap::new();
    let mut parse_errors = 0u64;
    let mut untimestamped = 0u64;
    let mut last_ts = String::new(); // carried forward for untimestamped lines
    let mut seen_tool_ids = std::collections::HashSet::new();

    // Pre-`Idx` actions: we don't know the final order until all lines are in,
    // so collect (sort_key, partial action) and assign Idx after sorting.
    let mut pending: Vec<PendingAction> = Vec::new();

    // `.lines()` is a lazy iterator; `.enumerate()` pairs each with its index.
    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // Parse to a generic Value. `match` forces us to handle the Err arm —
        // there is no way to accidentally ignore a parse failure.
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                parse_errors += 1; // bad line counted, file survives
                continue;
            }
        };

        // `type` histogram (every type, known or not).
        if let Some(t) = v.get("type").and_then(Value::as_str) {
            *type_counts.entry(t.to_string()).or_insert(0) += 1;
        }

        // effective timestamp: own, else carry forward the last one we saw.
        let (effective_ts, inherited) = match v.get("timestamp").and_then(Value::as_str) {
            Some(ts) => {
                last_ts = ts.to_string();
                (ts.to_string(), false)
            }
            None => {
                untimestamped += 1;
                (last_ts.clone(), true)
            }
        };

        let message = v.get("message");

        // Dedup layer (a): usage last-wins per message.id.
        // A "let-chain" (Rust 2024): each `let` binds only if the previous
        // matched, so this reads as "if there's a message AND it has an id AND
        // a usage block" — no nested `if`s.
        if let Some(msg) = message
            && let Some(id) = msg.get("id").and_then(Value::as_str)
            && let Some(usage) = msg.get("usage")
        {
            usage_by_msg.insert(id.to_string(), read_usage(usage));
        }

        // Extract tool_use actions from message.content blocks.
        // content may be a string (user text) or an array of blocks; we only
        // care about the array form here. `and_then` chains the Options so a
        // missing/wrong-typed field just yields None and we skip.
        if let Some(blocks) = message
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let tool_id = block.get("id").and_then(Value::as_str);
                // Dedup layer (b): first occurrence of a tool_use id wins.
                // `HashSet::insert` returns false if the id was already present.
                if let Some(id) = tool_id
                    && !seen_tool_ids.insert(id.to_string())
                {
                    continue; // replay/streaming duplicate — already counted
                }
                let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                let file_path = block
                    .get("input")
                    .and_then(|i| i.get("file_path"))
                    .and_then(Value::as_str)
                    .map(str::to_string);

                pending.push(PendingAction {
                    effective_ts: effective_ts.clone(),
                    ts_inherited: inherited,
                    lane: default_lane.clone(),
                    line_no,
                    kind: ActionKind::from_tool(name),
                    file_path,
                    is_error: None, // wired to tool_result in a later task
                });
            }
        }
    }

    // Apply the total ordering contract, then assign monotonic Idx.
    // `sort_by` with a tuple key sorts lexicographically: timestamp, then lane
    // (Main < Sub), then line number — a total, deterministic order.
    pending.sort_by(|a, b| {
        (&a.effective_ts, &a.lane, a.line_no).cmp(&(&b.effective_ts, &b.lane, b.line_no))
    });

    let actions = pending
        .into_iter()
        .enumerate()
        .map(|(i, p)| Action {
            idx: Idx(i as u32),
            effective_ts: p.effective_ts,
            ts_inherited: p.ts_inherited,
            lane: p.lane,
            line_no: p.line_no,
            kind: p.kind,
            file_path: p.file_path,
            is_error: p.is_error,
        })
        .collect();

    // Sum the deduped per-message usage into session totals.
    let mut tokens = Tokens::default();
    for u in usage_by_msg.values() {
        tokens.input += u.input;
        tokens.output += u.output;
        tokens.cache_read += u.cache_read;
        tokens.cache_creation += u.cache_creation;
    }

    Session {
        actions,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines: untimestamped,
    }
}

/// An action before we know its final `Idx` (order isn't known until all lines
/// are read). Private to this module.
struct PendingAction {
    effective_ts: String,
    ts_inherited: bool,
    lane: Lane,
    line_no: usize,
    kind: ActionKind,
    file_path: Option<String>,
    is_error: Option<bool>,
}

/// Read a `usage` object into [`Tokens`], tolerating missing fields.
fn read_usage(v: &Value) -> Tokens {
    // A tiny closure to pull a u64 field or default to 0.
    let g = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Tokens {
        input: g("input_tokens"),
        output: g("output_tokens"),
        cache_read: g("cache_read_input_tokens"),
        cache_creation: g("cache_creation_input_tokens"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_line_is_counted_not_fatal() {
        let raw = "not json\n{\"type\":\"user\",\"uuid\":\"a\"}";
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.parse_errors, 1);
        assert_eq!(s.type_counts.get("user"), Some(&1));
    }

    #[test]
    fn unknown_type_is_data() {
        let raw = "{\"type\":\"totally-new-type\",\"uuid\":\"x\"}";
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.type_counts.get("totally-new-type"), Some(&1));
        assert_eq!(s.parse_errors, 0);
    }

    #[test]
    fn replayed_tool_use_is_deduped() {
        // Same tool_use id on two lines (a resumed-session replay).
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"/a.ts"}}]}}"#;
        let s = ingest_str(&format!("{line}\n{line}"), Lane::Main);
        assert_eq!(s.actions.len(), 1, "duplicate tool_use id must not inflate");
        assert_eq!(s.actions[0].kind, ActionKind::Edit);
    }

    #[test]
    fn usage_summed_once_per_message_id() {
        // Streaming: two lines share message.id — usage counts once (last-wins).
        let l = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m1","usage":{"output_tokens":50,"input_tokens":10}}}"#;
        let s = ingest_str(&format!("{l}\n{l}"), Lane::Main);
        assert_eq!(s.tokens.output, 50);
        assert_eq!(s.tokens.input, 10);
    }

    #[test]
    fn untimestamped_line_inherits_previous_timestamp() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}}]}}"#,
            "\n",
            // no timestamp on this line; must inherit 00:00:05
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"/a"}}]}}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.untimestamped_lines, 1);
        let edit = s
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::Edit)
            .unwrap();
        assert_eq!(edit.effective_ts, "2026-01-01T00:00:05Z");
        assert!(edit.ts_inherited);
    }
}
