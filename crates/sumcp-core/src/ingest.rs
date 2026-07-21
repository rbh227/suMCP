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

use crate::model::{Action, ActionKind, Idx, Lane, Session, Spawn, Tokens, UserText};
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
    let mut auto_accept = false;
    // Tool-use ids of Agent/Task spawns, in first-seen order (post-dedup).
    // Resolved to agentIds after the results map is built.
    let mut spawn_ids: Vec<String> = Vec::new();
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

        // Auto-accept permission modes make approval latency meaningless.
        let mode = v
            .get("permissionMode")
            .or_else(|| v.get("mode"))
            .and_then(Value::as_str);
        if matches!(mode, Some("acceptEdits") | Some("bypassPermissions")) {
            auto_accept = true;
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
                        // Subagent spawns: "Agent" in current Claude Code
                        // versions, "Task" in older ones (exact names — the
                        // task-list tools TaskCreate/TaskUpdate/… are NOT
                        // spawns). Counted post-dedup so resumed-session
                        // replays don't inflate the exclusion count.
                        if name == "Agent" || name == "Task" {
                            // Record the spawn's own tool_use id; we resolve
                            // its agentId from the paired result below.
                            if let Some(id) = tool_id {
                                spawn_ids.push(id.to_string());
                            }
                        }
                        let input = block.get("input");
                        let file_path = input
                            .and_then(|i| i.get("file_path"))
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        // large-write size: Write uses `content`, Edit `new_string`.
                        // Chars AND lines are counted here on the FULL string,
                        // before `norm_cap` truncates what we store — a capped
                        // copy would undercount exactly the large writes the
                        // review-burden signal (#27) exists to catch.
                        let new_content = input
                            .and_then(|i| i.get("content").or_else(|| i.get("new_string")))
                            .and_then(Value::as_str);
                        let write_len = new_content.map(str::len);
                        let write_lines = new_content.map(|s| s.lines().count());
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
                            write_lines,
                            input_hash: Some(hash_call(name, input)),
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
                            // Read results report the file's REAL size
                            // (`file.totalLines`) even for partial reads —
                            // the relative-churn denominator (#7). T2 field:
                            // absent on non-Read results, tolerated.
                            let read_total_lines = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("file"))
                                .and_then(|f| f.get("totalLines"))
                                .and_then(Value::as_u64)
                                .map(|n| n as usize);
                            // Subagent spawns' results carry the child agent's
                            // id here; it's the only link to the child's file.
                            let agent_id = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("agentId"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            results.insert(
                                id.to_string(),
                                ResultInfo {
                                    is_error,
                                    error,
                                    hunks,
                                    user_modified,
                                    result_ts: effective_ts.clone(),
                                    read_total_lines,
                                    agent_id,
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
            // Approval latency: only for Edit/Write, only same-day (execution
            // is ~instant, so proposal→result delta ≈ human decision time).
            let approval_latency_s = if matches!(p.kind, ActionKind::Edit | ActionKind::Write) {
                r.and_then(|r| latency_secs(&p.effective_ts, &r.result_ts))
            } else {
                None
            };
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
                write_lines: p.write_lines,
                read_total_lines: r.and_then(|r| r.read_total_lines),
                input_hash: p.input_hash,
                error: r.and_then(|r| r.error.clone()),
                hunks: r.map(|r| r.hunks.clone()).unwrap_or_default(),
                command: p.command,
                user_modified: r.map(|r| r.user_modified).unwrap_or(false),
                edit_old: p.edit_old,
                edit_new: p.edit_new,
                approval_latency_s,
            }
        })
        .collect();

    // Resolve each spawn's agentId from its paired result (if the result had
    // come back and carried one). Order preserved from first-seen.
    let spawns: Vec<Spawn> = spawn_ids
        .iter()
        .map(|id| Spawn {
            agent_id: results.get(id).and_then(|r| r.agent_id.clone()),
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
        auto_accept,
        spawns,
        subagent_files_missing: 0,
    }
}

/// Parse `YYYY-MM-DDTHH:MM:SS(.fff)Z` into `(date, seconds-of-day)`.
/// Best-effort; returns `None` on anything unexpected.
fn parse_iso(ts: &str) -> Option<(&str, f64)> {
    let (date, rest) = ts.split_once('T')?;
    let time = rest.trim_end_matches('Z');
    let mut parts = time.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?; // "07.117" parses fine as f64
    Some((date, h * 3600.0 + m * 60.0 + s))
}

/// Seconds between two ISO timestamps, or `None` if they cross a day boundary
/// (can't be "instant") or don't parse.
fn latency_secs(proposal: &str, result: &str) -> Option<f64> {
    let (pd, ps) = parse_iso(proposal)?;
    let (rd, rs) = parse_iso(result)?;
    if pd != rd {
        return None;
    }
    let d = rs - ps;
    (d >= 0.0).then_some(d)
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
    write_lines: Option<usize>,
    input_hash: Option<u64>,
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
    result_ts: String,
    read_total_lines: Option<usize>,
    /// The child agent's id from a subagent spawn's `toolUseResult.agentId`.
    agent_id: Option<String>,
}

/// Hash (tool name + raw input JSON) for the loop detector: byte-identical
/// calls hash equal, that's the whole contract. `DefaultHasher` is not stable
/// across Rust versions — fine, the hash never leaves this process.
fn hash_call(name: &str, input: Option<&Value>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    name.hash(&mut h);
    // `to_string` on a serde_json::Value is deterministic for the same parsed
    // input (keys keep their parse order), so equal lines ⇒ equal strings.
    if let Some(i) = input {
        i.to_string().hash(&mut h);
    }
    h.finish()
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

    #[test]
    fn write_lines_counted_on_full_content_before_cap() {
        // 300 lines of 20 chars ≈ 6300 chars — far beyond the storage cap, so
        // a post-cap count would be wrong. The count must use the full input.
        let content: String = (0..300)
            .map(|i| format!("line {i:04} xxxxxxxx\\n"))
            .collect();
        let raw = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"w1","name":"Write","input":{{"file_path":"/big.rs","content":"{content}"}}}}]}}}}"#
        );
        let s = ingest_str(&raw, Lane::Main);
        assert_eq!(s.actions[0].write_lines, Some(300));
    }

    #[test]
    fn read_total_lines_joined_from_tool_use_result() {
        // Shape verified against fixtures/raw (toolUseResult.file.totalLines).
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/a.rs","offset":10,"limit":50}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"r1","is_error":false}]},"toolUseResult":{"type":"text","file":{"filePath":"/a.rs","numLines":50,"startLine":10,"totalLines":413}}}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(
            s.actions[0].read_total_lines,
            Some(413),
            "totalLines is the real file size even for a partial read"
        );
    }

    #[test]
    fn subagent_spawns_counted_task_list_tools_ignored() {
        // "Agent" (current) and "Task" (older versions) are spawns; the
        // task-list tools that merely share the prefix are not.
        let call = |id: &str, name: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{}}}}]}}}}"#
            )
        };
        let raw = [
            call("a1", "Agent"),
            call("a2", "Task"),
            call("a3", "TaskCreate"),
            call("a4", "TaskUpdate"),
            call("a1", "Agent"), // replay duplicate — must not inflate
            // A tool_result for the first Agent spawn, carrying the agentId the
            // legacy-layout discovery links on.
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"a1","is_error":false}]},"toolUseResult":{"agentId":"agent-abc"}}"#.to_string(),
        ]
        .join("\n");
        let s = ingest_str(&raw, Lane::Main);
        assert_eq!(
            s.spawns.len(),
            2,
            "two Agent/Task spawns, task-list tools ignored"
        );
        // The paired tool_result carried an agentId for the first spawn only.
        assert_eq!(s.spawns[0].agent_id.as_deref(), Some("agent-abc"));
    }

    #[test]
    fn input_hash_equal_iff_calls_byte_identical() {
        let grep = |id: &str, pat: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Grep","input":{{"pattern":"{pat}"}}}}]}}}}"#
            )
        };
        let raw = format!(
            "{}\n{}\n{}",
            grep("g1", "foo"),
            grep("g2", "foo"),
            grep("g3", "bar")
        );
        let s = ingest_str(&raw, Lane::Main);
        assert_eq!(s.actions[0].input_hash, s.actions[1].input_hash);
        assert_ne!(s.actions[0].input_hash, s.actions[2].input_hash);
        assert!(s.actions[0].input_hash.is_some());
    }
}
