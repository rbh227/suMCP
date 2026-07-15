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

use crate::model::{Action, ActionKind, Idx, Lane, Session, Tokens, UserText};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Cap on stored edit strings — reverts are hunk-sized; this keeps a whole-file
/// paste from bloating the model while still allowing equality comparison.
const EDIT_CAP: usize = 2000;
/// Prefix Claude Code writes when the user interrupts a turn.
const INTERRUPT_PREFIX: &str = "[Request interrupted by user";

/// Parse raw transcript text (one JSON object per line) into a [`Session`].
///
/// `default_lane` is `Lane::Main` for the primary transcript; subagent files
/// pass their own lane so a later merge can interleave them.
pub fn ingest_str(raw: &str, default_lane: Lane) -> Session {
    let mut type_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut usage_by_msg: BTreeMap<String, Tokens> = BTreeMap::new();
    let mut parse_errors = 0u64;
    let mut untimestamped = 0u64;
    let mut last_ts = String::new(); // carried forward for untimestamped lines
    let mut seen_tool_ids = HashSet::new();
    let mut pending: Vec<PendingAction> = Vec::new();
    let mut user_texts: Vec<UserText> = Vec::new();
    let mut interrupts = 0u64;
    // tool_use id -> the result that came back for it (error text, patch hunks).
    let mut results: HashMap<String, ResultInfo> = HashMap::new();

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                parse_errors += 1; // bad line counted, file survives
                continue;
            }
        };

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

        // Capture real user text (prompts, interrupts) — not tool_result echoes
        // or meta lines. Placed in time so signals can ask "did the user push
        // back between edit A and edit B?".
        if v.get("type").and_then(Value::as_str) == Some("user")
            && v.get("isMeta").and_then(Value::as_bool) != Some(true)
            && let Some(text) = extract_user_text(message)
        {
            if text.starts_with(INTERRUPT_PREFIX) {
                interrupts += 1;
            }
            user_texts.push(UserText {
                effective_ts: effective_ts.clone(),
                line_no,
                text,
            });
        }

        // Dedup layer (a): usage last-wins per message.id (a let-chain).
        if let Some(msg) = message
            && let Some(id) = msg.get("id").and_then(Value::as_str)
            && let Some(usage) = msg.get("usage")
        {
            usage_by_msg.insert(id.to_string(), read_usage(usage));
        }

        // Walk message.content blocks: `tool_use` becomes an action;
        // `tool_result` is captured and joined back by tool_use id afterwards.
        if let Some(blocks) = message
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use") => {
                        let tool_id = block.get("id").and_then(Value::as_str);
                        // Dedup layer (b): first occurrence of a tool_use id wins.
                        if let Some(id) = tool_id
                            && !seen_tool_ids.insert(id.to_string())
                        {
                            continue; // replay/streaming duplicate
                        }
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                        let input = block.get("input");
                        let file_path = input
                            .and_then(|i| i.get("file_path"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        // large-write size: Write uses `content`, Edit `new_string`.
                        let write_len = input
                            .and_then(|i| i.get("content").or_else(|| i.get("new_string")))
                            .and_then(Value::as_str)
                            .map(str::len);
                        let command = input
                            .and_then(|i| i.get("command"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        // normalized old/new strings for revert detection
                        let edit_old = norm_cap(
                            input
                                .and_then(|i| i.get("old_string"))
                                .and_then(Value::as_str),
                        );
                        let edit_new = norm_cap(
                            input
                                .and_then(|i| i.get("new_string"))
                                .and_then(Value::as_str),
                        );

                        pending.push(PendingAction {
                            tool_use_id: tool_id.map(str::to_string),
                            effective_ts: effective_ts.clone(),
                            ts_inherited: inherited,
                            lane: default_lane.clone(),
                            line_no,
                            kind: ActionKind::from_tool(name),
                            file_path,
                            write_len,
                            command,
                            edit_old,
                            edit_new,
                        });
                    }
                    Some("tool_result") => {
                        if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                            let is_error = block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            // On error, fold in stderr (Bash detail lives there,
                            // not in the terse tool_result "Exit code N").
                            let error = if is_error {
                                let content = content_to_string(block.get("content"));
                                let stderr = v
                                    .get("toolUseResult")
                                    .and_then(|r| r.get("stderr"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                Some(format!("{content} {stderr}").trim().to_string())
                            } else {
                                None
                            };
                            // structuredPatch (edit line ranges) lives at the
                            // top level of the same line, paired with this result.
                            let hunks = read_hunks(v.get("toolUseResult"));
                            let user_modified = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("userModified"))
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            results.insert(
                                id.to_string(),
                                ResultInfo {
                                    is_error,
                                    error,
                                    hunks,
                                    user_modified,
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Total ordering contract → monotonic Idx (see model.rs).
    pending.sort_by(|a, b| {
        (&a.effective_ts, &a.lane, a.line_no).cmp(&(&b.effective_ts, &b.lane, b.line_no))
    });

    let actions = pending
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            // Join in the result that came back for this tool call, if any.
            let r = p.tool_use_id.as_deref().and_then(|id| results.get(id));
            Action {
                idx: Idx(i as u32),
                effective_ts: p.effective_ts,
                ts_inherited: p.ts_inherited,
                lane: p.lane,
                line_no: p.line_no,
                kind: p.kind,
                file_path: p.file_path,
                is_error: r.map(|r| r.is_error),
                write_len: p.write_len,
                error: r.and_then(|r| r.error.clone()),
                hunks: r.map(|r| r.hunks.clone()).unwrap_or_default(),
                command: p.command,
                user_modified: r.map(|r| r.user_modified).unwrap_or(false),
                edit_old: p.edit_old,
                edit_new: p.edit_new,
            }
        })
        .collect();

    let mut tokens = Tokens::default();
    for u in usage_by_msg.values() {
        tokens.input += u.input;
        tokens.output += u.output;
        tokens.cache_read += u.cache_read;
        tokens.cache_creation += u.cache_creation;
    }

    Session {
        actions,
        user_texts,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines: untimestamped,
        interrupts,
    }
}

/// Normalize whitespace and cap length, for edit-string equality comparison.
fn norm_cap(s: Option<&str>) -> Option<String> {
    s.map(|s| {
        let normalized: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        normalized.chars().take(EDIT_CAP).collect::<String>()
    })
    .filter(|s| !s.is_empty())
}

/// Pull real user text from a message: a plain string, or joined text blocks.
/// Returns `None` for tool-result-only lines (no human text).
fn extract_user_text(message: Option<&Value>) -> Option<String> {
    let content = message?.get("content")?;
    match content {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Array(arr) => {
            let text: String = arr
                .iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// An action before we know its final `Idx`. Private to this module.
struct PendingAction {
    tool_use_id: Option<String>,
    effective_ts: String,
    ts_inherited: bool,
    lane: Lane,
    line_no: usize,
    kind: ActionKind,
    file_path: Option<String>,
    write_len: Option<usize>,
    command: Option<String>,
    edit_old: Option<String>,
    edit_new: Option<String>,
}

/// What came back for a tool call.
struct ResultInfo {
    is_error: bool,
    error: Option<String>,
    hunks: Vec<(u32, u32)>,
    user_modified: bool,
}

/// Read a `usage` object into [`Tokens`], tolerating missing fields.
fn read_usage(v: &Value) -> Tokens {
    let g = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
    Tokens {
        input: g("input_tokens"),
        output: g("output_tokens"),
        cache_read: g("cache_read_input_tokens"),
        cache_creation: g("cache_creation_input_tokens"),
    }
}

/// Tool-result content can be a plain string or an array of text blocks;
/// flatten either into one string (best-effort, for error matching).
fn content_to_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// Extract edited line ranges `(start, start+lines)` from a `structuredPatch`.
fn read_hunks(tool_use_result: Option<&Value>) -> Vec<(u32, u32)> {
    tool_use_result
        .and_then(|r| r.get("structuredPatch"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let start = h.get("oldStart").and_then(Value::as_u64)? as u32;
                    let lines = h.get("oldLines").and_then(Value::as_u64).unwrap_or(0) as u32;
                    Some((start, start + lines))
                })
                .collect()
        })
        .unwrap_or_default()
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
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"id":"m1","content":[{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"/a.ts"}}]}}"#;
        let s = ingest_str(&format!("{line}\n{line}"), Lane::Main);
        assert_eq!(s.actions.len(), 1, "duplicate tool_use id must not inflate");
        assert_eq!(s.actions[0].kind, ActionKind::Edit);
    }

    #[test]
    fn usage_summed_once_per_message_id() {
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

    #[test]
    fn tool_result_error_and_hunks_join_to_action() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"tu1","name":"Edit","input":{"file_path":"/a.ts","new_string":"hello"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","is_error":false}]},"toolUseResult":{"structuredPatch":[{"oldStart":10,"oldLines":5}]}}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        let a = &s.actions[0];
        assert_eq!(a.write_len, Some(5), "new_string length captured");
        assert_eq!(
            a.hunks,
            vec![(10, 15)],
            "structuredPatch joined by tool_use id"
        );
    }
}
