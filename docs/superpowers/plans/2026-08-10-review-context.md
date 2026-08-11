# Review Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give a reviewing agent the recorded context of a coding session (what was asked, what was decided, what was tried and failed, what was left unfinished, what the agent claimed) verbatim and with citations, so it stops reporting things that are not problems.

**Architecture:** Three new `Session` vectors populated at ingest (`decisions`, `task_events`, `agent_texts`), a pure `context` module that turns them into five extraction structs, two new capped payloads, a tiny `git` module that maps a commit range to session ids, and two new MCP tools plus a CLI command. suMCP retrieves and cites; it never judges.

**Tech Stack:** Rust 2024 edition, `serde` + `serde_json` only, `git` invoked as a subprocess.

**Spec:** `docs/superpowers/specs/2026-08-10-review-context-design.md`

## Global Constraints

- **MSRV 1.88**, edition 2024, enforced by CI. Let-chains (`if let ... && let ...`) are used throughout and are the reason for the floor.
- **Zero new dependencies.** `sumcp-core` depends on `serde` and `serde_json` only (ADR A2: synchronous and pure, no async deps). `tempfile` is dev-only. Adding a crate is a plan violation, not a judgment call.
- **No I/O below `ingest`** (ADR A2). The `context` module is pure: `&Session -> struct`. The `git` module is the one exception and lives in the CLI/MCP boundary layer, never called from a signal.
- **Every payload is token-capped** via the existing `shrink_to_fit(cap, start, build)` helper in `payloads.rs`, and every capped payload sets `"truncated": true` when it dropped anything.
- **Every extracted item carries `idxs`** (action indices) or `line_no` so `evidence()` can dereference it. An item with no citation path is a plan violation.
- **`exact: false` requires a `note`.** The repo's `Finding` invariant. The `constraints` block is the only heuristic extraction and must be labelled.
- **No em dashes** in any prose, comment, doc, or commit message written by this plan.
- **Comment density matches the surrounding code.** This codebase explains *why* in prose comments aimed at a reader learning Rust. Match it; do not strip it down.
- **Payload version is `"v": 3`** for the two new payloads. The existing six stay at `2`.

---

## File Structure

**Created:**
- `crates/sumcp-core/src/context.rs`: the five pure extractions. One responsibility: turn a `Session` into review context structs. No JSON, no capping, no I/O.
- `crates/sumcp-core/src/git.rs`: commit range to timestamp window. One responsibility: shell out to `git log` and parse two timestamps. Nothing else.
- `scripts/power_estimate.py`: the pre-implementation gate (Task 1).
- `scripts/review_experiment.py`: the two-arm harness (Task 16).
- `crates/sumcp-core/tests/context_recount.rs`: the independent naive recounter gate.

**Modified:**
- `crates/sumcp-core/src/model.rs`: three new `Session` fields plus their structs.
- `crates/sumcp-core/src/ingest.rs`: populate the three new fields.
- `crates/sumcp-core/src/merge.rs`: carry the three new fields through both merges.
- `crates/sumcp-core/src/payloads.rs`: `review_context()` and `session_intent()`.
- `crates/sumcp-core/src/lib.rs`: declare `pub mod context;` and `pub mod git;`.
- `crates/sumcp-mcp/src/server.rs`: register two tools, dispatch them.
- `crates/sumcp-cli/src/main.rs`: the `context` subcommand.

**Why `context.rs` is separate from `payloads.rs`:** `payloads.rs` is already 1,637 lines and owns JSON shaping and token capping. Extraction is a different responsibility with different tests (structs in, structs out, no capping). Keeping them apart means the extraction tests never assert on JSON, and the payload tests never re-test extraction.

---

## Task 1: Power estimate (gate before any product code)

The spec requires this before implementation, because the project has already shipped one study whose intervals were too wide to conclude anything. If the answer is "you cannot run enough commits," the experiment design changes and Tasks 2 through 16 are built against a different validation plan.

**Files:**
- Create: `scripts/power_estimate.py`

**Interfaces:**
- Consumes: nothing (stdlib only, matches `scripts/validity_sweep.py` house style).
- Produces: a printed table of detectable effect sizes by sample size, and a go/no-go line. No code depends on it.

- [ ] **Step 1: Write the script**

```python
#!/usr/bin/env python3
"""Power estimate for the two-arm review-precision experiment (dev-only).

Question this answers BEFORE any code is written: how many commits must the
experiment cover to detect a plausible improvement in the invalid share of a
reviewer's findings?

Design being powered: each commit is reviewed twice (blind arm A,
contextualized arm B). Every finding is adjudicated valid or invalid. The
primary metric is the difference in invalid PROPORTION between arms. The unit
of analysis is the FINDING, not the commit, so total N is
(commits x findings per commit x 2 arms).

Baseline: 56.3% of agentic review comments are rejected (arXiv:2607.03316).
We treat that as arm A's invalid share, p_a = 0.563.

Method: normal approximation for the difference of two independent
proportions, two-sided, alpha = 0.05. n per arm for power (1-beta):

    n = (z_{1-alpha/2} + z_{1-beta})^2 * (p_a(1-p_a) + p_b(1-p_b)) / (p_a-p_b)^2

Stdlib only, no scipy: the two z values needed are hardcoded constants, which
is honest because alpha and power are fixed by the design and not swept.
"""

from __future__ import annotations

# Two-sided alpha = 0.05, and power = 0.80. Fixed by the design, not swept,
# so hardcoding them is a statement of the design rather than a shortcut.
Z_ALPHA_2 = 1.959963985
Z_POWER = 0.8416212336

P_A = 0.563  # arXiv:2607.03316 rejection rate, arm A's assumed invalid share


def n_per_arm(p_a: float, p_b: float) -> float:
    """Findings needed PER ARM to detect p_a - p_b at alpha=.05, power=.80."""
    if p_a == p_b:
        return float("inf")
    num = (Z_ALPHA_2 + Z_POWER) ** 2 * (p_a * (1 - p_a) + p_b * (1 - p_b))
    return num / (p_a - p_b) ** 2


def main() -> None:
    print(f"Baseline invalid share (arm A): {P_A:.3f}  [arXiv:2607.03316]")
    print("alpha=0.05 two-sided, power=0.80, unit of analysis = one finding\n")
    print(f"{'improvement':>12} {'arm B share':>12} {'findings/arm':>13} "
          f"{'commits @3':>11} {'commits @6':>11}")
    for delta in (0.05, 0.10, 0.15, 0.20, 0.25):
        p_b = P_A - delta
        n = n_per_arm(P_A, p_b)
        # Commits needed, assuming a typical review yields 3 or 6 findings.
        print(f"{delta:>11.0%} {p_b:>12.3f} {n:>13.0f} "
              f"{n / 3:>11.0f} {n / 6:>11.0f}")
    print("\nGO/NO-GO: compare the rightmost columns against the number of")
    print("commits you can realistically review TWICE. If the smallest")
    print("improvement worth caring about needs more commits than you can")
    print("run, the finding-level design cannot answer the question and the")
    print("experiment must be redesigned (e.g. paired per-finding adjudication")
    print("on the SAME findings, which is far more efficient) BEFORE coding.")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run it**

Run: `python3 scripts/power_estimate.py`
Expected: a five-row table. Sanity check the arithmetic by hand on one row: at `delta=0.10`, `p_b=0.463`, the numerator is `(1.96+0.842)^2 * (0.563*0.437 + 0.463*0.537) = 7.849 * (0.2460 + 0.2486) = 3.883`, divided by `0.01` gives roughly **388 findings per arm**, which is roughly **130 commits at 3 findings each**.

- [ ] **Step 3: Record the decision**

Append the result and the go/no-go call to the spec's "Sample size" section, replacing the current placeholder wording that a power estimate "is required". State the actual number and what was decided.

```bash
git add scripts/power_estimate.py docs/superpowers/specs/2026-08-10-review-context-design.md
git commit -m "test: power estimate for the review-precision experiment, run before any code

Records the detectable effect sizes and the go/no-go call the spec demanded
up front, so the sample-size problem is settled before implementation rather
than reinterpreted after results are in hand."
```

**STOP HERE and report the number to the user before starting Task 2.** If the required commit count exceeds what is realistically runnable, the experiment must be redesigned first (the most likely fix is paired adjudication of the same findings across arms, which needs far fewer commits than two independent samples).

---

## Task 2: Ingest captures recorded decisions

**Files:**
- Modify: `crates/sumcp-core/src/model.rs` (add `Decision`, add `Session.decisions`)
- Modify: `crates/sumcp-core/src/ingest.rs` (populate it)
- Modify: `crates/sumcp-core/src/merge.rs` (carry it through both merges)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `sumcp_core::model::Decision { question: String, options: Vec<String>, answer: Option<String>, line_no: usize, idx: Option<Idx>, session_ix: u16 }` and `Session::decisions: Vec<Decision>`. Tasks 6 and 15 read this.

**Transcript shape (verified against a real transcript, not guessed):**
- `tool_use` with `name == "AskUserQuestion"`, `input.questions[]`, each with `question`, `header`, `multiSelect`, `options[] { label, description }`.
- The paired answer is on the result line's top-level `toolUseResult.answers`, a map keyed by the **question text**, valued by the chosen option label **or free text when the user answered "Other"**.

- [ ] **Step 1: Write the failing test**

Add to `crates/sumcp-core/src/ingest.rs`'s `#[cfg(test)] mod tests`:

```rust
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

    assert_eq!(s.decisions[0].answer.as_deref(), Some("neither, keep it in memory"));
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core ask_user_question_is_captured_with_its_answer`
Expected: FAIL, `no field 'decisions' on type 'Session'`.

- [ ] **Step 3: Add the model types**

In `crates/sumcp-core/src/model.rs`, add above `pub struct Session`:

```rust
/// One question the agent put to the human, the options it offered, and what
/// the human picked.
///
/// WHY THIS IS ITS OWN VEC AND NOT A FIELD ON `Action`: a session has tens of
/// thousands of actions and a handful of decisions. Hanging the question and
/// option text off every action would cost memory on every one of them to
/// serve a few. This follows the same shape as `Session::spawns`, which is
/// also a small paired-with-its-result list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Decision {
    /// The question text, verbatim.
    pub question: String,
    /// The option labels offered, in the order presented.
    pub options: Vec<String>,
    /// What the human chose. `None` when the session ended before answering.
    /// This is NOT constrained to be one of `options`: the "Other" escape
    /// hatch lets the human answer in free text, and that text is the most
    /// informative answer of all, so it is kept exactly as written.
    pub answer: Option<String>,
    /// Source line of the asking call (total-order tiebreak, same key as
    /// actions and user texts).
    pub line_no: usize,
    /// The asking action's index, so `evidence()` can dereference it.
    /// `None` only if the call was deduped away as a replay.
    pub idx: Option<Idx>,
    /// Which transcript of the work unit this came from. See
    /// [`Action::session_ix`] for why this is an index and not a `String`.
    #[serde(default)]
    pub session_ix: u16,
}
```

Add to `pub struct Session`, after `pub spawns: Vec<Spawn>,`:

```rust
    /// Questions the agent put to the human, with the options offered and the
    /// answer given. The review-context payload's highest-value block: it is
    /// the only place a deliberate human choice is recorded, so a reviewer
    /// can stop flagging it as a mistake.
    ///
    /// `#[serde(default)]` so transcript caches written before this field
    /// existed still deserialize.
    #[serde(default)]
    pub decisions: Vec<Decision>,
```

- [ ] **Step 4: Populate it in ingest**

In `crates/sumcp-core/src/ingest.rs`, add near the other accumulators at the top of `ingest_str` (beside `let mut spawn_ids ...`):

```rust
    // Decisions arrive in two halves on different lines: the AskUserQuestion
    // call carries the question and options, the paired result carries the
    // answer. We stash the half we have and join by tool_use id at the end,
    // exactly the way spawns already resolve their agentId.
    let mut pending_decisions: Vec<(String, Decision)> = Vec::new();
    let mut decision_answers: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
```

In the `Some("tool_use")` arm, after the existing `if name == "Agent" || name == "Task" { ... }` block:

```rust
                        // A recorded human decision. One AskUserQuestion call
                        // can carry several questions, and each becomes its
                        // own Decision so the payload can cite them
                        // separately.
                        if name == "AskUserQuestion"
                            && let Some(qs) = input
                                .and_then(|i| i.get("questions"))
                                .and_then(Value::as_array)
                        {
                            for q in qs {
                                let Some(question) =
                                    q.get("question").and_then(Value::as_str)
                                else {
                                    continue; // malformed entry is data, not an error
                                };
                                let options = q
                                    .get("options")
                                    .and_then(Value::as_array)
                                    .map(|os| {
                                        os.iter()
                                            .filter_map(|o| {
                                                o.get("label").and_then(Value::as_str)
                                            })
                                            .map(str::to_string)
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                pending_decisions.push((
                                    question.to_string(),
                                    Decision {
                                        question: question.to_string(),
                                        options,
                                        answer: None, // filled by the join below
                                        line_no,
                                        // The asking action's Idx is assigned
                                        // when `pending` is drained, so this
                                        // is resolved in the join too.
                                        idx: None,
                                        session_ix: 0,
                                    },
                                ));
                            }
                        }
```

In the `Some("tool_result")` arm, after the existing `hunks` read:

```rust
                            // The answers map is keyed by the question text
                            // itself, so it joins to the pending decisions
                            // without needing the tool_use id.
                            if let Some(answers) = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("answers"))
                                .and_then(Value::as_object)
                            {
                                for (q, a) in answers {
                                    if let Some(text) = a.as_str() {
                                        decision_answers
                                            .insert(q.clone(), text.to_string());
                                    }
                                }
                            }
```

Just before the `Session { ... }` construction at the end of `ingest_str`:

```rust
    // Join the two halves. An unanswered question keeps `answer: None`: the
    // options it offered are still evidence of what was under consideration.
    let decisions: Vec<Decision> = pending_decisions
        .into_iter()
        .map(|(q, mut d)| {
            d.answer = decision_answers.get(&q).cloned();
            d
        })
        .collect();
```

Add `decisions,` to the `Session { ... }` literal, and add `Decision` to the `use crate::model::{...}` import list at the top of the file.

- [ ] **Step 5: Carry it through both merges**

In `crates/sumcp-core/src/merge.rs`, inside `merge_sessions` (line 15), beside `let spawns = main.spawns;`:

```rust
    // Main only: a subagent has no channel to ask the human anything, so a
    // decision can only ever appear in the main transcript. Same reasoning
    // that drops sub.user_texts.
    let decisions = main.decisions;
```

Add `decisions,` to that function's `Session { ... }` literal.

In `merge_work_unit` (line 98), beside `let mut spawns = Vec::new();`:

```rust
    let mut decisions = Vec::new();
```

and inside the per-part loop, next to the `user_texts` stamping:

```rust
        // Stamped like user_texts: a decision must be attributable to the
        // transcript it came from, or the payload cannot cite it correctly
        // in a multi-transcript work unit.
        for mut d in part.decisions {
            d.session_ix = ix;
            decisions.push(d);
        }
```

Add `decisions,` to that function's `Session { ... }` literal.

- [ ] **Step 6: Fix every other Session literal**

Run: `cargo build -p sumcp-core 2>&1 | grep "missing field"`
Expected: a list of test helpers and fixtures constructing `Session` literally. Add `decisions: vec![],` to each. There is no `Default` impl to lean on, and adding one now would hide future missing-field errors that are the compiler doing its job.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p sumcp-core`
Expected: PASS, including the three new tests.

- [ ] **Step 8: Commit**

```bash
git add crates/sumcp-core/src/model.rs crates/sumcp-core/src/ingest.rs crates/sumcp-core/src/merge.rs
git commit -m "feat: ingest captures recorded human decisions

AskUserQuestion carries the question and the options offered; the paired
result carries the answer, keyed by question text. Both halves are joined at
the end of ingest the way spawns already resolve their agentId.

An Other answer is free text matching no option, and is kept verbatim: it is
the most informative answer of all. An unanswered question is kept with
answer: None, because the options it offered still show what was under
consideration."
```

---

## Task 3: Ingest captures task lifecycle events

**Files:**
- Modify: `crates/sumcp-core/src/model.rs`
- Modify: `crates/sumcp-core/src/ingest.rs`
- Modify: `crates/sumcp-core/src/merge.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `sumcp_core::model::TaskEvent { id: String, subject: Option<String>, status: String, line_no: usize, idx: Option<Idx>, session_ix: u16 }` and `Session::task_events: Vec<TaskEvent>`. Task 7 reads this.

**Transcript shape:** `TaskCreate` has `input.subject`, and the created id comes back in the result text. `TaskUpdate` has `input.taskId` and `input.status`. Because the create result's id is free text, we key on the **subject** for creates and the **taskId** for updates, and Task 7 reconciles them positionally (creates are numbered in order, which is how the tool assigns ids).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn task_events_record_creation_and_final_status() {
    // Unfinished work is invisible in a diff: a task created and never
    // completed means the commit is partial, and no reviewer can tell that
    // from the code.
    let create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Wire the cache"}}]}}"#;
    let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:10Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"}}]}}"#;
    let s = ingest_str(&format!("{create}\n{update}"), Lane::Main);

    assert_eq!(s.task_events.len(), 2);
    assert_eq!(s.task_events[0].subject.as_deref(), Some("Wire the cache"));
    assert_eq!(s.task_events[0].status, "pending", "a create starts pending");
    assert_eq!(s.task_events[0].id, "1", "creates are numbered in order");
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core task_events_record_creation_and_final_status`
Expected: FAIL, `no field 'task_events' on type 'Session'`.

- [ ] **Step 3: Add the model type**

In `model.rs`, after `Decision`:

```rust
/// One transition in a task's lifecycle: its creation, or a status change.
///
/// Kept as an event LIST rather than a final-state map because the payload
/// needs to cite the action that left a task unfinished, and a map would
/// have thrown that index away.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskEvent {
    /// The task id. Creates are numbered in the order they appear (`"1"`,
    /// `"2"`, ...) because the create's assigned id comes back only as free
    /// text in the result, which is not a field we can rely on. Updates use
    /// the `taskId` they were given, which matches that numbering.
    pub id: String,
    /// The subject, present on creates and on renames, absent on plain
    /// status updates.
    pub subject: Option<String>,
    /// `"pending"` for a create, otherwise the status the update set.
    pub status: String,
    /// Source line (total-order tiebreak).
    pub line_no: usize,
    /// The action's index, for `evidence()`.
    pub idx: Option<Idx>,
    /// Which transcript of the work unit this came from.
    #[serde(default)]
    pub session_ix: u16,
}
```

Add to `Session`, after `decisions`:

```rust
    /// Task lifecycle transitions, in source order. Replayed by the context
    /// module into a final state per task, so the payload can report work
    /// that was planned and never finished.
    #[serde(default)]
    pub task_events: Vec<TaskEvent>,
```

- [ ] **Step 4: Populate it in ingest**

Add an accumulator beside `pending_decisions`:

```rust
    // Task ids are assigned by the harness and only echoed back as free text,
    // so we number creates ourselves in the order they appear. The harness
    // numbers them the same way, which is why a later TaskUpdate's taskId
    // matches.
    let mut task_events: Vec<TaskEvent> = Vec::new();
    let mut creates_seen: u32 = 0;
```

In the `Some("tool_use")` arm, after the `AskUserQuestion` block:

```rust
                        // Task lifecycle. TaskCreate always starts a task at
                        // pending; TaskUpdate is a lifecycle event only when
                        // it carries a status (it is also used for renames
                        // and dependency edits, which are not evidence of
                        // anything unfinished).
                        if name == "TaskCreate" {
                            creates_seen += 1;
                            task_events.push(TaskEvent {
                                id: creates_seen.to_string(),
                                subject: input
                                    .and_then(|i| i.get("subject"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                status: "pending".to_string(),
                                line_no,
                                idx: None,
                                session_ix: 0,
                            });
                        } else if name == "TaskUpdate"
                            && let Some(status) = input
                                .and_then(|i| i.get("status"))
                                .and_then(Value::as_str)
                        {
                            task_events.push(TaskEvent {
                                id: input
                                    .and_then(|i| i.get("taskId"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?")
                                    .to_string(),
                                subject: input
                                    .and_then(|i| i.get("subject"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string),
                                status: status.to_string(),
                                line_no,
                                idx: None,
                                session_ix: 0,
                            });
                        }
```

Add `task_events,` to the `Session { ... }` literal and `TaskEvent` to the imports.

- [ ] **Step 5: Carry it through both merges**

In `merge_sessions`:

```rust
    // Extended, NOT main-only (unlike decisions and user_texts): a subagent
    // can create tasks, and a task a subagent left unfinished is exactly as
    // unfinished as one the main lane abandoned.
    let mut task_events = main.task_events;
```

and inside the `for sub in subs` loop:

```rust
        task_events.extend(sub.task_events);
```

In `merge_work_unit`, mirror the `decisions` stamping with a `task_events` vec and the same `session_ix` stamp.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p sumcp-core`
Expected: PASS. Fix any `missing field` build errors in test helpers by adding `task_events: vec![],`.

- [ ] **Step 7: Commit**

```bash
git add crates/sumcp-core/src/model.rs crates/sumcp-core/src/ingest.rs crates/sumcp-core/src/merge.rs
git commit -m "feat: ingest captures task lifecycle events

Creates are numbered in appearance order because the assigned id comes back
only as free text; the harness numbers them the same way, so a later
TaskUpdate taskId matches. TaskUpdate counts only when it carries a status,
since it is also used for renames and dependency edits.

Unlike decisions, these extend across subagents: a task a subagent left
unfinished is exactly as unfinished as one the main lane abandoned."
```

---

## Task 4: Ingest captures agent prose

**Files:**
- Modify: `crates/sumcp-core/src/model.rs`
- Modify: `crates/sumcp-core/src/ingest.rs`
- Modify: `crates/sumcp-core/src/merge.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `sumcp_core::model::AgentText { text: String, line_no: usize, session_ix: u16 }` and `Session::agent_texts: Vec<AgentText>`. Task 8 reads this.

**Measured volume:** 60 blocks over 80 chars, 51,358 chars, in one real session. Each block is capped at `AGENT_TEXT_CAP` so one runaway block cannot dominate memory, and the payload caps again on top.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn agent_prose_is_captured_and_short_blocks_are_skipped() {
    // These are the agent's claims about what it did. The reviewer verifies
    // them against the diff, which is the highest-yield automated review
    // question available. Short blocks are conversational filler ("Done.",
    // "Let me check.") and carry no verifiable assertion.
    let long = "x".repeat(100);
    let line = format!(
        r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{long}"}},{{"type":"text","text":"ok"}}]}}}}"#
    );
    let s = ingest_str(&line, Lane::Main);

    assert_eq!(s.agent_texts.len(), 1, "only the long block is a claim");
    assert_eq!(s.agent_texts[0].text.chars().count(), 100);
}

#[test]
fn a_runaway_prose_block_is_capped() {
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core agent_prose_is_captured_and_short_blocks_are_skipped`
Expected: FAIL, `no field 'agent_texts'`.

- [ ] **Step 3: Add the model type and constants**

In `model.rs`, after `TaskEvent`:

```rust
/// One block of agent prose: what it said it did.
///
/// The review payload hands these to the reviewer as CLAIMS to verify against
/// the diff. suMCP never checks them itself, because checking a natural
/// language assertion against code requires understanding both, which is the
/// consuming agent's job and not this tool's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentText {
    /// The prose, capped at `AGENT_TEXT_CAP` characters.
    pub text: String,
    /// Source line (total-order tiebreak).
    pub line_no: usize,
    /// Which transcript of the work unit this came from.
    #[serde(default)]
    pub session_ix: u16,
}
```

In `ingest.rs`, beside `const EDIT_CAP: usize = 2000;`:

```rust
/// Shortest prose block kept as a claim. Below this it is conversational
/// filler ("Done.", "Let me check.") with no verifiable assertion in it.
const AGENT_TEXT_MIN: usize = 80;
/// Longest prose block stored. Unlike `EDIT_CAP`, truncating here cannot skew
/// any metric: nothing counts these characters, they are only quoted.
pub(crate) const AGENT_TEXT_CAP: usize = 4000;
```

Add to `Session`, after `task_events`:

```rust
    /// Agent prose blocks, in source order: what the agent said it did. The
    /// payload presents these as claims for the reviewer to check against
    /// the diff.
    #[serde(default)]
    pub agent_texts: Vec<AgentText>,
```

- [ ] **Step 4: Populate it in ingest**

In the content-block loop, add an arm alongside `Some("tool_use")` and `Some("tool_result")`. It must fire only for assistant lines, since user lines also carry `text` blocks:

```rust
                    Some("text")
                        if v.get("type").and_then(Value::as_str) == Some("assistant") =>
                    {
                        if let Some(t) = block.get("text").and_then(Value::as_str)
                            && t.chars().count() >= AGENT_TEXT_MIN
                        {
                            agent_texts.push(AgentText {
                                // `chars().take()` not `[..n]`: slicing a
                                // String by byte index panics if it lands
                                // mid-character, and prose is full of
                                // multi-byte characters.
                                text: t.chars().take(AGENT_TEXT_CAP).collect(),
                                line_no,
                                session_ix: 0,
                            });
                        }
                    }
```

Declare `let mut agent_texts: Vec<AgentText> = Vec::new();` with the other accumulators, add `agent_texts,` to the `Session` literal, and import `AgentText`.

- [ ] **Step 5: Carry it through both merges**

`merge_sessions`: main only, with the reason.

```rust
    // Main only: subagent prose is internal reasoning the human never saw,
    // and folding it in would multiply the payload's largest block for no
    // reviewer benefit.
    let agent_texts = main.agent_texts;
```

`merge_work_unit`: stamp `session_ix` like `decisions`.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p sumcp-core`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/sumcp-core/src/model.rs crates/sumcp-core/src/ingest.rs crates/sumcp-core/src/merge.rs
git commit -m "feat: ingest captures agent prose as claims

Blocks under 80 chars are conversational filler with nothing verifiable in
them. Blocks are capped at 4000 chars using chars().take() rather than byte
slicing, which panics mid-character on multi-byte prose.

Unlike EDIT_CAP, this truncation cannot skew a metric: nothing counts these
characters, they are only quoted."
```

---

## Task 5: Extract `scope`

**Files:**
- Create: `crates/sumcp-core/src/context.rs`
- Modify: `crates/sumcp-core/src/lib.rs`

**Interfaces:**
- Consumes: `Session::user_texts` (existing, has `is_human` and `session_ix`).
- Produces: `sumcp_core::context::Scope { requests: Vec<Request>, files: Vec<String> }` where `Request { text: String, line_no: usize, session_ix: u16 }`. Task 10 reads this.

- [ ] **Step 1: Write the failing test**

Create `crates/sumcp-core/src/context.rs` with only the test module first:

```rust
//! Review context: the five deterministic extractions a reviewing agent needs.
//!
//! Pure `&Session -> struct`, no JSON and no capping (that is `payloads.rs`).
//! The invariant this module exists to hold: suMCP reports what was recorded,
//! verbatim, with a citation. It never asserts that anything is acceptable or
//! risky, because that judgement belongs to the agent consuming this.

use crate::model::Session;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    #[test]
    fn scope_quotes_human_turns_and_skips_harness_turns() {
        // A harness-injected turn (a task notification) is not the human
        // asking for anything, so quoting it as intent would be a lie about
        // what was requested.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache to the loader"}}"#;
        let bot = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>agent done</task-notification>"}}"#;
        let edit = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/loader.rs","new_string":"x"}}]}}"#;
        let s = ingest_str(&format!("{human}\n{bot}\n{edit}"), Lane::Main);

        let scope = scope(&s);
        assert_eq!(scope.requests.len(), 1, "only the human turn is a request");
        assert_eq!(scope.requests[0].text, "add a cache to the loader");
        assert_eq!(scope.files, vec!["/loader.rs".to_string()]);
    }

    #[test]
    fn scope_files_are_deduped_and_sorted() {
        // Deterministic output is a repo-wide rule: two runs over an
        // unchanged transcript must produce byte-identical payloads.
        let e1 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/b.rs","new_string":"x"}}]}}"#;
        let e2 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/a.rs","new_string":"y"}}]}}"#;
        let e3 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"e3","name":"Edit","input":{"file_path":"/b.rs","new_string":"z"}}]}}"#;
        let s = ingest_str(&format!("{e1}\n{e2}\n{e3}"), Lane::Main);

        assert_eq!(scope(&s).files, vec!["/a.rs".to_string(), "/b.rs".to_string()]);
    }
}
```

Add `pub mod context;` to `lib.rs` in alphabetical position (after `pub mod assemble;`).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core scope_quotes_human_turns_and_skips_harness_turns`
Expected: FAIL, `cannot find function 'scope' in this scope`.

- [ ] **Step 3: Implement**

Add above the test module in `context.rs`:

```rust
use crate::model::{ActionKind, Idx};
use std::collections::BTreeSet;

/// One thing the human asked for, quoted exactly as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The message text, verbatim. Never summarized: quoting is the whole
    /// reason this design beats inferring intent with a model.
    pub text: String,
    /// Source line, so the payload can cite it.
    pub line_no: usize,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
}

/// What was asked, and what was actually touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// Every human turn, in order.
    pub requests: Vec<Request>,
    /// Paths actually acted on, deduped and sorted. This is deliberately NOT
    /// an inference about which files the request "meant" to cover: it is the
    /// observed set, and a reviewer comparing it against the requests can
    /// draw its own conclusion about anything out of scope.
    pub files: Vec<String>,
}

/// Extract what was asked and what was touched.
pub fn scope(s: &Session) -> Scope {
    let requests = s
        .user_texts
        .iter()
        // `is_human` distinguishes a real turn from a harness-injected one
        // (task notifications, hook output). Quoting a harness turn as human
        // intent would misrepresent what was requested.
        .filter(|u| u.is_human)
        .map(|u| Request {
            text: u.text.clone(),
            line_no: u.line_no,
            session_ix: u.session_ix,
        })
        .collect();

    // BTreeSet gives dedup and sort in one step, which is what makes two runs
    // over an unchanged transcript byte-identical.
    let files: BTreeSet<String> = s
        .actions
        .iter()
        .filter(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write))
        .filter_map(|a| a.file_path.clone())
        .collect();

    Scope {
        requests,
        files: files.into_iter().collect(),
    }
}
```

Note: `Idx` is imported for later tasks in this module; if the compiler warns it is unused at this point, leave the import out and add it in Task 6.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core context::`
Expected: PASS, 2 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/context.rs crates/sumcp-core/src/lib.rs
git commit -m "feat: extract scope, the human's requests quoted verbatim

Harness-injected turns are excluded via is_human: quoting a task
notification as human intent would misrepresent what was asked. The file
list is the observed set of edited paths, never an inference about what the
request meant to cover."
```

---

## Task 6: Extract `decisions`

**Files:**
- Modify: `crates/sumcp-core/src/context.rs`

**Interfaces:**
- Consumes: `Session::decisions` (Task 2).
- Produces: `sumcp_core::context::decisions(&Session) -> Vec<DecisionOut>` where `DecisionOut { question: String, chosen: Option<String>, rejected: Vec<String>, line_no: usize, session_ix: u16 }`. Task 10 reads this.

**Why `rejected` and not `options`:** the reviewer's useful question is "what did the human turn down," and computing it here keeps the payload smaller than shipping every option plus the answer. When the answer is free text matching no option, every option is rejected, which is the correct reading.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn decisions_report_what_was_chosen_and_what_it_beat() {
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"},{"label":"memory"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        let d = decisions(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].chosen.as_deref(), Some("JSONL"));
        assert_eq!(d[0].rejected, vec!["SQLite".to_string(), "memory".to_string()]);
    }

    #[test]
    fn a_free_text_answer_rejects_every_offered_option() {
        // The human answered "Other". Nothing on the menu was chosen, so
        // every option was turned down, and the free text is the choice.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"keep it in memory"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        let d = decisions(&s);
        assert_eq!(d[0].chosen.as_deref(), Some("keep it in memory"));
        assert_eq!(d[0].rejected, vec!["SQLite".to_string(), "JSONL".to_string()]);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core decisions_report_what_was_chosen`
Expected: FAIL, `cannot find function 'decisions'`.

- [ ] **Step 3: Implement**

```rust
/// A recorded human choice, rendered for the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOut {
    /// The question, verbatim.
    pub question: String,
    /// What the human chose. `None` when the session ended unanswered.
    pub chosen: Option<String>,
    /// The options that were turned down. When the answer was free text
    /// matching no option, every option is here, which is the correct
    /// reading: nothing on the menu was picked.
    pub rejected: Vec<String>,
    /// Source line, for citation.
    pub line_no: usize,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
}

/// Extract the recorded human decisions.
pub fn decisions(s: &Session) -> Vec<DecisionOut> {
    s.decisions
        .iter()
        .map(|d| DecisionOut {
            question: d.question.clone(),
            chosen: d.answer.clone(),
            rejected: d
                .options
                .iter()
                // Everything that is not the answer was turned down. An
                // unanswered question (answer: None) rejects nothing, since
                // no choice was made at all.
                .filter(|o| d.answer.as_deref().is_some_and(|a| a != o.as_str()))
                .cloned()
                .collect(),
            line_no: d.line_no,
            session_ix: d.session_ix,
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core context::`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/context.rs
git commit -m "feat: extract decisions as chosen plus rejected

The reviewer's useful question is what the human turned down, so the payload
carries the rejected set rather than every option plus the answer. A free
text answer rejects every offered option, which is the correct reading:
nothing on the menu was picked."
```

---

## Task 7: Extract `incomplete`

**Files:**
- Modify: `crates/sumcp-core/src/context.rs`

**Interfaces:**
- Consumes: `Session::task_events` (Task 3), `Session::actions`.
- Produces: `sumcp_core::context::incomplete(&Session) -> Incomplete` where `Incomplete { unfinished_tasks: Vec<UnfinishedTask>, failing_commands: Vec<FailingCommand> }`. Task 10 reads this.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn a_task_never_completed_is_reported_unfinished() {
        let c1 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"Wire the cache"}}]}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"b","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        let done = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"c","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let s = ingest_str(&format!("{c1}\n{c2}\n{done}"), Lane::Main);

        let inc = incomplete(&s);
        assert_eq!(inc.unfinished_tasks.len(), 1, "task 2 never completed");
        assert_eq!(inc.unfinished_tasks[0].subject.as_deref(), Some("Add tests"));
        assert_eq!(inc.unfinished_tasks[0].last_status, "pending");
    }

    #[test]
    fn only_the_last_run_of_a_command_counts_as_failing() {
        // A test that failed and was then fixed is not unfinished work. Only
        // the final state of each distinct command matters.
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#;
        let pass = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let pass_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b2","is_error":false}]}}"#;
        let s = ingest_str(&format!("{fail}\n{fail_r}\n{pass}\n{pass_r}"), Lane::Main);

        assert!(
            incomplete(&s).failing_commands.is_empty(),
            "the command was rerun and passed"
        );
    }

    #[test]
    fn a_command_still_failing_at_the_end_is_reported() {
        let pass = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let pass_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":false}]}}"#;
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b2","is_error":true}]}}"#;
        let s = ingest_str(&format!("{pass}\n{pass_r}\n{fail}\n{fail_r}"), Lane::Main);

        let inc = incomplete(&s);
        assert_eq!(inc.failing_commands.len(), 1);
        assert_eq!(inc.failing_commands[0].command, "cargo test");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core a_task_never_completed_is_reported_unfinished`
Expected: FAIL, `cannot find function 'incomplete'`.

- [ ] **Step 3: Implement**

```rust
use std::collections::BTreeMap;

/// A task that was planned and never reached `completed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedTask {
    /// The task id.
    pub id: String,
    /// Its subject, if one was ever recorded.
    pub subject: Option<String>,
    /// The last status it reached.
    pub last_status: String,
    /// Source line of the last event about it, for citation.
    pub line_no: usize,
}

/// A command whose LAST run in the session failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailingCommand {
    /// The command string, verbatim.
    pub command: String,
    /// The failing action's index, for `evidence()`.
    pub idx: Idx,
}

/// Work that was planned or attempted and did not finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incomplete {
    /// Tasks that never reached `completed`.
    pub unfinished_tasks: Vec<UnfinishedTask>,
    /// Commands still failing when the session ended.
    pub failing_commands: Vec<FailingCommand>,
}

/// Extract work that was planned or attempted and did not finish.
pub fn incomplete(s: &Session) -> Incomplete {
    // Replay the event list into a final state per task. BTreeMap keeps the
    // output ordered by id, so two runs agree byte for byte.
    let mut final_state: BTreeMap<String, (Option<String>, String, usize)> = BTreeMap::new();
    for e in &s.task_events {
        let entry = final_state
            .entry(e.id.clone())
            .or_insert((None, String::new(), 0));
        // A later event's subject wins only when it has one: a plain status
        // update carries no subject and must not erase the one from create.
        if e.subject.is_some() {
            entry.0 = e.subject.clone();
        }
        entry.1 = e.status.clone();
        entry.2 = e.line_no;
    }
    let unfinished_tasks = final_state
        .into_iter()
        .filter(|(_, (_, status, _))| status != "completed" && status != "deleted")
        .map(|(id, (subject, last_status, line_no))| UnfinishedTask {
            id,
            subject,
            last_status,
            line_no,
        })
        .collect();

    // Last run wins, per distinct command string. A test that failed and was
    // then fixed is not unfinished work, so only the final state counts.
    let mut last_run: BTreeMap<String, (bool, Idx)> = BTreeMap::new();
    for a in &s.actions {
        if a.kind == ActionKind::Bash
            && let Some(cmd) = a.command.as_deref()
            && let Some(err) = a.is_error
        {
            last_run.insert(cmd.to_string(), (err, a.idx));
        }
    }
    let failing_commands = last_run
        .into_iter()
        .filter(|(_, (err, _))| *err)
        .map(|(command, (_, idx))| FailingCommand { command, idx })
        .collect();

    Incomplete {
        unfinished_tasks,
        failing_commands,
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core context::`
Expected: PASS, 7 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/context.rs
git commit -m "feat: extract incomplete work, tasks and still-failing commands

Task events are replayed to a final state per id; a status update carries no
subject and must not erase the one from create. Commands are keyed on the
command string with last run winning, because a test that failed and was
then fixed is not unfinished work."
```

---

## Task 8: Extract `claims`

**Files:**
- Modify: `crates/sumcp-core/src/context.rs`

**Interfaces:**
- Consumes: `Session::agent_texts` (Task 4).
- Produces: `sumcp_core::context::claims(&Session) -> Vec<Claim>` where `Claim { text: String, line_no: usize, session_ix: u16 }`. Task 10 reads this.

**Design note carried from the spec's open questions:** every prose block is reported, with a count, and the reviewer chooses. Selecting among them would be a judgment this design has committed to avoiding. If the volume proves unusable in the experiment, the selection rule becomes a real design problem rather than an afterthought.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn claims_are_reported_in_source_order_without_selection() {
        // No filtering beyond the length floor applied at ingest. Choosing
        // which claims "matter" is a judgment this tool does not make.
        let a = "a".repeat(100);
        let b = "b".repeat(100);
        let l1 = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{a}"}}]}}}}"#
        );
        let l2 = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"text","text":"{b}"}}]}}}}"#
        );
        let s = ingest_str(&format!("{l1}\n{l2}"), Lane::Main);

        let c = claims(&s);
        assert_eq!(c.len(), 2);
        assert!(c[0].text.starts_with('a'), "source order preserved");
        assert!(c[1].text.starts_with('b'));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core claims_are_reported_in_source_order`
Expected: FAIL, `cannot find function 'claims'`.

- [ ] **Step 3: Implement**

```rust
/// Something the agent said it did, for the reviewer to check against the
/// diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The prose, verbatim (capped at ingest).
    pub text: String,
    /// Source line, for citation.
    pub line_no: usize,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
}

/// Extract the agent's claims about what it did.
///
/// Every captured prose block is returned, in source order. There is
/// deliberately no selection: deciding which claims are "worth verifying"
/// would be a judgment, and this module makes none. The payload caps the
/// list and reports the true total, so the reviewer knows what it is seeing.
pub fn claims(s: &Session) -> Vec<Claim> {
    s.agent_texts
        .iter()
        .map(|t| Claim {
            text: t.text.clone(),
            line_no: t.line_no,
            session_ix: t.session_ix,
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core context::`
Expected: PASS, 8 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/context.rs
git commit -m "feat: extract claims, every prose block in source order

No selection beyond the length floor applied at ingest. Deciding which
claims are worth verifying would be a judgment this module makes none of;
the payload caps the list and reports the true total instead."
```

---

## Task 9: Extract `constraints` (the one heuristic block)

**Files:**
- Modify: `crates/sumcp-core/src/context.rs`

**Interfaces:**
- Consumes: `crate::score::all_findings` (existing, `pub fn all_findings(s: &Session) -> Vec<Finding>`), `Session::actions`.
- Produces: `sumcp_core::context::constraints(&Session) -> Vec<Constraint>` where `Constraint { what: String, why: String, idxs: Vec<Idx>, exact: bool }`. Task 10 reads this. `exact` is always `false`.

**Why this is heuristic:** "an approach was tried and abandoned" is an inference from shape, not a recorded fact. Two shapes support it: content that was changed and changed back (`FindingKind::TrueRevert`, already computed), and a command that errored and was never rerun. Both carry the repo's heuristic labelling, and the payload must surface it.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn a_command_that_errored_and_was_never_rerun_is_a_constraint() {
        // The reviewer's most useless output is recommending something that
        // was already tried and failed. This is the shape that catches it.
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo add sqlite"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]},"toolUseResult":{"stderr":"no matching package"}}"#;
        let s = ingest_str(&format!("{fail}\n{fail_r}"), Lane::Main);

        let c = constraints(&s);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].what, "cargo add sqlite");
        assert!(c[0].why.contains("no matching package"));
        assert!(!c[0].exact, "this is an inference from shape, never exact");
    }

    #[test]
    fn constraints_are_always_labelled_heuristic() {
        // The repo-wide rule: exact == false requires a stated reason. A
        // constraint that claimed to be exact would be lying about how it
        // was derived.
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"make"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#;
        let s = ingest_str(&format!("{fail}\n{fail_r}"), Lane::Main);

        assert!(constraints(&s).iter().all(|c| !c.exact && !c.why.is_empty()));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core a_command_that_errored_and_was_never_rerun`
Expected: FAIL, `cannot find function 'constraints'`.

- [ ] **Step 3: Implement**

```rust
use crate::model::FindingKind;

/// Something that was attempted and did not work.
///
/// HEURISTIC by construction: "abandoned" is inferred from shape, never
/// recorded. `exact` is always `false` and `why` is always populated, which
/// is the same contract `Finding` holds elsewhere in this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Constraint {
    /// What was attempted, verbatim (a command, or a file path).
    pub what: String,
    /// The recorded reason it did not work.
    pub why: String,
    /// Action indices proving it.
    pub idxs: Vec<Idx>,
    /// Always `false`. Present so a payload cannot serialize this block
    /// without carrying its own honesty label.
    pub exact: bool,
}

/// Extract approaches that were tried and abandoned.
pub fn constraints(s: &Session) -> Vec<Constraint> {
    let mut out = Vec::new();

    // Shape one: a command that errored and was never run again. If it had
    // been rerun, the last-run-wins rule in `incomplete` would cover it and
    // it would not be an abandoned approach.
    let mut last_error: BTreeMap<String, (Idx, String)> = BTreeMap::new();
    let mut rerun_ok: BTreeMap<String, bool> = BTreeMap::new();
    for a in &s.actions {
        if a.kind == ActionKind::Bash
            && let Some(cmd) = a.command.as_deref()
        {
            match a.is_error {
                Some(true) => {
                    last_error.insert(
                        cmd.to_string(),
                        (a.idx, a.error.clone().unwrap_or_default()),
                    );
                    rerun_ok.insert(cmd.to_string(), false);
                }
                Some(false) => {
                    rerun_ok.insert(cmd.to_string(), true);
                }
                None => {}
            }
        }
    }
    for (cmd, (idx, err)) in last_error {
        if rerun_ok.get(&cmd) == Some(&false) {
            out.push(Constraint {
                what: cmd,
                why: err,
                idxs: vec![idx],
                exact: false,
            });
        }
    }

    // Shape two: content changed and changed back. The existing revert signal
    // already computes this, so we read its findings rather than recomputing
    // the comparison and risking a second, subtly different definition.
    for f in crate::score::all_findings(s) {
        if f.kind == FindingKind::TrueRevert {
            out.push(Constraint {
                what: f.file.clone().unwrap_or_else(|| "(no file)".to_string()),
                why: f
                    .note
                    .clone()
                    .unwrap_or_else(|| "content was changed and changed back".to_string()),
                idxs: f.idxs.clone(),
                exact: false,
            });
        }
    }

    out
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core context::`
Expected: PASS, 10 tests.

If `FindingKind::TrueRevert` does not exist under that exact name, run `grep -n "TrueRevert\|Revert" crates/sumcp-core/src/model.rs` and use the actual variant. Do not invent one.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/context.rs
git commit -m "feat: extract constraints, the one heuristic block

Two shapes support 'tried and abandoned': a command that errored and was
never rerun, and content changed then changed back. The second reads the
existing revert finding rather than recomputing the comparison, so there is
only ever one definition of a revert in this crate.

exact is hardcoded false and why is always populated, matching the Finding
contract the rest of the crate holds."
```

---

## Task 10: The `review_context` payload

**Files:**
- Modify: `crates/sumcp-core/src/payloads.rs`

**Interfaces:**
- Consumes: `context::{scope, decisions, incomplete, claims, constraints}` (Tasks 5 to 9), and the existing `session_block(&SessionMeta) -> (Value, bool)` and `shrink_to_fit(cap, start, build)` helpers.
- Produces: `payloads::review_context(s: &Session, meta: &SessionMeta) -> Value`. Tasks 13 and 14 call this.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `payloads.rs`:

```rust
    #[test]
    fn review_context_carries_all_five_blocks_and_true_totals() {
        // "3 shown" must never be mistaken for "3 happened": the same rule
        // blind_spots already holds. Totals are unconditional; lists are a
        // capped sample.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let ask = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let ans = r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let task = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        let s = crate::ingest::ingest_str(
            &format!("{human}\n{ask}\n{ans}\n{task}"),
            crate::model::Lane::Main,
        );
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert_eq!(p["v"], 3, "new contract version");
        assert_eq!(p["scope"]["requests"][0]["text"], "add a cache");
        assert_eq!(p["decisions"][0]["chosen"], "JSONL");
        assert_eq!(p["decisions"][0]["rejected"][0], "SQLite");
        assert_eq!(p["incomplete"]["unfinished_tasks"][0]["subject"], "Add tests");
        assert_eq!(p["totals"]["decisions"], 1);
        assert_eq!(p["totals"]["unfinished_tasks"], 1);
    }

    #[test]
    fn review_context_stays_under_its_token_cap_on_a_dense_session() {
        // A real session has 13 human messages and 60 prose blocks. Without
        // shrink_to_fit this payload would blow past every other payload in
        // the crate combined.
        let mut lines = Vec::new();
        for i in 0..40 {
            let text = "z".repeat(400);
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:{i:02}Z","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert!(
            est_tokens(&p) <= CAP_REVIEW_CONTEXT,
            "payload must fit its cap, got {}",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true, "and must say so");
        assert_eq!(p["totals"]["claims"], 40, "true total survives the cap");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core review_context_carries_all_five_blocks`
Expected: FAIL, `cannot find function 'review_context'`.

- [ ] **Step 3: Implement**

Add the caps beside the existing ones near the top of `payloads.rs`:

```rust
/// Token cap for `review_context`. Larger than the other payloads because it
/// carries quoted prose rather than counts, and smaller than
/// `session_intent` because it is PUSHED into the reviewer's context by
/// default. Keeping the pushed payload small is what avoids the
/// overcorrection effect the design is built around.
const CAP_REVIEW_CONTEXT: usize = 2000;
/// Starting list length for each block, walked down by `shrink_to_fit`.
const CONTEXT_LIST_MAX: usize = 12;
/// Longest single quoted string in `review_context`. A quote longer than this
/// belongs in `session_intent`, which the reviewer pulls deliberately.
const QUOTE_MAX: usize = 500;
```

Add the function:

```rust
/// Recorded session context for a reviewing agent (`v: 3`).
///
/// Five blocks, four of them exact and one heuristic. Every list is a capped
/// sample and every corresponding total is unconditional, so "3 shown" can
/// never be read as "3 happened".
pub fn review_context(s: &Session, meta: &SessionMeta) -> Value {
    let scope = crate::context::scope(s);
    let decisions = crate::context::decisions(s);
    let incomplete = crate::context::incomplete(s);
    let claims = crate::context::claims(s);
    let constraints = crate::context::constraints(s);
    let (session, id_cut) = session_block(meta);

    // The true counts, computed once and never subject to the cap below.
    let totals = json!({
        "requests": scope.requests.len(),
        "files": scope.files.len(),
        "decisions": decisions.len(),
        "unfinished_tasks": incomplete.unfinished_tasks.len(),
        "failing_commands": incomplete.failing_commands.len(),
        "claims": claims.len(),
        "constraints": constraints.len()
    });
    let longest = scope
        .requests
        .len()
        .max(decisions.len())
        .max(claims.len())
        .max(constraints.len())
        .max(incomplete.unfinished_tasks.len())
        .max(incomplete.failing_commands.len());

    shrink_to_fit(CAP_REVIEW_CONTEXT, CONTEXT_LIST_MAX, |k| {
        json!({
            "v": 3,
            "session": session,
            "scope": {
                "requests": scope.requests.iter().take(k).map(|r| json!({
                    "text": elide_middle(&r.text, QUOTE_MAX),
                    "line": r.line_no
                })).collect::<Vec<_>>(),
                "files": scope.files.iter().take(k).map(|f| json!(
                    elide_middle(f, PATH_MAX)
                )).collect::<Vec<_>>()
            },
            "decisions": decisions.iter().take(k).map(|d| json!({
                "question": elide_middle(&d.question, QUOTE_MAX),
                "chosen": d.chosen.as_ref().map(|c| elide_middle(c, QUOTE_MAX)),
                "rejected": d.rejected,
                "line": d.line_no
            })).collect::<Vec<_>>(),
            "constraints": constraints.iter().take(k).map(|c| json!({
                "what": elide_middle(&c.what, QUOTE_MAX),
                "why": elide_middle(&c.why, QUOTE_MAX),
                "idxs": c.idxs.iter().take(FINDING_IDXS_MAX).collect::<Vec<_>>(),
                // Never omitted: a heuristic block that did not say so would
                // be indistinguishable from the four exact ones.
                "exact": false
            })).collect::<Vec<_>>(),
            "incomplete": {
                "unfinished_tasks": incomplete.unfinished_tasks.iter().take(k)
                    .map(|t| json!({
                        "subject": t.subject.as_ref().map(|x| elide_middle(x, QUOTE_MAX)),
                        "last_status": t.last_status,
                        "line": t.line_no
                    })).collect::<Vec<_>>(),
                "failing_commands": incomplete.failing_commands.iter().take(k)
                    .map(|c| json!({
                        "command": elide_middle(&c.command, QUOTE_MAX),
                        "idxs": [c.idx]
                    })).collect::<Vec<_>>()
            },
            "claims": claims.iter().take(k).map(|c| json!({
                "text": elide_middle(&c.text, QUOTE_MAX),
                "line": c.line_no
            })).collect::<Vec<_>>(),
            "totals": totals,
            "list_cap": k,
            // The reviewer must know the full intent is available rather than
            // guessing that what it received is everything there was.
            "full_intent_via": "session_intent",
            "truncated": longest > k || id_cut
        })
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core payloads::`
Expected: PASS. If `elide_middle` or `est_tokens` have different signatures, run `grep -n "fn elide_middle\|fn est_tokens" crates/sumcp-core/src/payloads.rs` and match them exactly.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/payloads.rs
git commit -m "feat: the review_context payload, v3

Five blocks, four exact and one heuristic, every list capped and every total
unconditional so 3 shown is never read as 3 happened. The exact: false flag
on constraints is never omitted, because a heuristic block that did not say
so would be indistinguishable from the exact ones.

Capped tighter than session_intent on purpose: this payload is pushed into
the reviewer's context, and keeping the pushed payload small is what avoids
the overcorrection effect the design is built around."
```

---

## Task 11: The `session_intent` payload

**Files:**
- Modify: `crates/sumcp-core/src/payloads.rs`

**Interfaces:**
- Consumes: `context::scope` (Task 5).
- Produces: `payloads::session_intent(s: &Session, meta: &SessionMeta, max_tokens: Option<usize>) -> Value`. Tasks 13 and 14 call this.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn session_intent_returns_full_requests_and_honours_a_smaller_budget() {
        let mut lines = Vec::new();
        for i in 0..30 {
            let text = "q".repeat(300);
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let big = session_intent(&s, &meta, None);
        let small = session_intent(&s, &meta, Some(300));

        assert_eq!(big["totals"]["requests"], 30);
        assert_eq!(small["totals"]["requests"], 30, "total never shrinks");
        assert!(
            small["requests"].as_array().unwrap().len()
                < big["requests"].as_array().unwrap().len(),
            "a smaller budget returns fewer requests"
        );
        assert!(est_tokens(&small) <= 300);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core session_intent_returns_full_requests`
Expected: FAIL, `cannot find function 'session_intent'`.

- [ ] **Step 3: Implement**

```rust
/// Default cap for `session_intent`. Far larger than every other payload
/// because this one is PULLED deliberately by a reviewer that has decided it
/// needs the full request text, not pushed at one that did not ask.
const CAP_INTENT: usize = 20_000;
/// Starting request count for `session_intent`, walked down by
/// `shrink_to_fit`. High enough that a normal session is never trimmed.
const INTENT_LIST_MAX: usize = 200;

/// The full verbatim human requests for the session (`v: 3`).
///
/// Deliberately a separate tool from `review_context`: supplying requirements
/// to a reviewer up front and asking it to check conformance induces
/// overcorrection, where the model assumes flaws exist and flags correct code
/// (arXiv:2603.00539). Making the reviewer ask for this keeps it out of the
/// default context.
///
/// `max_tokens` lets a caller request a smaller budget than `CAP_INTENT`.
/// A caller-supplied budget can only ever LOWER the cap, never raise it.
pub fn session_intent(s: &Session, meta: &SessionMeta, max_tokens: Option<usize>) -> Value {
    let scope = crate::context::scope(s);
    let (session, id_cut) = session_block(meta);
    let cap = max_tokens.unwrap_or(CAP_INTENT).min(CAP_INTENT);
    let total = scope.requests.len();

    shrink_to_fit(cap, INTENT_LIST_MAX.min(total.max(1)), |k| {
        json!({
            "v": 3,
            "session": session,
            "requests": scope.requests.iter().take(k).map(|r| json!({
                // NOT elided: the entire point of this tool is the full text.
                // If it does not fit, shrink_to_fit drops whole requests and
                // says so, rather than silently mutilating each one.
                "text": r.text,
                "line": r.line_no
            })).collect::<Vec<_>>(),
            "totals": {"requests": total},
            "truncated": total > k || id_cut
        })
    })
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core payloads::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/payloads.rs
git commit -m "feat: the session_intent payload, full verbatim requests

Separate from review_context on purpose: supplying requirements up front and
asking a reviewer to check conformance induces overcorrection
(arXiv:2603.00539), so the full text is pulled deliberately rather than
pushed at a reviewer that did not ask.

Request text is never elided. If the budget is exceeded, whole requests are
dropped and truncated says so, because a half-quoted request is worse than
a missing one for a tool whose entire premise is quoting."
```

---

## Task 12: The `git` module, commit range to session window

**Files:**
- Create: `crates/sumcp-core/src/git.rs`
- Modify: `crates/sumcp-core/src/lib.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `sumcp_core::git::range_window(repo: &Path, range: &str) -> std::io::Result<(String, String)>` returning `(oldest_commit_iso, newest_commit_iso)` in the same ISO 8601 form transcripts use. Tasks 13 and 14 call this.

**Why this is the only I/O exception:** it shells out, so it can never be called from a signal. It lives outside `context.rs` precisely so that boundary is visible in the module list.

- [ ] **Step 1: Write the failing test**

```rust
//! Commit range to time window. The only part of this crate that shells out.
//!
//! Kept in its own module so the no-I/O-below-ingest rule (ADR A2) stays
//! visibly intact: nothing in `signals/` or `context.rs` may call this.

use std::path::Path;
use std::process::Command;

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway repo with two commits at known times.
    fn fixture_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str], env: &[(&str, &str)]| {
            let mut c = Command::new("git");
            c.args(args).current_dir(p);
            for (k, v) in env {
                c.env(k, v);
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        };
        run(&["init", "-q"], &[]);
        run(&["config", "user.email", "t@example.com"], &[]);
        run(&["config", "user.name", "t"], &[]);
        std::fs::write(p.join("a.txt"), "one").unwrap();
        run(&["add", "."], &[]);
        run(
            &["commit", "-q", "-m", "first"],
            &[
                ("GIT_AUTHOR_DATE", "2026-01-01T10:00:00Z"),
                ("GIT_COMMITTER_DATE", "2026-01-01T10:00:00Z"),
            ],
        );
        std::fs::write(p.join("a.txt"), "two").unwrap();
        run(&["add", "."], &[]);
        run(
            &["commit", "-q", "-m", "second"],
            &[
                ("GIT_AUTHOR_DATE", "2026-01-02T15:30:00Z"),
                ("GIT_COMMITTER_DATE", "2026-01-02T15:30:00Z"),
            ],
        );
        dir
    }

    #[test]
    fn range_window_returns_oldest_and_newest_commit_times() {
        let dir = fixture_repo();
        let (from, to) = range_window(dir.path(), "HEAD~1..HEAD").unwrap();
        assert!(from.starts_with("2026-01-02"), "got {from}");
        assert!(to.starts_with("2026-01-02"), "got {to}");
    }

    #[test]
    fn a_two_commit_range_spans_both_times() {
        let dir = fixture_repo();
        let (from, to) = range_window(dir.path(), "HEAD~2..HEAD").unwrap();
        assert!(from.starts_with("2026-01-01"), "oldest first: got {from}");
        assert!(to.starts_with("2026-01-02"), "newest last: got {to}");
    }

    #[test]
    fn a_bad_range_is_an_error_not_a_guess() {
        // Guessing a window from a range git rejected would silently analyze
        // the wrong sessions, which is worse than failing.
        let dir = fixture_repo();
        assert!(range_window(dir.path(), "no-such-ref..HEAD").is_err());
    }
}
```

Add `pub mod git;` to `lib.rs`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-core git::`
Expected: FAIL, `cannot find function 'range_window'`.

- [ ] **Step 3: Implement**

```rust
/// The commit times bounding a range, oldest first, as ISO 8601 strings.
///
/// Returns `(oldest, newest)`. Both are in the strict ISO form
/// (`2026-01-02T15:30:00Z`) that transcript timestamps use, so a caller can
/// compare them against `effective_ts` with plain string ordering, which is
/// how every other time comparison in this crate already works.
///
/// Errors rather than guessing when git rejects the range: a guessed window
/// would silently select the wrong sessions, which is worse than failing.
pub fn range_window(repo: &Path, range: &str) -> std::io::Result<(String, String)> {
    let out = Command::new("git")
        .args(["log", "--format=%cI", range])
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("git log rejected the range '{range}'"),
        ));
    }
    // `git log` prints newest first, so the last line is the oldest commit.
    let times: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let (Some(newest), Some(oldest)) = (times.first(), times.last()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no commits in range '{range}'"),
        ));
    };
    Ok((oldest.clone(), newest.clone()))
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-core git::`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/git.rs crates/sumcp-core/src/lib.rs
git commit -m "feat: a git module mapping a commit range to a time window

The only part of the core that shells out, kept in its own module so the
no-IO-below-ingest rule stays visibly intact. Times are emitted in the same
strict ISO form transcripts use, so callers compare with plain string
ordering like every other time comparison in this crate.

A range git rejects is an error, never a guess: a guessed window would
silently select the wrong sessions."
```

---

## Task 13: Register the two MCP tools

**Files:**
- Modify: `crates/sumcp-mcp/src/server.rs`

**Interfaces:**
- Consumes: `payloads::review_context` (Task 10), `payloads::session_intent` (Task 11).
- Produces: two tools on the wire, `review_context` and `session_intent`.

**Note on `commit_range`:** the MCP server resolves sessions by the calling session id, not by commit. The `commit_range` argument is accepted and, for this first build, is **recorded in the payload but does not change session selection**, because the server's session resolution is already work-unit scoped and cross-checking it against git would need the repo path the server does not have. The CLI (Task 14) is where a range genuinely selects. This limitation is disclosed in the tool description rather than hidden.

- [ ] **Step 1: Write the failing test**

Add to `crates/sumcp-mcp/tests/stdio.rs`:

```rust
#[tokio::test]
async fn review_context_and_session_intent_answer_over_stdio() {
    // The two new tools must be listed AND callable, since a tool that lists
    // but errors on call is worse than one that does not exist.
    let (server, args) = fixture_server_and_args().await;

    let ctx = server
        .call_tool_for_test("review_context", &args)
        .await
        .expect("review_context callable");
    assert_eq!(ctx["v"], 3);
    assert!(ctx["totals"].is_object(), "totals always present");

    let intent = server
        .call_tool_for_test("session_intent", &args)
        .await
        .expect("session_intent callable");
    assert_eq!(intent["v"], 3);
    assert!(intent["requests"].is_array());
}
```

Match `fixture_server_and_args` and `call_tool_for_test` to whatever the existing tests in that file use. Run `grep -n "fn call_tool_for_test\|async fn " crates/sumcp-mcp/tests/stdio.rs crates/sumcp-mcp/src/server.rs` and copy the existing helper exactly rather than inventing one.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-mcp review_context_and_session_intent_answer_over_stdio`
Expected: FAIL, `unknown tool 'review_context'`.

- [ ] **Step 3: Add the tool descriptors**

In `tool_list()`, after the `evidence` entry:

```rust
        tool(
            "review_context",
            "Recorded session context for reviewing a change: what was asked, what the human decided (and the options rejected), what was tried and failed, what was left unfinished, and what the agent claimed it did. Facts with citations, never judgments. Call session_intent for the full request text.",
            serde_json::json!({"commit_range": {"type": "string", "description": "Informational only in this version: sessions are resolved from the calling session, not from git."}}),
            &[],
        ),
        tool(
            "session_intent",
            "The full verbatim human requests for the session. Pull this only when reasoning about a specific change; it is large by design and is deliberately not included in review_context.",
            serde_json::json!({"max_tokens": {"type": "integer", "description": "Optional smaller budget. Can only lower the default cap, never raise it."}}),
            &[],
        ),
```

Update the doc comment above `tool_list` from "The six tools" to "The eight tools", and update `info.instructions` in `get_info` from "Six read-only tools" to "Eight read-only tools".

- [ ] **Step 4: Add the dispatch arms**

In `call_tool`, before the `other =>` arm:

```rust
            "review_context" => payloads::review_context(session, &meta),
            "session_intent" => {
                let max = args
                    .get("max_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as usize);
                payloads::session_intent(session, &meta, max)
            }
```

Add the identical two arms to the second dispatch site (the test helper around line 387, which mirrors `call_tool` against `loaded.session`). Missing that second site is the most likely mistake in this task, because the code compiles fine without it and only the stdio test catches it.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p sumcp-mcp`
Expected: PASS, including `six_tools_answer_the_frozen_contract_over_stdio`. That test asserts a tool count and **will fail**; update it to eight and rename it to `eight_tools_answer_the_frozen_contract_over_stdio`.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-mcp/src/server.rs crates/sumcp-mcp/tests/stdio.rs
git commit -m "feat: register review_context and session_intent as MCP tools

commit_range is accepted and recorded but does not yet change session
selection: the server resolves sessions from the calling session and has no
repo path to cross-check against git. Stated in the tool description rather
than hidden, and the CLI is where a range genuinely selects."
```

---

## Task 14: The `sumcp context` CLI command

**Files:**
- Modify: `crates/sumcp-cli/src/main.rs`

**Interfaces:**
- Consumes: `payloads::review_context`, `payloads::session_intent`, `git::range_window`.
- Produces: `sumcp context [--range <rev-range>] [--intent]` printing JSON to stdout.

**Why the CLI gets the real range handling:** it knows the working directory, so it can call git. This is the path a reviewer with no MCP wiring uses, which the spec requires.

- [ ] **Step 1: Write the failing test**

Add to `crates/sumcp-cli/tests/` a new file `context_cmd.rs`:

```rust
//! `sumcp context` end to end against a fixture transcript.

use std::process::Command;

/// The binary under test, as cargo built it.
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_sumcp")
}

#[test]
fn context_prints_a_v3_payload_for_an_explicit_file() {
    let out = Command::new(bin())
        .args(["context", "--file", "fixtures/demo/demo-session.jsonl"])
        .output()
        .expect("ran");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is valid JSON");
    assert_eq!(v["v"], 3);
    assert!(v["totals"].is_object());
}

#[test]
fn context_intent_prints_the_requests_payload() {
    let out = Command::new(bin())
        .args(["context", "--file", "fixtures/demo/demo-session.jsonl", "--intent"])
        .output()
        .expect("ran");
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert!(v["requests"].is_array());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p sumcp-cli context_prints_a_v3_payload_for_an_explicit_file`
Expected: FAIL, non-zero exit with the usage message.

- [ ] **Step 3: Implement**

Add a `Context` variant to the CLI's command enum, mirroring how `Install` and `Uninstall` are declared. Then in `main`, alongside the existing `Install` / `Uninstall` arms:

```rust
        Some(Command::Context { file, range, intent }) => {
            // Resolve which transcript(s) to read, reusing the same target
            // resolution the default path already uses so `sumcp context`
            // and bare `sumcp` never disagree about what "this session" is.
            let target = match resolve_target(file, home.as_deref(), cwd.as_deref()) {
                Ok(t) => t,
                Err(why) => {
                    eprintln!("sumcp: {why}");
                    return ExitCode::FAILURE;
                }
            };
            // A range narrows nothing yet if git cannot answer; that is a
            // hard error rather than a silent full-session answer, because
            // answering about the wrong sessions is the failure this whole
            // design exists to prevent.
            if let Some(r) = range.as_deref()
                && let Some(dir) = cwd.as_deref()
            {
                match sumcp_core::git::range_window(dir, r) {
                    Ok((from, to)) => {
                        eprintln!("sumcp: range {r} spans {from} .. {to}");
                    }
                    Err(e) => {
                        eprintln!("sumcp: could not resolve range '{r}': {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            let assembled = match sumcp_core::assemble::load_work_unit(
                &target.path,
                sumcp_core::assemble::MAX_TRANSCRIPT_BYTES,
            ) {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("sumcp: could not read {}: {e}", target.path.display());
                    return ExitCode::FAILURE;
                }
            };
            let meta = sumcp_core::payloads::SessionMeta {
                id: target.id.clone(),
                identified_by: "explicit".into(),
                unit: assembled.meta_unit.clone(),
            };
            let payload = if intent {
                sumcp_core::payloads::session_intent(&assembled.session, &meta, None)
            } else {
                sumcp_core::payloads::review_context(&assembled.session, &meta)
            };
            println!("{}", serde_json::to_string_pretty(&payload).unwrap_or_default());
            return ExitCode::SUCCESS;
        }
```

Match `resolve_target`, `load_work_unit`, and `SessionMeta`'s real field names by reading `crates/sumcp-cli/src/main.rs` around line 188 and `crates/sumcp-core/src/payloads.rs` for `SessionMeta`. Do not invent field names.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p sumcp-cli`
Expected: PASS, including the existing `flag_conflicts` tests. If `--range` needs to conflict with anything, add the conflict declaration and a test for it, matching the pattern already in `flag_conflicts.rs`.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-cli/src/main.rs crates/sumcp-cli/tests/context_cmd.rs
git commit -m "feat: sumcp context, the no-MCP path to the review payloads

The CLI knows its working directory, so this is where a commit range is
genuinely resolved. An unresolvable range is a hard error rather than a
silent full-session answer: answering about the wrong sessions is the exact
failure this design exists to prevent."
```

---

## Task 15: The independent recount gate

**Files:**
- Create: `crates/sumcp-core/tests/context_recount.rs`

**Interfaces:**
- Consumes: `context::{decisions, incomplete, claims}`, `payloads::review_context`.
- Produces: a CI gate. Nothing depends on it.

**Why:** the repo's existing naive recounter caught a 3x undercount that 271 green tests missed, because every fixture shared the production code's blind spot. This is the same insurance for extraction, and it must re-parse the raw JSON independently rather than reusing `ingest_str`.

- [ ] **Step 1: Write the test**

```rust
//! An independent, deliberately naive recount of the review-context blocks.
//!
//! The rule this enforces: a second implementation that shares NO code with
//! the production path must agree exactly on every count. The production
//! extractor is clever (it joins across lines, dedups replays, replays task
//! state); this one is stupid on purpose. When they disagree, one of them is
//! wrong, and a disagreement is the only cheap way to find out which.
//!
//! Precedent: `scripts/recount.py` caught a 3x undercount that 271 green
//! tests missed, because every fixture shared the production blind spot.

use serde_json::Value;
use std::path::Path;

/// Count AskUserQuestion questions, TaskCreate calls, and long prose blocks
/// by walking the raw JSON. No shared helpers with `ingest.rs`.
fn naive_counts(raw: &str) -> (usize, usize, usize) {
    let (mut questions, mut creates, mut prose) = (0, 0, 0);
    for line in raw.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let is_assistant = v.get("type").and_then(Value::as_str) == Some("assistant");
        let Some(content) = v
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for b in content {
            match b.get("type").and_then(Value::as_str) {
                Some("tool_use") => match b.get("name").and_then(Value::as_str) {
                    Some("AskUserQuestion") => {
                        questions += b
                            .get("input")
                            .and_then(|i| i.get("questions"))
                            .and_then(Value::as_array)
                            .map(Vec::len)
                            .unwrap_or(0);
                    }
                    Some("TaskCreate") => creates += 1,
                    _ => {}
                },
                Some("text") if is_assistant => {
                    if b.get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t.chars().count() >= 80)
                    {
                        prose += 1;
                    }
                }
                _ => {}
            }
        }
    }
    (questions, creates, prose)
}

#[test]
fn extraction_agrees_with_an_independent_naive_count() {
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
        let (q, c, p) = naive_counts(&raw);

        let s = sumcp_core::ingest::ingest_str(&raw, sumcp_core::model::Lane::Main);

        assert_eq!(
            sumcp_core::context::decisions(&s).len(),
            q,
            "decision count disagrees in {name}"
        );
        assert_eq!(
            sumcp_core::context::claims(&s).len(),
            p,
            "claim count disagrees in {name}"
        );
        // Every created task appears in the event list; the extractor then
        // filters to unfinished, so unfinished can only be <= creates.
        assert!(
            sumcp_core::context::incomplete(&s).unfinished_tasks.len() <= c,
            "more unfinished tasks than were ever created in {name}"
        );
    }
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p sumcp-core --test context_recount`
Expected: PASS. **If it fails, the production extractor is wrong, not the test.** The most likely cause is `ingest_str`'s replay dedup removing a duplicated `tool_use` id that the naive counter counts twice. If so, make the naive counter dedup by `tool_use` id too, and document in a comment that it is the ONLY production rule it is permitted to mirror.

- [ ] **Step 3: Commit**

```bash
git add crates/sumcp-core/tests/context_recount.rs
git commit -m "test: an independent naive recount gate for review-context extraction

Shares no code with the production extractor. Precedent: scripts/recount.py
caught a 3x undercount that 271 green tests missed, because every fixture
shared the production code's blind spot. Extraction gets the same insurance."
```

---

## Task 16: The two-arm precision experiment

**Files:**
- Create: `scripts/review_experiment.py`

**Interfaces:**
- Consumes: `sumcp context --file <transcript>` (Task 14), the `codex` CLI.
- Produces: `docs/validation/YYYY-MM-DD-review-precision.md` and a scratch JSON of per-finding records.

**Read Task 1's result before starting.** If the power estimate said the finding-level design needs more commits than are runnable, the harness must implement **paired adjudication** (both arms judged against the same union of findings) instead of two independent samples, because that is far more efficient. Do not build the underpowered version.

- [ ] **Step 1: Write the harness**

```python
#!/usr/bin/env python3
"""Two-arm review-precision experiment (dev-only, stdlib only).

Question: does recorded session context reduce the share of a reviewing
agent's findings that are invalid?

  Arm A (blind):          codex reviews the commit, diff only.
  Arm B (contextualized): codex reviews the same commit with the output of
                          `sumcp context` supplied as additional context.

PRIMARY METRIC, declared here before any run and not changed afterwards:
the invalid share of findings in arm B minus arm A, with a 95% CI.
SECONDARY METRIC, reported whatever it says: true positives found in A and
missed in B (the tunnel-vision cost).

Adjudication is MANUAL and the reason is deliberate: an LLM judging whether
an LLM's finding is valid reintroduces exactly the circularity that made the
2026-07-22 study unfalsifiable. The harness collects and pairs; a human
labels. Labels are cached by finding hash so a rerun never re-asks.

Usage:
  review_experiment.py collect --repo . --commits HEAD~20..HEAD
  review_experiment.py adjudicate      # interactive labelling
  review_experiment.py report
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

RAW = Path(".superpowers/sdd/review-experiment.json")

# Same taxonomy as arXiv:2607.03316, so the result is comparable to its
# 56.3% baseline rather than to a category scheme invented here.
LABELS = ["valid", "false_positive", "redundant", "out_of_scope", "misaligned_intent"]


def finding_key(commit: str, f: dict) -> str:
    """Stable id for a finding, so labels survive reruns."""
    blob = f"{commit}|{f.get('file')}|{f.get('line_start')}|{f.get('summary', '')[:200]}"
    return hashlib.sha256(blob.encode()).hexdigest()[:16]


def load() -> dict:
    if RAW.exists():
        return json.loads(RAW.read_text())
    return {"findings": {}, "labels": {}}


def save(state: dict) -> None:
    RAW.parent.mkdir(parents=True, exist_ok=True)
    RAW.write_text(json.dumps(state, indent=1, sort_keys=True))


def run_codex(repo: str, commit: str, context: str | None) -> list[dict]:
    """One adversarial review. Returns the findings list, or [] on failure."""
    prompt = f"Adversarially review commit {commit}. Return JSON findings."
    if context:
        prompt += (
            "\n\nRecorded context from the session that produced this commit. "
            "These are facts with citations, not judgments. Use them to avoid "
            "reporting things that were deliberate, already tried, or out of "
            "scope:\n" + context
        )
    try:
        out = subprocess.run(
            ["codex", "exec", "--json", prompt],
            cwd=repo, capture_output=True, text=True, timeout=900,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        print(f"  codex failed on {commit}: {e}", file=sys.stderr)
        return []
    for line in out.stdout.splitlines():
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(d, dict) and isinstance(d.get("findings"), list):
            return d["findings"]
    return []


def cmd_collect(args: argparse.Namespace) -> None:
    state = load()
    commits = subprocess.run(
        ["git", "log", "--format=%H", args.commits],
        cwd=args.repo, capture_output=True, text=True, check=True,
    ).stdout.split()
    print(f"{len(commits)} commits")
    for i, c in enumerate(commits, 1):
        if any(v["commit"] == c for v in state["findings"].values()):
            continue  # already collected, never re-run a paid call
        ctx = subprocess.run(
            ["sumcp", "context"], cwd=args.repo, capture_output=True, text=True
        ).stdout
        for arm, context in (("A", None), ("B", ctx)):
            for f in run_codex(args.repo, c, context):
                k = finding_key(c, f)
                state["findings"].setdefault(
                    f"{arm}:{k}", {"commit": c, "arm": arm, "finding": f}
                )
        print(f"  [{i}/{len(commits)}] {c[:8]}")
        save(state)
    save(state)


def cmd_adjudicate(args: argparse.Namespace) -> None:
    state = load()
    todo = [k for k in state["findings"] if k not in state["labels"]]
    print(f"{len(todo)} findings to label. Labels: {', '.join(LABELS)}")
    for k in todo:
        rec = state["findings"][k]
        f = rec["finding"]
        # The ARM IS HIDDEN. A labeller who knows which arm produced a
        # finding will label it differently, and that bias would be
        # indistinguishable from the effect being measured.
        print(f"\ncommit {rec['commit'][:8]}  {f.get('file')}:{f.get('line_start')}")
        print(f"  {f.get('summary', '')[:400]}")
        while True:
            ans = input(f"label {LABELS} (or 'skip'): ").strip()
            if ans == "skip":
                break
            if ans in LABELS:
                state["labels"][k] = ans
                save(state)
                break


def wilson(k: int, n: int) -> tuple[float, float]:
    """Wilson 95% CI for a proportion. Better than normal approx at small n,
    which is exactly the regime this experiment lives in."""
    if n == 0:
        return (0.0, 0.0)
    z = 1.959963985
    p = k / n
    d = 1 + z * z / n
    c = p + z * z / (2 * n)
    m = z * ((p * (1 - p) / n + z * z / (4 * n * n)) ** 0.5)
    return ((c - m) / d, (c + m) / d)


def cmd_report(args: argparse.Namespace) -> None:
    state = load()
    counts = {"A": [0, 0], "B": [0, 0]}  # [invalid, total]
    for k, label in state["labels"].items():
        arm = state["findings"][k]["arm"]
        counts[arm][1] += 1
        if label != "valid":
            counts[arm][0] += 1
    lines = ["# Review-precision experiment", "",
             "Primary metric declared before collection: invalid share, arm B",
             "minus arm A. Baseline for comparison: 56.3% (arXiv:2607.03316).",
             "", "| arm | findings | invalid | share | 95% CI |",
             "|---|---|---|---|---|"]
    for arm in ("A", "B"):
        bad, tot = counts[arm]
        share = bad / tot if tot else 0.0
        lo, hi = wilson(bad, tot)
        name = "A (blind)" if arm == "A" else "B (contextualized)"
        lines.append(f"| {name} | {tot} | {bad} | {share:.3f} | {lo:.3f}-{hi:.3f} |")
    a_share = counts["A"][0] / counts["A"][1] if counts["A"][1] else 0.0
    b_share = counts["B"][0] / counts["B"][1] if counts["B"][1] else 0.0
    lines += ["", f"**Difference (B - A): {b_share - a_share:+.3f}**", "",
              "Kill criterion from the spec: if the CIs overlap such that no",
              "improvement is demonstrated, the context does not improve",
              "precision and the memory layer must not be built."]
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines) + "\n")
    print("\n".join(lines))


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(required=True)
    c = sub.add_parser("collect"); c.set_defaults(fn=cmd_collect)
    c.add_argument("--repo", default="."); c.add_argument("--commits", required=True)
    a = sub.add_parser("adjudicate"); a.set_defaults(fn=cmd_adjudicate)
    r = sub.add_parser("report"); r.set_defaults(fn=cmd_report)
    r.add_argument("--out", default="docs/validation/2026-08-10-review-precision.md")
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Verify it runs without calling codex**

Run: `python3 scripts/review_experiment.py report --out /tmp/r.md`
Expected: an empty table with zeros, and no crash. This proves the reporting path works before any expensive collection.

- [ ] **Step 3: Add the scratch path to .gitignore**

Confirm `.superpowers/sdd/` is already ignored: `grep -n "superpowers" .gitignore`. If not, add it. The raw file contains real project paths and finding text and must not be committed.

- [ ] **Step 4: Commit**

```bash
git add scripts/review_experiment.py .gitignore
git commit -m "test: the two-arm review-precision experiment harness

Primary metric declared in the docstring before any run, matching the
discipline that made the 2026-07-22 negative result credible. The
adjudication taxonomy matches arXiv:2607.03316 so the result is comparable
to its 56.3% baseline.

Adjudication is manual and the arm is hidden from the labeller. An LLM
judging whether an LLM finding is valid would reintroduce exactly the
circularity that made the earlier study unfalsifiable, and a labeller who
knows the arm produces a bias indistinguishable from the effect."
```

---

## Execution Protocol

Standard subagent-driven development with **one substitution, decided by the
human partner on 2026-08-11**: the per-task review gate is a **Codex
adversarial review**, not a Claude task-reviewer subagent.

Per task N:

1. **Record the base.** `BASE=$(git rev-parse HEAD)` before dispatching
   anything. Never use `HEAD~1` as the base: a task that lands more than one
   commit would silently have all but the last dropped from review.
2. **Extract the brief.** `scripts/task-brief docs/superpowers/plans/2026-08-10-review-context.md N`,
   which prints a file path. The implementer reads that file; the plan is
   never pasted into a dispatch prompt.
3. **Dispatch one implementer subagent** with the brief path, a report-file
   path, and the interfaces from earlier tasks that the brief cannot know.
   One subagent at a time, never parallel: these tasks touch overlapping
   files (`model.rs`, `ingest.rs`, `merge.rs`, `payloads.rs`).
4. **Codex adversarial review of that task only:**

   ```bash
   node "$CODEX_COMPANION" adversarial-review --base "$BASE" <focus text>
   ```

   Scoping to `$BASE` is what makes this a per-task gate rather than a
   whole-branch review. A working-tree target reviews nothing once the
   implementer has committed, which is exactly how the first attempt at this
   returned a vacuous `approve` on an empty diff.
5. **Receive the review under `superpowers:receiving-code-review`.** Codex's
   output is a set of suggestions to evaluate, not orders to follow. For each
   finding: verify it against this codebase, check whether it breaks existing
   behaviour, check whether the plan or spec deliberately chose otherwise.
   Push back with technical reasoning where it is wrong. Dispatch a fix
   subagent only for findings that survive that check.
6. **A finding that contradicts the plan is the human's call.** Present the
   finding beside the plan text that mandates it and ask which governs. Do
   not let a fix silently overrule the plan, and do not dismiss a finding
   because the plan mandated the behaviour.
7. **Append one line to the ledger** at `.superpowers/sdd/progress.md`:
   `Task N: complete (commits <base7>..<head7>, codex review <verdict>)`.

**Codex has a conflict of interest here and it must be named.** This plan's
entire thesis is that a reviewing agent produces better findings when given
recorded session context. Codex is that reviewing agent. Its verdicts on the
tool built to feed it are not disinterested, and an `approve` from it is not
evidence the design works. Treat its findings as useful and its verdicts as
uninformative.

## Verification

After Task 16, before reporting completion:

- [ ] `cargo test --release` passes with zero failures. Record the count; it was 330 before this plan.
- [ ] `cargo clippy --all-targets -- -D warnings` is clean.
- [ ] `cargo build --release && ./target/release/sumcp context --file fixtures/demo/demo-session.jsonl` prints a `v: 3` payload.
- [ ] The MCP server lists eight tools: `python3` driving `./target/release/sumcp-mcp` over stdio, per the probe pattern in `crates/sumcp-mcp/tests/stdio.rs`.
- [ ] `python3 scripts/power_estimate.py` output is recorded in the spec.
- [ ] `docs/payload-schema.md` documents the two `v: 3` payloads. This is a doc the repo keeps current and the plan is not complete without it.

---

## Self-Review Notes

**Spec coverage.** All five extraction blocks (Tasks 5 to 9), both tools (Tasks 10, 11, 13), the CLI path (Task 14), git range resolution (Task 12), the recount gate (Task 15), the experiment with pre-declared metrics and kill criteria (Task 16), and the power estimate the spec demanded first (Task 1). The memory layer is correctly absent: the spec marks it deferred behind the precision result.

**Two known gaps, both deliberate and disclosed rather than silently dropped:**

1. **`commit_range` does not select sessions in the MCP path** (Task 13). The server has no repo path. The CLI does it properly. Disclosed in the tool description and in the spec's open questions about session-to-commit mapping, which remains unresolved.
2. **The spec's `claims` open question is unresolved by design.** Every prose block is reported. If 60 blocks per session proves unusable in the experiment, the selection rule becomes a real design problem, which is what the spec already says.

**Type consistency checked:** `Decision` (model, Task 2) vs `DecisionOut` (context, Task 6) are deliberately distinct types, one raw and one shaped for the payload. `Idx` is used in `FailingCommand` and `Constraint` and imported once in `context.rs`. `session_ix: u16` matches `Action::session_ix` throughout.
