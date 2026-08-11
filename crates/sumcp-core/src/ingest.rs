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

use crate::model::{
    Action, ActionKind, AgentText, Decision, Idx, Lane, Session, Spawn, TaskEvent, Tokens,
    TurnOrigin, UserText,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Cap on stored edit strings — reverts are hunk-sized; this keeps a whole-file
/// paste from bloating the model while still allowing equality comparison.
const EDIT_CAP: usize = 2000;
/// Prefix Claude Code writes when the user interrupts a turn.
const INTERRUPT_PREFIX: &str = "[Request interrupted by user";
/// Longest prose block stored. Unlike `EDIT_CAP`, truncating here cannot skew
/// any metric: nothing counts these characters, they are only quoted.
///
/// There is deliberately no MINIMUM. A length floor was measured as a proxy
/// for "is this a verifiable claim" and rejected: across 8 real sessions the
/// 80-159 character band is mostly narration ("Now the rules engine and
/// TikTok driver:"), not assertion. Selection happens in `context::claims`
/// using the spec's rule instead.
pub(crate) const AGENT_TEXT_CAP: usize = 4000;

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
    let mut cwd: Option<String> = None;
    let mut seen_tool_ids = HashSet::new();
    let mut pending: Vec<PendingAction> = Vec::new();
    let mut user_texts: Vec<UserText> = Vec::new();
    let mut interrupts = 0u64;
    let mut auto_accept = false;
    // The permission mode currently in force. Claude Code emits a `mode`
    // event whenever it changes, and it changes often within one session
    // (96 times in one observed transcript), so this is carried forward line
    // by line exactly the way `last_ts` is, and stamped onto every action.
    let mut mode_is_auto = false;
    // Tool-use ids of Agent/Task spawns, in first-seen order (post-dedup).
    // Resolved to agentIds after the results map is built.
    let mut spawn_ids: Vec<String> = Vec::new();
    // tool_use id -> the result that came back for it (error text, patch hunks).
    let mut results: HashMap<String, ResultInfo> = HashMap::new();
    // Decisions arrive in two halves on different lines: the AskUserQuestion
    // call carries the question and options, the paired result carries the
    // answer. We stash the half we have and join by tool_use id at the end,
    // exactly the way spawns already resolve their agentId.
    //
    // The join MUST be scoped by tool_use_id, not by question text alone.
    // If a session asks the same question text in two different calls, a
    // text-only key would let the second call's answer overwrite the
    // first's in a shared map, so BOTH decisions would report the later
    // answer, a fabricated result for the earlier one. Each pending
    // decision therefore carries the tool_use_id of the call that asked it
    // (`None` only if the call's `id` was itself missing, which the "cannot
    // disambiguate" fallback below also covers), and `decision_answers` is
    // keyed by tool_use_id first, question text second, matching the shape
    // of the transcript's own per-call `answers` object.
    let mut pending_decisions: Vec<(Option<String>, bool, Decision)> = Vec::new();
    let mut decision_answers: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<String, String>,
    > = std::collections::BTreeMap::new();
    // Task creates and updates, like decisions, arrive in two halves on
    // different lines: the call, then its paired result. A create's real id
    // is only reported in the result (`toolUseResult.task.id`), and an
    // update can fail, in which case reporting the requested status as
    // though it happened would hide genuinely unfinished work. Both are
    // therefore staged here in the order they're seen and resolved by
    // tool_use_id after the full pass, exactly like `pending_decisions`.
    // `Session::task_events` documents itself as "in source order", so this
    // stays a single ordered `Vec` rather than separate create/update lists
    // that would need re-interleaving afterward.
    let mut pending_task_events: Vec<PendingTaskEvent> = Vec::new();
    // Every non-empty prose block the agent wrote ON THE MAIN LANE, in source
    // order. Ingest makes no judgment about which blocks are worth keeping
    // (see AGENT_TEXT_CAP's doc comment): that selection is Task 8's job.
    // Subagent prose is never pushed here at all: `merge_sessions` discards
    // it unconditionally (subagent prose is internal reasoning the human
    // never saw), so capturing it would only allocate strings that are
    // guaranteed to be thrown away.
    let mut agent_texts: Vec<AgentText> = Vec::new();
    // How many non-empty prose blocks were seen on a non-main lane and
    // therefore never pushed above. The count survives even though the
    // string does not, so a payload can disclose that a subagent's account
    // of its own work went unrecorded (see `Session::agent_texts_excluded`).
    let mut agent_texts_excluded = 0u64;

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

        if cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
        {
            cwd = Some(c.to_string());
        }

        // Auto-accept permission modes make approval latency meaningless.
        // `acceptEdits` and `bypassPermissions` both mean edits land without
        // a human decision. The running mode flips BOTH ways: a `mode` event
        // carrying `normal` (or `default`/`plan`) ends an auto-accept
        // stretch, so only the actions inside one are suppressed. A value
        // outside both lists leaves the running mode alone rather than
        // guessing what an unknown future mode means.
        let mode = v
            .get("permissionMode")
            .or_else(|| v.get("mode"))
            .and_then(Value::as_str);
        match mode {
            Some("acceptEdits") | Some("bypassPermissions") => {
                mode_is_auto = true;
                auto_accept = true; // session-level "ever seen", unchanged
            }
            Some("normal") | Some("default") | Some("plan") => {
                mode_is_auto = false;
            }
            _ => {}
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
                // A bare `ingest_str` call parses one transcript in isolation
                // and has no idea what index it will end up at inside a
                // work unit, so this is always `0` here; the merge step
                // stamps the real value later (see `UserText::session_ix`).
                session_ix: 0,
                // `origin.kind` distinguishes a real human turn from a
                // harness-injected one, or from a line where the field is
                // simply absent (a transcript older than the field, or an
                // interrupt/slash-command echo that never carries one).
                // Exactly "human" maps to Human; any other present value is
                // known non-human evidence; an absent `origin` is Unknown,
                // not assumed human. See `TurnOrigin` for why the two
                // consumers of this need different defaults for "unknown".
                origin: match v
                    .get("origin")
                    .and_then(|o| o.get("kind"))
                    .and_then(Value::as_str)
                {
                    Some("human") => TurnOrigin::Human,
                    Some(_) => TurnOrigin::NonHuman,
                    None => TurnOrigin::Unknown,
                },
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

                        // A recorded human decision. One AskUserQuestion call
                        // can carry several questions, and each becomes its
                        // own Decision so the payload can cite them
                        // separately.
                        if name == "AskUserQuestion"
                            && let Some(qs) = input
                                .and_then(|i| i.get("questions"))
                                .and_then(Value::as_array)
                        {
                            // The transcript's own answers map (read from the
                            // paired result line) is keyed by question text
                            // WITHIN one call. If this call asks the same
                            // text twice, that map has room for only one
                            // entry, so there is no way to tell which
                            // decision the recorded answer belongs to.
                            // Detect that collision here, at the source, so
                            // the join below can refuse to guess instead of
                            // handing both decisions the same answer.
                            let mut text_counts: HashMap<&str, usize> = HashMap::new();
                            for q in qs {
                                if let Some(text) = q.get("question").and_then(Value::as_str) {
                                    *text_counts.entry(text).or_insert(0) += 1;
                                }
                            }
                            for q in qs {
                                let Some(question) = q.get("question").and_then(Value::as_str)
                                else {
                                    continue; // malformed entry is data, not an error
                                };
                                let options = q
                                    .get("options")
                                    .and_then(Value::as_array)
                                    .map(|os| {
                                        os.iter()
                                            .filter_map(|o| o.get("label").and_then(Value::as_str))
                                            .map(str::to_string)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                let ambiguous_in_call =
                                    text_counts.get(question).copied().unwrap_or(0) > 1;
                                pending_decisions.push((
                                    tool_id.map(str::to_string),
                                    ambiguous_in_call,
                                    Decision {
                                        question: question.to_string(),
                                        options,
                                        answer: None, // filled by the join below
                                        line_no,
                                        session_ix: 0,
                                    },
                                ));
                            }
                        }

                        // Task lifecycle. TaskCreate always starts a task at
                        // pending; TaskUpdate is a lifecycle event only when
                        // it carries a status (it is also used for renames
                        // and dependency edits, which are not evidence of
                        // anything unfinished). Neither becomes a `TaskEvent`
                        // yet: a create needs its result for the real id, and
                        // an update needs its result to confirm the status
                        // actually took. Both are staged for the join below.
                        if name == "TaskCreate" {
                            pending_task_events.push(PendingTaskEvent::Create {
                                tool_use_id: tool_id.map(str::to_string),
                                subject: input
                                    .and_then(|i| i.get("subject"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                line_no,
                                lane: default_lane.clone(),
                            });
                        } else if name == "TaskUpdate"
                            && let Some(status) = input
                                .and_then(|i| i.get("status"))
                                .and_then(Value::as_str)
                        {
                            pending_task_events.push(PendingTaskEvent::Update {
                                tool_use_id: tool_id.map(str::to_string),
                                task_id: input
                                    .and_then(|i| i.get("taskId"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string(),
                                requested_status: status.to_string(),
                                subject: input
                                    .and_then(|i| i.get("subject"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                line_no,
                                lane: default_lane.clone(),
                            });
                        }

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
                            auto_accept_here: mode_is_auto,
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
                            // The answers map is keyed by question text
                            // WITHIN this one call, so it is stored under
                            // this result's tool_use_id (`id`, bound above)
                            // and joined to the pending decisions by that id
                            // plus question text below. Keying by question
                            // text alone (the old behaviour) let a second
                            // call asking the same text overwrite the
                            // first's answer in a session-wide map.
                            if let Some(answers) = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("answers"))
                                .and_then(Value::as_object)
                            {
                                let per_call = decision_answers.entry(id.to_string()).or_default();
                                for (q, a) in answers {
                                    if let Some(text) = a.as_str() {
                                        per_call.insert(q.clone(), text.to_string());
                                    }
                                }
                            }
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
                            // A TaskCreate result's real, harness-assigned id
                            // (measured present in 78/78 real cases): the
                            // only source of truth for `TaskEvent::id`, since
                            // the id is otherwise reported only as free text.
                            let task_id = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("task"))
                                .and_then(|t| t.get("id"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            // A TaskUpdate result's confirmation. `success`
                            // gates whether the update happened at all;
                            // `statusChange` is present in 120/121 measured
                            // cases and, when present, is the confirmed
                            // resulting status (preferred over the requested
                            // one). The one older case without it still
                            // carries `success`, which the join below falls
                            // back on.
                            let task_update_success = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("success"))
                                .and_then(Value::as_bool);
                            let task_status_change = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("statusChange"))
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
                                    task_id,
                                    task_update_success,
                                    task_status_change,
                                },
                            );
                        }
                    }
                    Some("text")
                        if v.get("type").and_then(Value::as_str) == Some("assistant") =>
                    {
                        // Whitespace-only blocks carry nothing and would only
                        // pad the list Task 8 selects from. `str::trim` uses
                        // Unicode `White_Space`, not just ASCII, so a block of
                        // e.g. U+3000 (ideographic space) is caught here too.
                        //
                        // Trim BEFORE capping, not after: a block can carry
                        // more than AGENT_TEXT_CAP leading whitespace
                        // characters followed by real text. Capping the raw
                        // string first would keep only whitespace (the real
                        // text sits past the cut), passing this check on the
                        // full string while storing a blank block, exactly
                        // the kind of blank that could later stand in as the
                        // "last block before a human turn" and displace a
                        // real claim. Trimming first means what gets stored
                        // is what was validated as non-empty.
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            let trimmed = t.trim();
                            if !trimmed.is_empty() {
                                // Subagent prose (non-main lane) is never kept:
                                // `merge_sessions` drops every subagent
                                // AgentText unconditionally, so pushing it here
                                // would allocate a string that is always
                                // thrown away. Its existence is still counted
                                // (see `agent_texts_excluded`'s declaration)
                                // so the exclusion is disclosed, not hidden.
                                if default_lane == Lane::Main {
                                    agent_texts.push(AgentText {
                                        // `chars().take()` not `[..n]`: slicing
                                        // a String by byte index panics if it
                                        // lands mid-character, and prose is
                                        // full of multi-byte characters.
                                        text: trimmed.chars().take(AGENT_TEXT_CAP).collect(),
                                        line_no,
                                        session_ix: 0,
                                    });
                                } else {
                                    agent_texts_excluded += 1;
                                }
                            }
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
                // A bare `ingest_str` call parses one transcript in
                // isolation, so every action it produces belongs to "the
                // only transcript there is": index 0. Task 6 (assembly) is
                // what rewrites this when several transcripts get merged.
                session_ix: 0,
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
                auto_accept_here: p.auto_accept_here,
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

    // Join the two halves, scoped by the asking call's tool_use_id. An
    // unanswered question keeps `answer: None`: the options it offered are
    // still evidence of what was under consideration. A question that
    // shares its exact text with another question in the SAME call also
    // keeps `answer: None`: the call's own answers map cannot tell the two
    // apart, so reporting a specific answer there would be a guess dressed
    // up as quoted ground truth, exactly the fabrication this join exists
    // to prevent.
    let decisions: Vec<Decision> = pending_decisions
        .into_iter()
        .map(|(tool_use_id, ambiguous_in_call, mut d)| {
            if !ambiguous_in_call {
                d.answer = tool_use_id
                    .as_ref()
                    .and_then(|id| decision_answers.get(id))
                    .and_then(|per_call| per_call.get(&d.question))
                    .cloned();
            }
            d
        })
        .collect();

    // Resolve task creates and updates against their paired results, in the
    // order they were seen (see `pending_task_events`'s declaration for why
    // order is preserved through this join). `filter_map` drops an entry
    // rather than inventing one: an interrupted session can leave a create
    // or update with no result line at all, and that is exactly the case
    // where fabricating a status would be worst.
    let task_events: Vec<TaskEvent> = pending_task_events
        .into_iter()
        .filter_map(|p| match p {
            PendingTaskEvent::Create {
                tool_use_id,
                subject,
                line_no,
                lane,
            } => {
                // No paired result, or a result that never reports the
                // harness-assigned id: nothing to trust as this task's
                // identity, so no event (FIX1's "interrupted session" case).
                let id = tool_use_id
                    .as_deref()
                    .and_then(|id| results.get(id))
                    .and_then(|r| r.task_id.clone())?;
                Some(TaskEvent {
                    id,
                    subject,
                    status: "pending".to_string(),
                    line_no,
                    session_ix: 0,
                    lane,
                })
            }
            PendingTaskEvent::Update {
                tool_use_id,
                task_id,
                requested_status,
                subject,
                line_no,
                lane,
            } => {
                let r = tool_use_id.as_deref().and_then(|id| results.get(id))?;
                // Anything other than an explicit `success: true` is treated
                // as unconfirmed: a failure, or a result that omits the key
                // entirely, is not evidence the transition happened.
                if r.task_update_success != Some(true) {
                    return None;
                }
                let status = r
                    .task_status_change
                    .clone()
                    .unwrap_or(requested_status);
                Some(TaskEvent {
                    id: task_id,
                    subject,
                    status,
                    line_no,
                    session_ix: 0,
                    lane,
                })
            }
        })
        .collect();

    Session {
        actions,
        user_texts,
        cwd,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines: untimestamped,
        interrupts,
        auto_accept,
        spawns,
        decisions,
        task_events,
        agent_texts,
        agent_texts_excluded,
        // A single `ingest_str` call has no idea what its own transcript id
        // is (that lives outside the raw JSONL text it was handed), so it
        // cannot fill this in. It leaves the table empty; the merge/assembly
        // step (a later task) is the one place that knows every transcript's
        // real id and stamps this table in.
        session_ids: Vec::new(),
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
    auto_accept_here: bool,
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
    /// A TaskCreate result's harness-assigned id (`toolUseResult.task.id`).
    task_id: Option<String>,
    /// A TaskUpdate result's `success` flag.
    task_update_success: Option<bool>,
    /// A TaskUpdate result's confirmed status (`toolUseResult.statusChange`),
    /// when the result shape carries one.
    task_status_change: Option<String>,
}

/// A `TaskCreate` or `TaskUpdate` call, staged until its paired result comes
/// back. Kept as one enum (not two separate `Vec`s) so the join below can
/// walk both in the single order they were seen in the transcript, matching
/// `Session::task_events`'s "in source order" contract.
enum PendingTaskEvent {
    /// The call that starts a task. Its real id is not known yet: the
    /// harness reports it only in the paired result's `task.id`.
    Create {
        tool_use_id: Option<String>,
        subject: Option<String>,
        line_no: usize,
        lane: Lane,
    },
    /// A status-carrying call. `task_id` comes straight from the call's own
    /// `taskId` input (that part was never in question); `requested_status`
    /// is what it asked for, which the join below only trusts once the
    /// paired result confirms it.
    Update {
        tool_use_id: Option<String>,
        task_id: String,
        requested_status: String,
        subject: Option<String>,
        line_no: usize,
        lane: Lane,
    },
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

    #[test]
    fn cwd_is_captured_from_first_line_carrying_one() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","cwd":"/work/proj","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","cwd":"/other","message":{"content":[{"type":"tool_use","id":"2","name":"Read","input":{"file_path":"/b.ts"}}]}}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.cwd.as_deref(), Some("/work/proj"), "first cwd wins");
        let none = ingest_str(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[]}}"#,
            Lane::Main,
        );
        assert_eq!(none.cwd, None, "no cwd line -> None");
    }

    #[test]
    fn mode_events_set_auto_accept_per_action_not_per_session() {
        // A real session emits `mode` events repeatedly (96 times in one
        // observed session), so the permission mode CHANGES mid-session.
        // Suppressing latency heuristics for the whole session because the
        // mode was once auto-accept throws away the parts that were not.
        let raw = concat!(
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"mode","mode":"acceptEdits","sessionId":"s"}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/a.rs","new_string":"y"}}]}}"#,
            "\n"
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.actions.len(), 2);
        assert!(
            !s.actions[0].auto_accept_here,
            "first edit ran under normal mode"
        );
        assert!(
            s.actions[1].auto_accept_here,
            "second ran under acceptEdits"
        );
        assert!(
            s.auto_accept,
            "the session-level flag still reports 'ever seen'"
        );
    }

    #[test]
    fn a_task_notification_is_not_a_human_turn() {
        // Review burden counts lines written between substantive HUMAN turns.
        // A task notification is injected by the harness, and counting it as a
        // human turn truncates the window and understates the metric.
        let raw = concat!(
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","origin":{"kind":"human"},"message":{"content":"do the thing"}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>done</task-notification>"}}"#,
            "\n"
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.user_texts.len(), 2, "both are recorded");
        // `.is_human()` is the "Human or Unknown" method (review-burden's
        // sense); a task notification is the one case that fails it because
        // it is KNOWN non-human, not merely unattributed.
        assert!(s.user_texts[0].is_human());
        assert!(!s.user_texts[1].is_human());
        assert_eq!(s.user_texts[1].origin, crate::model::TurnOrigin::NonHuman);
    }

    #[test]
    fn an_absent_origin_defaults_to_human() {
        // Transcripts written before the `origin` field existed must keep the
        // old, WIDER review window: defaulting unknown to human (via
        // `.is_human()`) means those turns still draw segment boundaries
        // exactly as they always did.
        let raw =
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":"hi"}}"#;
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.user_texts.len(), 1);
        assert!(s.user_texts[0].is_human(), "no origin field means human");
    }

    #[test]
    fn an_absent_origin_is_unknown_not_explicitly_human() {
        // DEFECT 1 regression: `.is_human()` folds Unknown into "human" for
        // the review-burden consumer (see the test above), but the raw
        // `origin` must stay Unknown, not Human. `context::scope()` reads
        // this field directly, and it must be able to tell "the human said
        // this" apart from "we don't know who said this" so it never quotes
        // the latter as intent.
        let raw =
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":"hi"}}"#;
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.user_texts[0].origin, crate::model::TurnOrigin::Unknown);
    }

    #[test]
    fn ask_user_question_is_captured_with_its_answer() {
        // A recorded decision is the highest-value item in the review payload:
        // it is the human's explicit choice AND the options it beat. Both halves
        // live on different lines (the call, then the result), joined by
        // tool_use id, exactly like structuredPatch already is.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","header":"Store","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        assert_eq!(s.decisions.len(), 1, "one recorded decision");
        let d = &s.decisions[0];
        assert_eq!(d.question, "Which store?");
        assert_eq!(d.options, vec!["SQLite".to_string(), "JSONL".to_string()]);
        assert_eq!(d.answer.as_deref(), Some("JSONL"));
    }

    #[test]
    fn an_other_answer_is_kept_verbatim_not_matched_to_an_option() {
        // The user can answer "Other" with free text that matches no option.
        // Quoting is the whole point of this design, so the free text must
        // survive intact rather than being dropped for failing to match.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"neither, keep it in memory"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        assert_eq!(
            s.decisions[0].answer.as_deref(),
            Some("neither, keep it in memory")
        );
    }

    #[test]
    fn an_unanswered_question_is_still_recorded() {
        // An interrupted session can leave a question with no result line. The
        // question and its options are still evidence of what was under
        // consideration, so the entry is kept with answer: None rather than
        // dropped.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"}]}]}}]}}"#;
        let s = ingest_str(call, Lane::Main);

        assert_eq!(s.decisions.len(), 1);
        assert_eq!(s.decisions[0].answer, None);
    }

    #[test]
    fn same_question_text_twice_in_one_session_gets_its_own_answer_each_time() {
        // Regression for a fabrication bug: decision_answers used to be keyed
        // by question text ALONE across the whole session, so a second call
        // asking the identically-worded question overwrote the first call's
        // answer in the shared map, and BOTH decisions reported the later
        // answer. The join must be scoped by the asking call's tool_use_id
        // so each call's answer stays its own.
        let call1 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result1 = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let call2 = r#"{"type":"assistant","timestamp":"2026-01-01T00:10:00Z","message":{"content":[{"type":"tool_use","id":"q2","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result2 = r#"{"type":"user","timestamp":"2026-01-01T00:10:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q2"}]},"toolUseResult":{"answers":{"Which store?":"SQLite"}}}"#;
        let raw = format!("{call1}\n{result1}\n{call2}\n{result2}");
        let s = ingest_str(&raw, Lane::Main);

        assert_eq!(s.decisions.len(), 2, "both decisions recorded");
        assert_eq!(
            s.decisions[0].answer.as_deref(),
            Some("JSONL"),
            "first call's own answer, not the second call's"
        );
        assert_eq!(
            s.decisions[1].answer.as_deref(),
            Some("SQLite"),
            "second call's own answer, not the first call's"
        );
    }

    #[test]
    fn task_events_record_creation_and_final_status() {
        // Unfinished work is invisible in a diff: a task created and never
        // completed means the commit is partial, and no reviewer can tell that
        // from the code.
        let create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Wire the cache"}}]}}"#;
        let create_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]},"toolUseResult":{"task":{"id":"1","subject":"Wire the cache"}}}"#;
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"}}]}}"#;
        let update_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:11Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2"}]},"toolUseResult":{"success":true,"taskId":"1","statusChange":"in_progress"}}"#;
        let s = ingest_str(
            &format!("{create}\n{create_result}\n{update}\n{update_result}"),
            Lane::Main,
        );

        assert_eq!(s.task_events.len(), 2);
        assert_eq!(s.task_events[0].subject.as_deref(), Some("Wire the cache"));
        assert_eq!(s.task_events[0].status, "pending", "a create starts pending");
        assert_eq!(s.task_events[0].id, "1", "id comes from the result");
        assert_eq!(s.task_events[1].id, "1");
        assert_eq!(s.task_events[1].status, "in_progress");
    }

    #[test]
    fn a_task_update_without_a_status_is_ignored() {
        // TaskUpdate also renames and reassigns. Only status transitions are
        // lifecycle events; a rename is not evidence of anything unfinished.
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","subject":"Renamed"}}]}}"#;
        let s = ingest_str(update, Lane::Main);

        assert!(s.task_events.is_empty());
    }

    #[test]
    fn a_task_create_takes_its_id_from_the_result_not_appearance_order() {
        // The old code numbered creates 1..N in the order they appeared,
        // assuming the harness numbers them identically. Real transcripts
        // don't guarantee that (a task from an earlier, since-cleared list
        // can leave the counter ahead), so the first create appearing here
        // deliberately reports an id that is NOT "1".
        let create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Commit untracked VGGT scripts"}}]}}"#;
        let create_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]},"toolUseResult":{"task":{"id":"7","subject":"Commit untracked VGGT scripts"}}}"#;
        let s = ingest_str(&format!("{create}\n{create_result}"), Lane::Main);

        assert_eq!(s.task_events.len(), 1);
        assert_eq!(
            s.task_events[0].id, "7",
            "the reported id, not the 1st-appearance position"
        );
    }

    #[test]
    fn a_task_create_with_no_paired_result_produces_no_event() {
        // An interrupted session can leave a TaskCreate with no result line.
        // With no result there is no real id to report, and inventing one
        // (the old synthetic counter's behaviour) would be a fabrication.
        let create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Wire the cache"}}]}}"#;
        let s = ingest_str(create, Lane::Main);

        assert!(s.task_events.is_empty());
    }

    #[test]
    fn a_task_update_whose_result_reports_failure_produces_no_event() {
        // A failed update that requested "completed" must not be recorded as
        // completed: that would hide genuinely unfinished work in exactly
        // the block whose value is reporting it honestly.
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let update_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:11Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2"}]},"toolUseResult":{"success":false,"taskId":"1"}}"#;
        let s = ingest_str(&format!("{update}\n{update_result}"), Lane::Main);

        assert!(s.task_events.is_empty());
    }

    #[test]
    fn a_task_update_result_with_status_change_uses_the_confirmed_status() {
        // The result's `statusChange` is authoritative over the requested
        // status: here the update asks for "completed" but the result
        // confirms only "blocked" (e.g. an unmet dependency), and the
        // recorded event must reflect what actually happened.
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let update_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:11Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2"}]},"toolUseResult":{"success":true,"taskId":"1","statusChange":"blocked"}}"#;
        let s = ingest_str(&format!("{update}\n{update_result}"), Lane::Main);

        assert_eq!(s.task_events.len(), 1);
        assert_eq!(
            s.task_events[0].status, "blocked",
            "the confirmed status, not the requested one"
        );
    }

    #[test]
    fn a_task_update_result_without_status_change_falls_back_to_requested_status() {
        // One measured real case carried `success` but no `statusChange`
        // (an older result shape). `success: true` with nothing to override
        // it is still a confirmation, so the requested status stands.
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"}}]}}"#;
        let update_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:11Z","message":{"content":[{"type":"tool_result","tool_use_id":"t2"}]},"toolUseResult":{"success":true,"taskId":"1"}}"#;
        let s = ingest_str(&format!("{update}\n{update_result}"), Lane::Main);

        assert_eq!(s.task_events.len(), 1);
        assert_eq!(s.task_events[0].status, "in_progress");
    }

    #[test]
    fn every_non_empty_agent_prose_block_is_captured() {
        // No length filter here on purpose: selecting which prose is a claim is
        // Task 8's job, using the spec's rule (last block before a human turn).
        // Measurement showed length is a bad proxy, so ingest makes no judgment
        // and keeps everything with text in it.
        let long = "x".repeat(100);
        let line = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{long}"}},{{"type":"text","text":"ok"}},{{"type":"text","text":"   "}}]}}}}"#
        );
        let s = ingest_str(&line, Lane::Main);

        assert_eq!(s.agent_texts.len(), 2, "both blocks with text, not the blank");
        assert_eq!(s.agent_texts[0].text.chars().count(), 100);
        assert_eq!(s.agent_texts[1].text, "ok");
    }

    #[test]
    fn a_runaway_prose_block_is_capped() {
        use crate::ingest::AGENT_TEXT_CAP;
        // One block must not be able to dominate memory. The cap is on the
        // stored copy only; nothing downstream counts characters for a metric,
        // so truncation here cannot skew a number the way EDIT_CAP would have.
        let huge = "y".repeat(AGENT_TEXT_CAP + 500);
        let line = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{huge}"}}]}}}}"#
        );
        let s = ingest_str(&line, Lane::Main);

        assert_eq!(s.agent_texts[0].text.chars().count(), AGENT_TEXT_CAP);
    }

    #[test]
    fn a_subagent_lanes_prose_is_not_retained_but_is_counted_as_excluded() {
        // merge_sessions drops every subagent AgentText unconditionally, so
        // keeping it here at ingest time would only allocate a string that is
        // always thrown away. Its existence must still be disclosed: the
        // count survives even though the text does not.
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"text","text":"I refactored the parser to handle the new shape."}]}}"#;
        let s = ingest_str(line, Lane::Sub("x".into()));

        assert!(
            s.agent_texts.is_empty(),
            "subagent prose must not be retained"
        );
        assert_eq!(
            s.agent_texts_excluded, 1,
            "but its existence must be counted"
        );
    }

    #[test]
    fn leading_whitespace_past_the_cap_still_stores_the_real_text() {
        // A block can carry more leading whitespace than AGENT_TEXT_CAP,
        // followed by real prose. Capping the raw string BEFORE trimming
        // would keep only whitespace (the real text sits past the cut),
        // which would pass the old "is this blank" check on the full string
        // while storing a blank block. Trimming first means what's stored is
        // what was validated as non-empty.
        let padding = " ".repeat(AGENT_TEXT_CAP + 500);
        let text = format!("{padding}real claim here");
        let line = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
        );
        let s = ingest_str(&line, Lane::Main);

        assert_eq!(s.agent_texts.len(), 1);
        assert_eq!(
            s.agent_texts[0].text, "real claim here",
            "the trimmed real text must be stored, not a capped run of blanks"
        );
    }

    #[test]
    fn unicode_whitespace_only_block_is_still_skipped() {
        // U+3000 (ideographic space) is Unicode whitespace, not ASCII, so an
        // ASCII-only blank check would wrongly keep this block. `str::trim`
        // uses Unicode `White_Space`, which does cover it.
        let line = "{\"type\":\"assistant\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"\u{3000}\u{3000}\u{3000}\"}]}}";
        let s = ingest_str(line, Lane::Main);

        assert!(
            s.agent_texts.is_empty(),
            "Unicode-whitespace-only block must be skipped, not stored"
        );
        assert_eq!(
            s.agent_texts_excluded, 0,
            "skipped on the main lane, not excluded (exclusion is a non-main-lane count)"
        );
    }

    #[test]
    fn two_identical_questions_in_one_call_both_resolve_to_none() {
        // One AskUserQuestion call can carry several questions. If two of
        // them share identical text, the transcript's own `answers` map
        // (keyed by question text) has room for only one entry, so there is
        // no way to tell which decision it belongs to. Guessing would hand
        // both decisions the same fabricated answer, exactly the failure
        // mode this fix exists to prevent, so both must stay `None`.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"}]},{"question":"Which store?","options":[{"label":"JSONL"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        assert_eq!(s.decisions.len(), 2, "both questions still recorded");
        assert_eq!(
            s.decisions[0].answer, None,
            "ambiguous within the call: no guessing"
        );
        assert_eq!(
            s.decisions[1].answer, None,
            "ambiguous within the call: no guessing"
        );
    }
}
