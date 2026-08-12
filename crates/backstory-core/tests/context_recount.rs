//! An independent, deliberately naive recount of the review-context blocks.
//!
//! Precedent: `scripts/recount.py` caught a 3x undercount that 271 green
//! tests missed, because every fixture shared the production blind spot.
//! Extraction gets the same insurance: a second implementation, re-parsing
//! the raw JSON by hand, whose counts must agree exactly with the production
//! extractor's on every real transcript in the corpus.
//!
//! What this file actually proves is not uniform, and an earlier version of
//! this header claimed more than it should have ("shares NO code with the
//! production path"). Read literally that is true of every function here,
//! but it is rhetorically misleading: some of what agreement demonstrates is
//! genuine independent correctness, and some of it is only consistency.
//! Split explicitly into the two tiers an adversarial review asked for:
//!
//! ## Tier 1: independently derived ALGORITHMS
//!
//! The claims windowing, the task-lifecycle replay, and the failing-commands
//! replay below are each re-derived from the RULE STATEMENT in `context.rs`,
//! not transliterated from its code. This is verifiable, not just claimed:
//! the task replay's `or_insert` (this file) genuinely differs from
//! production's overwrite-on-update, so the two would actually DISAGREE on a
//! transcript where a task's create record arrives after an update to the
//! same id: that disagreement never firing across 627 real transcripts is
//! real evidence, because a bug in either side's algorithm had a concrete,
//! checkable way to surface. For this tier, exact agreement across the whole
//! corpus is evidence of CORRECTNESS, not just of consistent application.
//!
//! ## Tier 2: mirrored DEFINITIONAL CONSTANTS
//!
//! `is_harness_notice`, `extract_user_text`, `origin_of`, and
//! `is_validation_naive` below are not re-derivations: each is the same
//! marker set, needle list, or shape re-typed by hand from the production
//! function it mirrors (their own doc comments say so). So is
//! `seen_tool_use_ids`'s tool_use-id dedup, mirroring the one rule the
//! original header named explicitly. For this tier, agreement proves only
//! that BOTH sides applied the same definition consistently; it cannot
//! catch a definition that is wrong on both sides at once. If a future
//! harness-notice wording is missing from the three markers here, or an
//! eighth validation needle turns out to need a ninth, this gate is
//! structurally blind to it: both sides would miss it identically and still
//! agree. Correctness for this tier rests on the ingest-side measurement
//! that picked the definition in the first place (the 42-record marker
//! survey behind `is_harness_notice`, cited in `ingest.rs`), not on anything
//! this file can independently check.
//!
//! This file was also rewritten because the original brief encoded rules
//! that no longer exist: an 80-character floor on claims/prose (measured and
//! rejected; see `context::claims`'s doc comment) and a global, un-scoped
//! prose/boundary walk (the real rule is scoped per transcript, since
//! `line_no` is only meaningful within one transcript's own file). Both are
//! reimplemented below against what `context.rs` and `ingest.rs` actually do
//! now, read directly from those files, not from the brief.

use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

// ---------------------------------------------------------------------
// Shared micro-helpers. Each is a direct reading of one documented rule
// from ingest.rs/context.rs, re-typed here rather than called from either
// file. Most of the ones below are Tier 2 (mirrored constants: see the
// module doc) -- `is_harness_notice`, `extract_user_text`, `origin_of`, and
// `is_validation_naive` are named there explicitly.
// ---------------------------------------------------------------------

/// Mirrors `ingest.rs`'s harness-notice check: a record Claude Code injects
/// (dropped connection, spend limit, overload, "no response requested")
/// that carries an ordinary-looking `text` block but is not agent prose.
fn is_harness_notice(v: &Value) -> bool {
    v.get("message")
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str)
        == Some("<synthetic>")
        || v.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
        || v.get("error").is_some()
}

/// Mirrors `ingest.rs::extract_user_text`: a plain non-empty string, or an
/// array of blocks whose `text` fields join to something non-empty. A
/// tool-result-only content array (no `text` fields anywhere) yields `None`.
fn extract_user_text(v: &Value) -> Option<String> {
    let content = v.get("message")?.get("content")?;
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

/// Mirrors `TurnOrigin`: exactly `origin.kind == "human"` is Human, any
/// other present kind is NonHuman, and a missing `origin` (or a missing
/// `kind` on it) is Unknown.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Origin {
    Human,
    NonHuman,
    Unknown,
}

fn origin_of(v: &Value) -> Origin {
    match v
        .get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(Value::as_str)
    {
        Some("human") => Origin::Human,
        Some(_) => Origin::NonHuman,
        None => Origin::Unknown,
    }
}

/// Mirrors `signals/failures.rs::is_validation`: a case-insensitive
/// substring match against the same eight needles. Re-encoded here (the
/// production function is `pub(crate)` and unreachable from this crate
/// anyway) rather than imported, per this file's whole reason to exist.
fn is_validation_naive(cmd: &str) -> bool {
    const NEEDLES: [&str; 8] = [
        "test",
        "lint",
        "build",
        "tsc",
        "typecheck",
        "cargo check",
        "pytest",
        "clippy",
    ];
    let c = cmd.to_lowercase();
    NEEDLES.iter().any(|n| c.contains(n))
}

/// What a `tool_result` line reported, keyed by the `tool_use_id` it answers.
#[derive(Default, Clone)]
struct ResultInfo {
    is_error: bool,
    task_id: Option<String>,
    task_update_success: Option<bool>,
    task_status_change: Option<String>,
}

/// One `TaskCreate`/`TaskUpdate` call, staged in file order until resolved
/// against its paired result. Kept as one enum in one `Vec`, exactly the
/// shape `context::incomplete`'s doc comment says `Session::task_events`
/// needs ("in source order"), so this side's replay walks events in the same
/// order production does.
enum PendingTask {
    Create {
        tool_use_id: Option<String>,
    },
    Update {
        tool_use_id: Option<String>,
        task_id: String,
        requested_status: String,
    },
}

/// The four counts this test compares against `context::{decisions, claims,
/// incomplete}`. Everything here is derived from one independent walk of the
/// raw JSON; nothing here calls into `backstory_core`.
struct NaiveCounts {
    decisions: usize,
    claims: usize,
    unfinished_tasks: usize,
    /// Sorted, deduped command strings whose LAST recorded invocation
    /// (ordered by effective timestamp, then line number, mirroring the
    /// ordering `Session::actions` is sorted into before `incomplete()` walks
    /// it) explicitly failed.
    failing_commands: Vec<String>,
}

/// Walk one transcript's raw JSONL text and compute all four counts by hand.
///
/// This is a SINGLE independent pass, deliberately not decomposed to mirror
/// `ingest_str`'s internal structure (staged halves, a join step, a replay):
/// decomposing it the same way would risk copying the production shape along
/// with the rule.
fn naive_counts(raw: &str) -> NaiveCounts {
    let mut seen_tool_use_ids: std::collections::HashSet<String> = Default::default();
    let mut results: HashMap<String, ResultInfo> = HashMap::new();
    let mut pending_tasks: Vec<PendingTask> = Vec::new();
    let mut decisions = 0usize;

    // For claims(): every non-empty, non-harness-notice assistant text
    // block, and every user-turn boundary whose origin is Human or Unknown
    // (mirrors `UserText::is_human`), each tagged with the line it came
    // from. A single transcript file is already "one transcript's own
    // scope" (`context::claims` partitions by `session_ix` specifically so a
    // merged work unit's several files don't cross-contaminate boundaries;
    // there is exactly one file here, so that partition is trivially this
    // whole vector).
    let mut prose_lines: Vec<usize> = Vec::new();
    let mut boundary_lines: Vec<usize> = Vec::new();

    // For failing_commands(): every validation Bash call, in the order
    // `Session::actions` would sort them into (effective timestamp, then
    // line number; lane is constant within one file, so it drops out of
    // the comparison).
    let mut bash_calls: Vec<(String, usize, Option<String>, String)> = Vec::new(); // (ts, line_no, tool_use_id, command)

    let mut last_ts = String::new();

    for (line_no, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue; // a bad line is skipped, exactly like ingest_str
        };

        // effective_ts: own timestamp, else carry the last one forward.
        // Needed only to order the bash replay the way `Session::actions`
        // (sorted before `incomplete()` walks it) is ordered.
        let effective_ts = match v.get("timestamp").and_then(Value::as_str) {
            Some(ts) => {
                last_ts = ts.to_string();
                last_ts.clone()
            }
            None => last_ts.clone(),
        };

        let is_notice = is_harness_notice(&v);
        let top_type = v.get("type").and_then(Value::as_str);

        // ---- user-turn boundary, for claims() ----
        if top_type == Some("user")
            && v.get("isMeta").and_then(Value::as_bool) != Some(true)
            && extract_user_text(&v).is_some()
            && origin_of(&v) != Origin::NonHuman
        {
            boundary_lines.push(line_no);
        }

        // ---- tool_use / tool_result blocks ----
        let Some(blocks) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };

        for b in blocks {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    let tool_id = b.get("id").and_then(Value::as_str);
                    // THE ONE MIRRORED PRODUCTION RULE: first occurrence of a
                    // tool_use id wins, a replay/streaming duplicate of the
                    // same call is skipped entirely (not just for one field:
                    // ingest.rs `continue`s before any per-tool logic runs on
                    // a duplicate, so this side does too).
                    if let Some(id) = tool_id
                        && !seen_tool_use_ids.insert(id.to_string())
                    {
                        continue;
                    }
                    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                    let input = b.get("input");

                    if name == "AskUserQuestion"
                        && let Some(qs) = input
                            .and_then(|i| i.get("questions"))
                            .and_then(Value::as_array)
                    {
                        for q in qs {
                            // Malformed entry (no "question" text) is data,
                            // not counted, same as ingest.rs's `else continue`.
                            if q.get("question").and_then(Value::as_str).is_some() {
                                decisions += 1;
                            }
                        }
                    } else if name == "TaskCreate" {
                        pending_tasks.push(PendingTask::Create {
                            tool_use_id: tool_id.map(str::to_string),
                        });
                    } else if name == "TaskUpdate"
                        && let Some(status) =
                            input.and_then(|i| i.get("status")).and_then(Value::as_str)
                    {
                        pending_tasks.push(PendingTask::Update {
                            tool_use_id: tool_id.map(str::to_string),
                            task_id: input
                                .and_then(|i| i.get("taskId"))
                                .and_then(Value::as_str)
                                .unwrap_or("?")
                                .to_string(),
                            requested_status: status.to_string(),
                        });
                    } else if name == "Bash"
                        && let Some(cmd) =
                            input.and_then(|i| i.get("command")).and_then(Value::as_str)
                        && is_validation_naive(cmd)
                    {
                        bash_calls.push((
                            effective_ts.clone(),
                            line_no,
                            tool_id.map(str::to_string),
                            cmd.to_string(),
                        ));
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = b.get("tool_use_id").and_then(Value::as_str) {
                        let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        let tur = v.get("toolUseResult");
                        results.insert(
                            id.to_string(),
                            ResultInfo {
                                is_error,
                                task_id: tur
                                    .and_then(|r| r.get("task"))
                                    .and_then(|t| t.get("id"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                task_update_success: tur
                                    .and_then(|r| r.get("success"))
                                    .and_then(Value::as_bool),
                                task_status_change: tur
                                    .and_then(|r| r.get("statusChange"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                            },
                        );
                    }
                }
                Some("text") if top_type == Some("assistant") => {
                    if let Some(t) = b.get("text").and_then(Value::as_str) {
                        let trimmed = t.trim();
                        if !trimmed.is_empty() && !is_notice {
                            prose_lines.push(line_no);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // ---- claims(): last prose block before each boundary, plus the
    // trailing block if the last boundary this transcript ever crossed had
    // no boundary after it (or there were no boundaries at all). Positional
    // windowing, independently derived from context::claims's own
    // description rather than copied from its loop. ----
    let mut claims = 0usize;
    let mut last_prose: Option<usize> = None;
    let mut next_boundary = 0usize;
    for &p in &prose_lines {
        while next_boundary < boundary_lines.len() && p > boundary_lines[next_boundary] {
            if last_prose.take().is_some() {
                claims += 1;
            }
            next_boundary += 1;
        }
        last_prose = Some(p);
    }
    if last_prose.is_some() {
        claims += 1;
    }

    // ---- incomplete().unfinished_tasks: replay creates/updates, in file
    // order, into a final status per task id (one file == one lane, so the
    // id alone is a safe replay key here). ----
    let mut final_status: BTreeMap<String, String> = BTreeMap::new();
    for p in pending_tasks {
        match p {
            PendingTask::Create { tool_use_id } => {
                // No paired result, or a result that never reported the
                // harness-assigned id: no trustworthy identity, no event.
                let Some(id) = tool_use_id
                    .as_deref()
                    .and_then(|id| results.get(id))
                    .and_then(|r| r.task_id.clone())
                else {
                    continue;
                };
                final_status
                    .entry(id)
                    .or_insert_with(|| "pending".to_string());
            }
            PendingTask::Update {
                tool_use_id,
                task_id,
                requested_status,
            } => {
                let Some(r) = tool_use_id.as_deref().and_then(|id| results.get(id)) else {
                    continue;
                };
                if r.task_update_success != Some(true) {
                    continue;
                }
                let status = r.task_status_change.clone().unwrap_or(requested_status);
                final_status.insert(task_id, status);
            }
        }
    }
    let unfinished_tasks = final_status
        .values()
        .filter(|s| s.as_str() != "completed" && s.as_str() != "deleted")
        .count();

    // ---- incomplete().failing_commands: last run wins per distinct
    // validation command string, ordered the way Session::actions is sorted
    // (effective_ts, then line_no) before incomplete() walks it. ----
    bash_calls.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    let mut last_run: BTreeMap<String, Option<bool>> = BTreeMap::new();
    for (_, _, tool_use_id, cmd) in &bash_calls {
        let is_error = tool_use_id
            .as_deref()
            .and_then(|id| results.get(id))
            .map(|r| r.is_error);
        last_run.insert(cmd.clone(), is_error);
    }
    let failing_commands: Vec<String> = last_run
        .into_iter()
        .filter(|(_, err)| *err == Some(true))
        .map(|(cmd, _)| cmd)
        .collect();

    NaiveCounts {
        decisions,
        claims,
        unfinished_tasks,
        failing_commands,
    }
}

/// Compare one transcript's naive counts against the production extractors,
/// panicking with a descriptive message on the first mismatch.
fn assert_agrees(label: &str, raw: &str) {
    let naive = naive_counts(raw);
    let s = backstory_core::ingest::ingest_str(raw, backstory_core::model::Lane::Main);

    assert_eq!(
        backstory_core::context::decisions(&s).len(),
        naive.decisions,
        "decision count disagrees in {label}"
    );
    assert_eq!(
        backstory_core::context::claims(&s).len(),
        naive.claims,
        "claim count disagrees in {label}"
    );
    let inc = backstory_core::context::incomplete(&s);
    assert_eq!(
        inc.unfinished_tasks.len(),
        naive.unfinished_tasks,
        "unfinished task count disagrees in {label}"
    );
    let mut product_failing: Vec<String> = inc
        .failing_commands
        .iter()
        .map(|f| f.command.clone())
        .collect();
    product_failing.sort();
    assert_eq!(
        product_failing, naive.failing_commands,
        "failing validation commands disagree in {label}"
    );
}

#[test]
fn extraction_agrees_with_an_independent_naive_count_on_fixtures() {
    for name in [
        "fixtures/demo/demo-session.jsonl",
        "fixtures/edge-cases.jsonl",
        "fixtures/session-2_1_210-subagents.jsonl",
    ] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(name);
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        assert_agrees(name, &raw);
    }
}

/// Same check, run against every transcript under `~/.claude/projects/`
/// instead of the committed fixtures. The fixtures were written alongside
/// the code they test and can share its blind spots; real transcripts were
/// not. `#[ignore]`d so CI stays hermetic and does not depend on a
/// developer's local `~/.claude` directory existing at all.
///
/// Run manually with: `cargo test -p backstory-core --test context_recount -- --ignored --nocapture`
#[test]
#[ignore]
fn extraction_agrees_with_an_independent_naive_count_on_real_transcripts() {
    let Some(home) = std::env::var_os("HOME") else {
        eprintln!("HOME not set, skipping real-transcript recount");
        return;
    };
    let root = Path::new(&home).join(".claude").join("projects");
    if !root.is_dir() {
        eprintln!(
            "{} does not exist, skipping real-transcript recount",
            root.display()
        );
        return;
    }

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(p);
            }
        }
    }
    files.sort();

    let mut checked = 0usize;
    let mut disagreements: Vec<String> = Vec::new();
    for path in &files {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        let naive = naive_counts(&raw);
        let s = backstory_core::ingest::ingest_str(&raw, backstory_core::model::Lane::Main);
        let label = path.display().to_string();

        let d = backstory_core::context::decisions(&s).len();
        if d != naive.decisions {
            disagreements.push(format!(
                "{label}: decisions naive={} product={d}",
                naive.decisions
            ));
        }
        let c = backstory_core::context::claims(&s).len();
        if c != naive.claims {
            disagreements.push(format!(
                "{label}: claims naive={} product={c}",
                naive.claims
            ));
        }
        let inc = backstory_core::context::incomplete(&s);
        if inc.unfinished_tasks.len() != naive.unfinished_tasks {
            disagreements.push(format!(
                "{label}: unfinished_tasks naive={} product={}",
                naive.unfinished_tasks,
                inc.unfinished_tasks.len()
            ));
        }
        let mut product_failing: Vec<String> = inc
            .failing_commands
            .iter()
            .map(|f| f.command.clone())
            .collect();
        product_failing.sort();
        if product_failing != naive.failing_commands {
            disagreements.push(format!(
                "{label}: failing_commands naive={:?} product={:?}",
                naive.failing_commands, product_failing
            ));
        }
        checked += 1;
    }

    println!(
        "recount: {checked} real transcript(s) checked under {}",
        root.display()
    );
    if !disagreements.is_empty() {
        println!("{} DISAGREEMENT(S):", disagreements.len());
        for d in &disagreements {
            println!("  {d}");
        }
    }
    assert!(
        disagreements.is_empty(),
        "{} disagreement(s) between the naive recount and the production extractor, see stdout (run with --nocapture)",
        disagreements.len()
    );
}
