# Subagent Flat-Merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ingest a session's subagent transcripts and flat-merge their actions into the main session's total order, so debriefs and queries stop being blind to delegated work (Checkpoint-D blind spot).

**Architecture:** `ingest_str` still parses one file. A new pure `merge_sessions` combines a main `Session` with N subagent `Session`s into one totally-ordered `Session`. A new `assemble::load_session` orchestrates the fs work (read main → discover subagent files per on-disk layout → read+validate each → merge). Both binaries call `load_session` in place of the bare single-file `ingest_str`. Three position/line-number findings gain same-lane guards so the merge can't manufacture cross-lane false findings.

**Tech Stack:** Rust (workspace: `sumcp-core`, `sumcp-cli`, `sumcp-mcp`), `serde_json`, `rmcp` (server), `tempfile` (tests). Contract checker is Python (`scripts/check_payloads.py`).

**Design source:** `docs/superpowers/specs/2026-07-20-subagent-flat-merge-design.md`.

## Global Constraints

- **Rust edition/toolchain:** existing workspace (Rust 1.97). Every task ends green on `cargo test --workspace` AND `cargo clippy --workspace -- -D warnings`.
- **Learner annotation:** Raphael is learning Rust — annotate new code heavily, explaining non-obvious constructs in plain-language comments (existing files model the density).
- **`sumcp-core` purity:** no I/O below `ingest` EXCEPT `locate.rs`/`assemble.rs`, which are core's designated filesystem-boundary modules. `merge.rs` stays pure (no I/O).
- **Never fail the session on a bad subagent:** any subagent-side failure is counted, never fatal to the main-lane analysis.
- **Resource caps (ADR A9(3)):** every file read is byte-bounded at `MAX_TRANSCRIPT_BYTES` (256 MiB) and rejects non-regular files; subagent file *count* is capped at `MAX_SUBAGENT_FILES = 64`.
- **TDD, red-first:** for non-trivial logic (merge ordering, lane guards, discovery, files_missing accounting) write the failing test, run it, watch it fail, then implement. Commit after each green step.
- **Total-order key (unchanged):** `(effective_ts, lane, line_no)`; `Lane::Main` sorts before every `Lane::Sub(id)`.

---

### Task 1: Model spawn records + rename the honesty counter

Replace the `subagent_spawns: u64` counter (which fed the `subagents_excluded` payload) with (a) `spawns: Vec<Spawn>` populated by ingest — carrying each spawn's `agentId` for legacy-layout discovery — and (b) `subagent_files_missing: u64`, the post-merge honesty counter. The payload flag renames in lockstep with its schema, mock, and contract checker so everything stays green.

**Files:**
- Modify: `crates/sumcp-core/src/model.rs` (add `Spawn`, swap two `Session` fields)
- Modify: `crates/sumcp-core/src/ingest.rs` (populate `spawns`, capture `agentId`, set `subagent_files_missing: 0`)
- Modify: `crates/sumcp-core/src/payloads.rs:74` (flag rename)
- Modify: `docs/payload-schema.md:88` (schema row rename)
- Modify: `fixtures/mock-payloads/session_overview.json:11` (mock field rename)
- Test: `crates/sumcp-core/src/ingest.rs` (existing `subagent_spawns_counted_task_list_tools_ignored` test updated)

**Interfaces:**
- Produces: `model::Spawn { pub agent_id: Option<String> }`; `Session.spawns: Vec<Spawn>`; `Session.subagent_files_missing: u64`. `Session.subagent_spawns` is REMOVED.

- [ ] **Step 1: Update the existing ingest test to the new shape (red)**

In `crates/sumcp-core/src/ingest.rs`, find the test `subagent_spawns_counted_task_list_tools_ignored` (around line 537). Replace its final assertion and add an `agent_id` capture assertion:

```rust
// was: assert_eq!(s.subagent_spawns, 2);
assert_eq!(s.spawns.len(), 2, "two Agent/Task spawns, task-list tools ignored");
// The paired tool_result carried an agentId for the first spawn only.
assert_eq!(s.spawns[0].agent_id.as_deref(), Some("agent-abc"));
```

Then, in that test's transcript-line setup, make the first `Agent` spawn's result carry an agentId. Find where the test builds its lines and add a matching `tool_result` line whose `toolUseResult` includes `"agentId":"agent-abc"` and whose `tool_use_id` matches the first `Agent` call's id. (If the test currently has no result lines, add one line:)

```rust
// A tool_result for the first Agent spawn, carrying the agentId the
// legacy-layout discovery links on.
r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"a1","is_error":false}]},"toolUseResult":{"agentId":"agent-abc"}}"#,
```

(Match `"a1"` to whatever id the first `call("a1","Agent")` helper uses.)

- [ ] **Step 2: Run it — expect a compile error**

Run: `cargo test -p sumcp-core subagent_spawns_counted --no-run`
Expected: FAIL — `no field spawns on type Session`, `no field subagent_files_missing`.

- [ ] **Step 3: Add the `Spawn` type and swap the `Session` fields**

In `crates/sumcp-core/src/model.rs`, add above `Session` (near the `Lane` enum):

```rust
/// One subagent spawn recorded in the MAIN transcript (an `Agent`/`Task`
/// tool call). We keep the spawn's `agentId` because the legacy on-disk
/// layout names the child transcript `agent-<agentId>.jsonl` — that id is
/// the only link from a spawn to its file. `None` when the spawn's result
/// had not come back yet (subagent still running) or carried no agentId.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawn {
    /// The child agent's id, from the spawn's `toolUseResult.agentId`.
    pub agent_id: Option<String>,
}
```

In `Session` (around line 276), delete the `subagent_spawns` field and its doc comment, and add:

```rust
    /// This session's direct subagent spawns (`Agent`/`Task` calls in the
    /// MAIN transcript), post-dedup. Used by assembly to find and merge the
    /// child transcripts; carried through the merge as provenance.
    pub spawns: Vec<Spawn>,
    /// Subagent spawns whose transcript could not be turned into analyzed
    /// actions (file not found / unreadable / oversized / parsed to zero
    /// actions / over the file-count cap). Honest scope disclosure, surfaced
    /// as `subagent_files_missing` in `session_overview`. `0` from a bare
    /// `ingest_str`; set by `merge_sessions` from the assembly's count.
    pub subagent_files_missing: u64,
```

Update the `main_lane_sorts_before_subagents` test region if it constructs a `Session` literal (it does not — it only sorts `Lane`s, so no change).

- [ ] **Step 4: Populate the new fields in `ingest.rs`**

In `crates/sumcp-core/src/ingest.rs`:

(a) Replace the `subagent_spawns` counter declaration (line ~41):

```rust
    // Tool-use ids of Agent/Task spawns, in first-seen order (post-dedup).
    // Resolved to agentIds after the results map is built.
    let mut spawn_ids: Vec<String> = Vec::new();
```

(b) In the `tool_use` branch where `name == "Agent" || name == "Task"` (line ~131), replace `subagent_spawns += 1;` with:

```rust
                        if name == "Agent" || name == "Task" {
                            // Record the spawn's own tool_use id; we resolve
                            // its agentId from the paired result below.
                            if let Some(id) = tool_id {
                                spawn_ids.push(id.to_string());
                            }
                        }
```

(c) Capture `agentId` in `ResultInfo`. Find the `ResultInfo` struct and its construction (in the `tool_result` branch, ~line 218). Add a field `agent_id: Option<String>` to the struct definition, and populate it where the result is built:

```rust
                            let agent_id = v
                                .get("toolUseResult")
                                .and_then(|r| r.get("agentId"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
```

Add `agent_id,` to the `ResultInfo { .. }` literal.

(d) After the actions are built and before the `Session { .. }` literal (line ~287), resolve spawns:

```rust
    // Resolve each spawn's agentId from its paired result (if the result had
    // come back and carried one). Order preserved from first-seen.
    let spawns: Vec<Spawn> = spawn_ids
        .iter()
        .map(|id| Spawn {
            agent_id: results.get(id).and_then(|r| r.agent_id.clone()),
        })
        .collect();
```

(e) In the `Session { .. }` literal, replace `subagent_spawns,` with:

```rust
        spawns,
        subagent_files_missing: 0,
```

(f) Add `Spawn` to the imports at the top: `use crate::model::{Action, ActionKind, Idx, Lane, Session, Spawn, Tokens, UserText};`

- [ ] **Step 5: Run the ingest test — expect green**

Run: `cargo test -p sumcp-core subagent_spawns_counted -- --nocapture`
Expected: PASS.

- [ ] **Step 6: Rename the payload flag**

In `crates/sumcp-core/src/payloads.rs` (line ~72), replace:

```rust
            // Honest scope disclosure (T4.2): spawns whose subagent work we
            // did not analyze. Goes away when the flat-merge lands.
            "subagents_excluded": s.subagent_spawns
```

with:

```rust
            // Honest scope disclosure: subagent spawns we could not turn into
            // analyzed actions (missing/unreadable/empty child transcript, or
            // over the file-count cap). `0` once every spawn's work merged in.
            "subagent_files_missing": s.subagent_files_missing
```

- [ ] **Step 7: Update schema doc + mock fixture**

In `docs/payload-schema.md`, replace the `subagents_excluded` row (line 88) with:

```
| `session_overview.flags` | `subagent_files_missing` | count of subagent spawns whose child transcript could not be analyzed — not found, unreadable, oversized, parsed to zero actions, or beyond the 64-file cap. `0` when every spawn's work was merged in (the common case) and when no subagents ran. Replaces the pre-merge `subagents_excluded` counter. |
```

In `fixtures/mock-payloads/session_overview.json`, change `"subagents_excluded":2` to `"subagent_files_missing":0` (the mock now represents a fully-merged session).

- [ ] **Step 8: Verify workspace + contract checker green, then commit**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && python3 scripts/check_payloads.py`
Expected: all PASS (the checker does not key on the flag name, so the rename is safe; run it to confirm).

```bash
git add crates/sumcp-core/src/model.rs crates/sumcp-core/src/ingest.rs crates/sumcp-core/src/payloads.rs docs/payload-schema.md fixtures/mock-payloads/session_overview.json
git commit -m "T5.0a: spawn records + subagent_files_missing counter (replaces subagents_excluded)"
```

---

### Task 2: `merge_sessions` pure function

The heart: combine a main `Session` with N subagent `Session`s into one totally-ordered `Session`. Pure, no I/O.

**Files:**
- Create: `crates/sumcp-core/src/merge.rs`
- Modify: `crates/sumcp-core/src/lib.rs` (add `pub mod merge;`)
- Test: inline `#[cfg(test)]` module in `merge.rs`

**Interfaces:**
- Consumes: `model::{Session, Action, Idx, Lane}` from Task 1.
- Produces: `merge::merge_sessions(main: Session, subs: Vec<Session>, files_missing: u64) -> Session`.

- [ ] **Step 1: Write the failing tests**

Create `crates/sumcp-core/src/merge.rs` with a test module first:

```rust
//! Flat-merge of a main session with its subagent sessions (SPEC decision 2).
//!
//! Pure: takes already-parsed `Session`s and produces one merged `Session`.
//! All filesystem work (finding and reading the child transcripts) lives in
//! `assemble.rs`; this module only combines what it is handed.

use crate::model::{Action, Idx, Lane, Session};

// (implementation goes here in Step 3)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActionKind, Spawn};

    /// Build a minimal one-action Session in a given lane at a given timestamp.
    fn one(lane: Lane, ts: &str, line_no: usize, file: &str) -> Session {
        Session {
            actions: vec![Action {
                idx: Idx(0),
                effective_ts: ts.to_string(),
                ts_inherited: false,
                lane,
                line_no,
                kind: ActionKind::Edit,
                file_path: Some(file.to_string()),
                is_error: None,
                write_len: None,
                write_lines: None,
                read_total_lines: None,
                input_hash: None,
                error: None,
                hunks: vec![],
                command: None,
                user_modified: false,
                edit_old: None,
                edit_new: None,
                approval_latency_s: None,
            }],
            user_texts: vec![],
            tokens: Default::default(),
            type_counts: Default::default(),
            parse_errors: 0,
            untimestamped_lines: 0,
            interrupts: 0,
            auto_accept: false,
            spawns: vec![],
            subagent_files_missing: 0,
        }
    }

    #[test]
    fn merged_idx_is_contiguous_and_ordered() {
        // main action at 00:02, sub action at 00:01 → sub sorts first.
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.spawns = vec![Spawn { agent_id: Some("x".into()) }];
        let sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");

        let merged = merge_sessions(main, vec![sub], 0);

        // Two actions, Idx re-numbered 0,1 in total order.
        assert_eq!(merged.actions.len(), 2);
        assert_eq!(merged.actions[0].idx, Idx(0));
        assert_eq!(merged.actions[1].idx, Idx(1));
        // Earlier timestamp (the sub) comes first.
        assert_eq!(merged.actions[0].lane, Lane::Sub("x".into()));
        assert_eq!(merged.actions[1].lane, Lane::Main);
        // Invariant every payload relies on: actions[i].idx == Idx(i).
        for (i, a) in merged.actions.iter().enumerate() {
            assert_eq!(a.idx, Idx(i as u32));
        }
    }

    #[test]
    fn main_first_on_timestamp_tie() {
        // Identical timestamps → Lane tie-break puts Main first.
        let main = one(Lane::Main, "2026-01-01T00:00:01Z", 5, "/a");
        let sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        let merged = merge_sessions(main, vec![sub], 0);
        assert_eq!(merged.actions[0].lane, Lane::Main);
    }

    #[test]
    fn keeps_only_main_user_texts_and_ors_nothing_for_auto_accept() {
        use crate::model::UserText;
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.user_texts = vec![UserText { line_no: 1, text: "human says".into(), effective_ts: "2026-01-01T00:00:00Z".into() }];
        main.auto_accept = false;
        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.user_texts = vec![UserText { line_no: 1, text: "orchestrator prompt".into(), effective_ts: "2026-01-01T00:00:00Z".into() }];
        sub.auto_accept = true; // a sub in auto-accept must NOT flip the merged flag

        let merged = merge_sessions(main, vec![sub], 0);
        assert_eq!(merged.user_texts.len(), 1);
        assert_eq!(merged.user_texts[0].text, "human says");
        assert!(!merged.auto_accept, "sub auto-accept must not suppress main latency");
    }

    #[test]
    fn counters_sum_and_files_missing_passthrough() {
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.parse_errors = 1;
        main.untimestamped_lines = 2;
        main.interrupts = 1;
        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.parse_errors = 3;
        sub.untimestamped_lines = 4;

        let merged = merge_sessions(main, vec![sub], 7);
        assert_eq!(merged.parse_errors, 4);
        assert_eq!(merged.untimestamped_lines, 6);
        assert_eq!(merged.interrupts, 1);
        assert_eq!(merged.subagent_files_missing, 7);
    }

    #[test]
    fn determinism_independent_of_subs_order() {
        let main = one(Lane::Main, "2026-01-01T00:00:03Z", 5, "/a");
        let s1 = one(Lane::Sub("a".into()), "2026-01-01T00:00:01Z", 1, "/b");
        let s2 = one(Lane::Sub("b".into()), "2026-01-01T00:00:02Z", 1, "/c");

        let m1 = merge_sessions(main.clone(), vec![s1.clone(), s2.clone()], 0);
        let m2 = merge_sessions(main, vec![s2, s1], 0);
        let lanes1: Vec<_> = m1.actions.iter().map(|a| a.lane.clone()).collect();
        let lanes2: Vec<_> = m2.actions.iter().map(|a| a.lane.clone()).collect();
        assert_eq!(lanes1, lanes2);
    }

    #[test]
    fn empty_sub_contributes_nothing() {
        let main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        let empty = Session {
            actions: vec![], user_texts: vec![], tokens: Default::default(),
            type_counts: Default::default(), parse_errors: 0, untimestamped_lines: 0,
            interrupts: 0, auto_accept: false, spawns: vec![], subagent_files_missing: 0,
        };
        let merged = merge_sessions(main, vec![empty], 1);
        assert_eq!(merged.actions.len(), 1);
        assert_eq!(merged.subagent_files_missing, 1);
    }
}
```

Note: if `UserText`'s fields differ from `{ line_no, text, effective_ts }`, open `model.rs`, read the real `UserText` definition, and adjust the test constructors to match before running.

- [ ] **Step 2: Register the module and run — expect failure**

Add to `crates/sumcp-core/src/lib.rs` after `pub mod locate;`:

```rust
pub mod merge;
```

Run: `cargo test -p sumcp-core --lib merge::`
Expected: FAIL — `cannot find function merge_sessions`.

- [ ] **Step 3: Implement `merge_sessions`**

In `merge.rs`, replace the `// (implementation goes here in Step 3)` line with:

```rust
/// Merge a main session with its subagent sessions into one totally-ordered
/// `Session`. `files_missing` is computed by the caller (assembly) and stored
/// verbatim; this function does no filesystem work.
pub fn merge_sessions(main: Session, subs: Vec<Session>, files_missing: u64) -> Session {
    // Start from main's counters and user_texts; main is privileged.
    let mut actions: Vec<Action> = main.actions;
    let user_texts = main.user_texts; // subagent user turns are dropped
    let mut tokens = main.tokens;
    let mut type_counts = main.type_counts;
    let mut parse_errors = main.parse_errors;
    let mut untimestamped_lines = main.untimestamped_lines;
    let mut interrupts = main.interrupts;
    let auto_accept = main.auto_accept; // NOT OR'd — see spec §5
    let spawns = main.spawns;

    // Fold every subagent's actions and additive counters in.
    for sub in subs {
        actions.extend(sub.actions);
        tokens.input += sub.tokens.input;
        tokens.output += sub.tokens.output;
        tokens.cache_read += sub.tokens.cache_read;
        tokens.cache_creation += sub.tokens.cache_creation;
        for (t, n) in sub.type_counts {
            *type_counts.entry(t).or_insert(0) += n;
        }
        parse_errors += sub.parse_errors;
        untimestamped_lines += sub.untimestamped_lines;
        interrupts += sub.interrupts;
        // sub.user_texts, sub.auto_accept, sub.spawns intentionally ignored.
    }

    // Total-order sort (same key as ingest): timestamp, then lane (Main first),
    // then source line number. `sort_by` is stable, but the key is already
    // total so stability is not load-bearing.
    actions.sort_by(|a, b| {
        (&a.effective_ts, &a.lane, a.line_no).cmp(&(&b.effective_ts, &b.lane, b.line_no))
    });

    // Re-number Idx so actions[i].idx == Idx(i) across the merged whole. This
    // is the invariant every payload and evidence() depends on — pre-merge
    // Idx values are meaningless after interleaving.
    for (i, a) in actions.iter_mut().enumerate() {
        a.idx = Idx(i as u32);
    }

    Session {
        actions,
        user_texts,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines,
        interrupts,
        auto_accept,
        spawns,
        subagent_files_missing: files_missing,
    }
}
```

- [ ] **Step 4: Run merge tests — expect green**

Run: `cargo test -p sumcp-core --lib merge::`
Expected: PASS (all six).

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/merge.rs crates/sumcp-core/src/lib.rs
git commit -m "T5.0b: merge_sessions — pure flat-merge with Idx re-numbering"
```

---

### Task 3: Lane-scope the position/line-number findings

Add same-lane guards so `true_revert`/`flip` and failure-attribution proximity compare only within one lane. Without this the merge manufactures cross-lane false findings (spec §5a).

**Files:**
- Modify: `crates/sumcp-core/src/signals/dynamics.rs` (revert match: add lane guard)
- Modify: `crates/sumcp-core/src/signals/failures.rs` (proximity window: filter to same lane)
- Test: inline tests in each file

**Interfaces:**
- Consumes: `model::Lane` (already used in both files).

- [ ] **Step 1: Write the failing lane-scoping tests**

In `crates/sumcp-core/src/signals/dynamics.rs`, inside its `#[cfg(test)]` module, add:

```rust
    #[test]
    fn cross_lane_revert_is_not_flagged() {
        // A main edit and a subagent edit on the same path, where the sub's
        // new_string restores the main's old_string. Different actors → not a
        // revert. Same content within one lane WOULD fire (asserted below).
        use crate::model::{Action, ActionKind, Idx, Lane};
        let mk = |idx, lane, ts: &str, line, old: &str, new: &str| Action {
            idx: Idx(idx), effective_ts: ts.into(), ts_inherited: false, lane,
            line_no: line, kind: ActionKind::Edit, file_path: Some("/a".into()),
            is_error: None, write_len: None, write_lines: None, read_total_lines: None,
            input_hash: None, error: None, hunks: vec![], command: None,
            user_modified: false, edit_old: Some(old.into()), edit_new: Some(new.into()),
            approval_latency_s: None,
        };
        let mut s = crate::model::Session {
            actions: vec![
                mk(0, Lane::Main, "2026-01-01T00:00:01Z", 1, "foo", "bar"),
                mk(1, Lane::Sub("x".into()), "2026-01-01T00:00:02Z", 1, "bar", "foo"),
            ],
            user_texts: vec![], tokens: Default::default(), type_counts: Default::default(),
            parse_errors: 0, untimestamped_lines: 0, interrupts: 0, auto_accept: false,
            spawns: vec![], subagent_files_missing: 0,
        };
        assert!(reverts_and_flips(&s).is_empty(), "cross-lane pair must not be a revert");

        // Same pair, both main lane → a true_revert fires.
        s.actions[1].lane = Lane::Main;
        assert_eq!(reverts_and_flips(&s).len(), 1, "same-lane revert must fire");
    }
```

In `crates/sumcp-core/src/signals/failures.rs`, inside its `#[cfg(test)]` module, add:

```rust
    #[test]
    fn proximity_does_not_attribute_across_lanes() {
        // A main-lane failing Bash whose nearest prior Edit is in a SUB lane
        // must not be attributed to it; a same-lane prior edit is required.
        use crate::model::{Action, ActionKind, Idx, Lane, Session};
        let edit = |idx, lane, file: &str| Action {
            idx: Idx(idx), effective_ts: "2026-01-01T00:00:01Z".into(), ts_inherited: false,
            lane, line_no: idx as usize, kind: ActionKind::Edit, file_path: Some(file.into()),
            is_error: None, write_len: None, write_lines: None, read_total_lines: None,
            input_hash: None, error: None, hunks: vec![], command: None, user_modified: false,
            edit_old: None, edit_new: None, approval_latency_s: None,
        };
        let mut bash = edit(1, Lane::Main, "/x");
        bash.kind = ActionKind::Bash;
        bash.command = Some("run".into());
        bash.is_error = Some(true);
        bash.file_path = None;

        // Only prior edit is in a sub lane → must fall through to unattributed.
        let s = Session {
            actions: vec![edit(0, Lane::Sub("z".into()), "/sub-file"), bash.clone()],
            user_texts: vec![], tokens: Default::default(), type_counts: Default::default(),
            parse_errors: 0, untimestamped_lines: 0, interrupts: 0, auto_accept: false,
            spawns: vec![], subagent_files_missing: 0,
        };
        let f = failures(&s);
        // No path match, no same-lane prior edit → the failure is unattributed,
        // i.e. carries no file.
        assert!(f.iter().all(|x| x.file.is_none()), "must not attribute to sub-lane edit");
    }
```

- [ ] **Step 2: Run both — expect failure**

Run: `cargo test -p sumcp-core --lib cross_lane_revert_is_not_flagged proximity_does_not_attribute_across_lanes`
Expected: FAIL — cross-lane revert currently fires; sub-lane edit currently attributed.

- [ ] **Step 3: Add the lane guard in `dynamics.rs`**

In `reverts_and_flips` (around line 250), change the revert match condition:

```rust
            // same LANE, same file, and the later edit puts back what the
            // earlier removed. Lane guard (spec §5a): a revert is one actor
            // undoing its own change — a cross-lane content coincidence on a
            // shared path is not a revert, and a subagent edit's line_no
            // indexes a different file than the main-lane pushback stream.
            if earlier.lane == later.lane
                && earlier.file_path == later.file_path
                && later.edit_new == earlier.edit_old
            {
```

- [ ] **Step 4: Add the lane filter in `failures.rs`**

In `attribute` (Step 3 of the chain, around line 108), filter the proximity slice to the failing action's lane:

```rust
    // Step 3: the most recent Edit/Write within PROXIMITY_WINDOW actions back,
    // IN THE SAME LANE (spec §5a). A failure in one lane is only ever caused by
    // an edit in that same lane; cross-lane adjacency in the merged order is
    // coincidental. Trade-off: heavy interleaving narrows the effective window.
    let start = pos.saturating_sub(PROXIMITY_WINDOW);
    if let Some(prev) = s.actions[start..pos]
        .iter()
        .rev()
        .find(|p| {
            p.lane == a.lane
                && matches!(p.kind, ActionKind::Edit | ActionKind::Write)
                && p.file_path.is_some()
        })
    {
        return Some((prev.file_path.clone().unwrap(), Attribution::Proximity));
    }
```

- [ ] **Step 5: Run the new tests + full suite — expect green**

Run: `cargo test -p sumcp-core --lib && cargo clippy -p sumcp-core -- -D warnings`
Expected: PASS, including the pre-existing revert/flip/attribution tests (single-lane behavior unchanged).

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/signals/dynamics.rs crates/sumcp-core/src/signals/failures.rs
git commit -m "T5.0c: lane-scope true_revert/flip and attribution proximity"
```

---

### Task 4: Subagent file discovery in `locate.rs`

Given the main transcript path and its spawns, return safety-checked candidate subagent file paths — layout-aware, `is_within`-guarded, count-capped. No content reading here.

**Files:**
- Modify: `crates/sumcp-core/src/locate.rs` (add discovery fns + `MAX_SUBAGENT_FILES`)
- Test: inline tests in `locate.rs` (use `tempfile`)

**Interfaces:**
- Consumes: `model::Spawn` (Task 1).
- Produces:
  - `locate::MAX_SUBAGENT_FILES: usize` (= 64)
  - `locate::subagents_dir(main_path: &Path) -> PathBuf`
  - `locate::discover_subagent_paths(main_path: &Path, spawns: &[Spawn]) -> Vec<PathBuf>`

- [ ] **Step 1: Confirm `tempfile` is a dev-dependency**

Run: `grep -n "tempfile" crates/sumcp-core/Cargo.toml`
Expected: a line under `[dev-dependencies]`. If absent, add `tempfile = "3"` under `[dev-dependencies]` in `crates/sumcp-core/Cargo.toml` (the mcp crate already uses it, so it is in the lockfile).

- [ ] **Step 2: Write the failing discovery tests**

In `crates/sumcp-core/src/locate.rs`, inside the `#[cfg(test)]` module, add:

```rust
    use crate::model::Spawn;

    #[test]
    fn discovers_2_1_x_subagents_dir() {
        // Layout: <dir>/<uuid>.jsonl (main) + <dir>/<uuid>/subagents/agent-*.jsonl
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("agent-aaa.jsonl"), "{}").unwrap();
        std::fs::write(subs.join("agent-bbb.jsonl"), "{}").unwrap();
        std::fs::write(subs.join("notes.txt"), "ignore me").unwrap();

        let found = discover_subagent_paths(&main, &[]);
        assert_eq!(found.len(), 2, "two agent-*.jsonl, notes.txt ignored");
    }

    #[test]
    fn discovers_legacy_siblings_by_spawn_agent_id() {
        // Layout: <dir>/<uuid>.jsonl (main) + <dir>/agent-<id>.jsonl (siblings),
        // no <uuid>/subagents dir → legacy path.
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        std::fs::write(td.path().join("agent-present.jsonl"), "{}").unwrap();
        // A decoy sibling for a DIFFERENT session's agent — must not be found.
        std::fs::write(td.path().join("agent-decoy.jsonl"), "{}").unwrap();

        let spawns = vec![
            Spawn { agent_id: Some("present".into()) },
            Spawn { agent_id: Some("absent".into()) }, // file does not exist
            Spawn { agent_id: None },                  // unresolved, skipped
        ];
        let found = discover_subagent_paths(&main, &spawns);
        assert_eq!(found.len(), 1, "only the spawn-linked, existing sibling");
        assert!(found[0].ends_with("agent-present.jsonl"));
    }

    #[test]
    fn file_count_is_capped() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        for i in 0..(MAX_SUBAGENT_FILES + 10) {
            std::fs::write(subs.join(format!("agent-{i:03}.jsonl")), "{}").unwrap();
        }
        let found = discover_subagent_paths(&main, &[]);
        assert_eq!(found.len(), MAX_SUBAGENT_FILES, "capped");
    }
```

- [ ] **Step 3: Run — expect failure**

Run: `cargo test -p sumcp-core --lib locate::tests::discovers`
Expected: FAIL — `cannot find function discover_subagent_paths`.

- [ ] **Step 4: Implement discovery**

In `crates/sumcp-core/src/locate.rs`, add near the top (after the existing `use`):

```rust
use crate::model::Spawn;

/// Cap on subagent files merged for one session (ADR A9(3)). ~5× the largest
/// real spawn count observed (12 on the donor); the rest count as missing.
pub const MAX_SUBAGENT_FILES: usize = 64;
```

Then add these functions (after `is_within`):

```rust
/// The 2.1.x subagents directory for a main transcript: `<dir>/<stem>/subagents`,
/// where `<stem>` is the main file's name without `.jsonl` (the session uuid).
pub fn subagents_dir(main_path: &Path) -> PathBuf {
    let stem = main_path.file_stem().unwrap_or_default();
    let parent = main_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(stem).join("subagents")
}

/// The legacy sibling transcript path for a given agent id:
/// `<dir>/agent-<agentId>.jsonl` next to the main transcript.
fn legacy_sibling(main_path: &Path, agent_id: &str) -> PathBuf {
    let parent = main_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("agent-{agent_id}.jsonl"))
}

/// Discover this session's subagent transcript files, safety-checked and
/// count-capped. Layout is auto-detected: if the 2.1.x `subagents/` directory
/// exists we list it; otherwise we resolve legacy siblings from the spawns'
/// agent ids. Returns only existing regular-file paths that resolve INSIDE the
/// session's own directory tree (ADR A9 symlink/`..` guard).
pub fn discover_subagent_paths(main_path: &Path, spawns: &[Spawn]) -> Vec<PathBuf> {
    let dir = subagents_dir(main_path);
    let mut out: Vec<PathBuf> = if dir.is_dir() {
        // 2.1.x: list agent-*.jsonl in the session-namespaced directory. The
        // directory itself guarantees these belong to this session, so no
        // spawn-linking is needed here (content validation happens at read).
        let root = main_path.parent().unwrap_or_else(|| Path::new("."));
        let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| is_agent_jsonl(p))
                    .filter(|p| is_within(root, p))
                    .collect()
            })
            .unwrap_or_default();
        // Deterministic order regardless of filesystem enumeration.
        v.sort();
        v
    } else {
        // Legacy: resolve exactly the siblings our own spawns name. Never list
        // the shared project dir — that would false-merge other sessions.
        let root = main_path.parent().unwrap_or_else(|| Path::new("."));
        spawns
            .iter()
            .filter_map(|s| s.agent_id.as_deref())
            .map(|id| legacy_sibling(main_path, id))
            .filter(|p| p.is_file() && is_within(root, p))
            .collect()
    };
    out.truncate(MAX_SUBAGENT_FILES);
    out
}

/// True for a regular file named `agent-*.jsonl`.
fn is_agent_jsonl(p: &Path) -> bool {
    p.is_file()
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
}
```

Note on `is_within`: it canonicalizes both paths. In tests the tempdir is real, so canonicalization succeeds. Confirm the existing `is_within` returns `true` for these in-tree paths when you run Step 5; if the tempdir symlinks (macOS `/tmp` → `/private/tmp`) trip it, canonicalize `root` from `main_path.parent()` (already real) — it should pass.

- [ ] **Step 5: Run — expect green**

Run: `cargo test -p sumcp-core --lib locate:: && cargo clippy -p sumcp-core -- -D warnings`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/locate.rs crates/sumcp-core/Cargo.toml
git commit -m "T5.0d: subagent file discovery (2.1.x dir + legacy siblings, capped)"
```

---

### Task 5: Assembly entry point (`assemble.rs`)

Orchestrate the full pipeline: read main → ingest → discover → read+validate+ingest each subagent → merge, computing `files_missing`. This is the single entry point both binaries call.

**Files:**
- Create: `crates/sumcp-core/src/assemble.rs`
- Modify: `crates/sumcp-core/src/lib.rs` (add `pub mod assemble;`)
- Test: inline tests (tempdir fixtures for both layouts + decoy + missing)

**Interfaces:**
- Consumes: `ingest::ingest_str`, `locate::discover_subagent_paths`, `merge::merge_sessions`, `model::{Session, Lane}`.
- Produces:
  - `assemble::MAX_TRANSCRIPT_BYTES: u64` (= 256 MiB)
  - `assemble::Assembled { pub session: Session, pub subagent_paths: Vec<std::path::PathBuf> }`
  - `assemble::load_session(main_path: &Path, max_bytes: u64) -> std::io::Result<Assembled>`

- [ ] **Step 1: Write the failing assembly tests**

Create `crates/sumcp-core/src/assemble.rs` with a test module:

```rust
//! Assembly: the filesystem-facing entry point that turns a main transcript
//! path into a fully merged `Session` (main + subagents). Both binaries call
//! `load_session`; it is the only place that reads subagent files.

use crate::ingest::ingest_str;
use crate::locate::{discover_subagent_paths, is_within_or_root};
use crate::merge::merge_sessions;
use crate::model::{Lane, Session};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Hard ceiling on any single transcript we read (ADR A9(3)).
pub const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;

// (implementation in Step 3)

#[cfg(test)]
mod tests {
    use super::*;

    fn agent_spawn_lines(id: &str, agent_id: &str) -> String {
        // A main-transcript Agent spawn whose result carries an agentId.
        format!(
            "{}\n{}",
            format!(r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Agent","input":{{"subagent_type":"x"}}}}]}}}}"#),
            format!(r#"{{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}},"toolUseResult":{{"agentId":"{agent_id}"}}}}"#),
        )
    }

    /// A subagent transcript line: one Edit, carrying a parent sessionId.
    fn sub_edit_line(parent: &str) -> String {
        format!(r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","sessionId":"{parent}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"/sub.rs","old_string":"a","new_string":"b"}}}}]}}}}"#)
    }

    #[test]
    fn legacy_layout_merges_present_counts_missing() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        // two spawns: "present" has a sibling file, "absent" does not.
        std::fs::write(
            &main,
            format!("{}\n{}", agent_spawn_lines("a1", "present"), agent_spawn_lines("a2", "absent")),
        )
        .unwrap();
        std::fs::write(td.path().join("agent-present.jsonl"), sub_edit_line(uuid)).unwrap();
        // decoy sibling for a different session — not named by any spawn.
        std::fs::write(td.path().join("agent-decoy.jsonl"), sub_edit_line("other")).unwrap();

        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        // The present subagent's Edit merged in (a sub-lane action exists).
        assert!(a.session.actions.iter().any(|x| matches!(x.lane, Lane::Sub(_))));
        // One spawn ("absent") could not be resolved.
        assert_eq!(a.session.subagent_files_missing, 1);
        // The decoy was never read.
        assert!(a.subagent_paths.iter().all(|p| !p.ends_with("agent-decoy.jsonl")));
    }

    #[test]
    fn v2_1_x_rejects_wrong_session_id() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, agent_spawn_lines("a1", "present")).unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        // one file belongs to this session, one carries a wrong sessionId.
        std::fs::write(subs.join("agent-ok.jsonl"), sub_edit_line(uuid)).unwrap();
        std::fs::write(subs.join("agent-wrong.jsonl"), sub_edit_line("SOMEONE-ELSE")).unwrap();

        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        let sub_actions = a.session.actions.iter().filter(|x| matches!(x.lane, Lane::Sub(_))).count();
        assert_eq!(sub_actions, 1, "only the matching-sessionId file merges");
        assert_eq!(a.session.subagent_files_missing, 1, "wrong-sessionId file counts missing");
    }

    #[test]
    fn no_subagents_is_main_only() {
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("flat.jsonl");
        std::fs::write(
            &main,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","new_string":"x"}}]}}"#,
        )
        .unwrap();
        let a = load_session(&main, MAX_TRANSCRIPT_BYTES).unwrap();
        assert_eq!(a.session.subagent_files_missing, 0);
        assert!(a.subagent_paths.is_empty());
    }
}
```

- [ ] **Step 2: Register the module + expose an `is_within` helper; run — expect failure**

Add to `crates/sumcp-core/src/lib.rs` after `pub mod merge;`:

```rust
pub mod assemble;
```

The test imports `is_within_or_root` — we will add a small public wrapper in `locate.rs` in Step 3 so assembly can reuse the guard. For now run:

Run: `cargo test -p sumcp-core --lib assemble:: --no-run`
Expected: FAIL — `cannot find function load_session` (and `is_within_or_root`).

- [ ] **Step 3: Implement assembly**

First, in `locate.rs`, add a public convenience used by assembly (keeps the canonicalize guard in one module):

```rust
/// True if `candidate` is inside `main_path`'s parent directory tree — the
/// guard assembly applies before reading any discovered subagent file.
pub fn is_within_or_root(main_path: &Path, candidate: &Path) -> bool {
    let root = main_path.parent().unwrap_or_else(|| Path::new("."));
    is_within(root, candidate)
}
```

Then, in `assemble.rs`, replace `// (implementation in Step 3)` with:

```rust
/// The outcome of assembly: the merged session plus the subagent files that
/// were actually read (the MCP store keys cache freshness on these).
pub struct Assembled {
    /// The fully merged session (main + subagents).
    pub session: Session,
    /// Absolute paths of the subagent transcripts that were read and merged.
    pub subagent_paths: Vec<PathBuf>,
}

/// Read a file up to `max_bytes`, rejecting non-regular files (ADR A9(3)).
/// Returns `Ok(None)` on any I/O problem — callers treat that as "missing".
fn read_bounded(path: &Path, max_bytes: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max_bytes {
        return None;
    }
    let mut raw = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(max_bytes + 1)
        .read_to_string(&mut raw)
        .ok()?;
    if raw.len() as u64 > max_bytes {
        return None;
    }
    Some(raw)
}

/// Extract the first `sessionId` value seen in a transcript, if any. Used to
/// validate a 2.1.x subagent file belongs to the expected parent session.
fn first_session_id(raw: &str) -> Option<String> {
    for line in raw.lines() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(id) = v.get("sessionId").and_then(|x| x.as_str()) {
                return Some(id.to_string());
            }
        }
    }
    None
}

/// Turn a main transcript path into a fully merged `Session`. Reads the main
/// file, discovers and reads subagent files, validates ownership, merges. Only
/// a failure to read the MAIN file is an error; subagent failures are counted
/// in `subagent_files_missing`, never fatal.
pub fn load_session(main_path: &Path, max_bytes: u64) -> std::io::Result<Assembled> {
    let raw = read_bounded(main_path, max_bytes)
        .ok_or_else(|| std::io::Error::other("main transcript unreadable or over size ceiling"))?;
    let main = ingest_str(&raw, Lane::Main);

    // The expected parent session id: the main file's stem (its uuid).
    let expected_id = main_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    let candidates = discover_subagent_paths(main_path, &main.spawns);
    let is_2_1_x = crate::locate::subagents_dir(main_path).is_dir();

    let mut subs: Vec<Session> = Vec::new();
    let mut read_paths: Vec<PathBuf> = Vec::new();
    // Count of successfully merged, non-empty subagent transcripts.
    let mut merged_ok = 0usize;

    for path in candidates {
        if !is_within_or_root(main_path, &path) {
            continue; // safety guard (also applied in discovery; belt-and-suspenders)
        }
        let Some(sub_raw) = read_bounded(&path, max_bytes) else { continue };
        // 2.1.x ownership check: reject a file whose sessionId is present AND
        // mismatched. Absent sessionId is accepted (the namespaced directory is
        // the primary ownership guarantee; the field's exact shape is not yet
        // verified against a real 2.1.x session — see spec open item #1).
        if is_2_1_x {
            if let Some(sid) = first_session_id(&sub_raw) {
                if sid != expected_id {
                    continue; // belongs to another session — count as missing below
                }
            }
        }
        // Agent id from filename `agent-<id>.jsonl` for the lane label.
        let agent_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("agent-"))
            .unwrap_or("unknown")
            .to_string();
        let sub = ingest_str(&sub_raw, Lane::Sub(agent_id));
        if sub.actions.is_empty() {
            continue; // resolved but nothing usable — counts as missing
        }
        merged_ok += 1;
        subs.push(sub);
        read_paths.push(path);
    }

    // files_missing: spawns we could not turn into analyzed actions. Floored at
    // zero (a version/streaming artifact could surface more files than spawns).
    let files_missing = (main.spawns.len()).saturating_sub(merged_ok) as u64;

    let session = merge_sessions(main, subs, files_missing);
    Ok(Assembled { session, subagent_paths: read_paths })
}
```

- [ ] **Step 4: Run assembly tests — expect green**

Run: `cargo test -p sumcp-core --lib assemble:: && cargo clippy -p sumcp-core -- -D warnings`
Expected: PASS (three tests).

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/assemble.rs crates/sumcp-core/src/locate.rs crates/sumcp-core/src/lib.rs
git commit -m "T5.0e: assemble::load_session — discover, validate, merge subagents"
```

---

### Task 6: Wire assembly into the MCP store + multi-file cache freshness

Replace `store.rs`'s single-file `ingest_str` with `assemble::load_session`, and extend the freshness key so an appended subagent file re-parses.

**Files:**
- Modify: `crates/sumcp-mcp/src/store.rs`
- Test: inline tests in `store.rs`

**Interfaces:**
- Consumes: `assemble::{load_session, Assembled, MAX_TRANSCRIPT_BYTES}`.

- [ ] **Step 1: Write the failing test — a session with a subagent merges via the store**

In `crates/sumcp-mcp/src/store.rs` test module, add:

```rust
    #[test]
    fn store_merges_subagents_and_reparses_on_sub_change() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = dir.path().join(format!("{uuid}.jsonl"));
        // main spawns one Agent with a legacy sibling.
        std::fs::write(
            &main,
            format!(
                "{}\n{}",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"x"}}]}}"#,
                r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"a1","is_error":false}]},"toolUseResult":{"agentId":"present"}}"#,
            ),
        )
        .unwrap();
        let sib = dir.path().join("agent-present.jsonl");
        std::fs::write(
            &sib,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/s.rs","new_string":"x"}}]}}"#,
        )
        .unwrap();

        let store = SessionStore::new();
        let a = store.load(&main).unwrap();
        let sub_actions =
            a.actions.iter().filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_))).count();
        assert_eq!(sub_actions, 1, "subagent edit merged via the store");

        // Grow the subagent file; the merged session must re-parse.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(&sib).unwrap();
        writeln!(f).unwrap();
        f.write_all(
            br#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/s2.rs","new_string":"y"}}]}}"#,
        )
        .unwrap();
        drop(f);

        let b = store.load(&main).unwrap();
        let sub_actions_b =
            b.actions.iter().filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_))).count();
        assert_eq!(sub_actions_b, 2, "appended subagent action picked up (freshness over sub files)");
    }
```

- [ ] **Step 2: Run — expect failure**

Run: `cargo test -p sumcp-mcp store_merges_subagents --no-run` then `cargo test -p sumcp-mcp store_merges_subagents`
Expected: FAIL — store currently ignores subagent files (`sub_actions == 0`), and the append is undetected.

- [ ] **Step 3: Extend the cache entry + `load_bounded`**

In `crates/sumcp-mcp/src/store.rs`:

(a) Add subagent-file fingerprints to `CacheEntry`:

```rust
struct CacheEntry {
    mtime: SystemTime,
    size: u64,
    /// (path, mtime, size) for every subagent file merged into `session`.
    /// Freshness requires ALL of these to be unchanged too, so an appended or
    /// added subagent transcript re-parses.
    subs: Vec<(PathBuf, SystemTime, u64)>,
    session: Arc<Session>,
    last_used: u64,
}
```

(b) Replace the imports line `use sumcp_core::ingest::ingest_str;` and `use sumcp_core::model::{Lane, Session};` with:

```rust
use sumcp_core::assemble::{load_session, MAX_TRANSCRIPT_BYTES as CORE_MAX_BYTES};
use sumcp_core::model::Session;
```

(Delete the local `MAX_TRANSCRIPT_BYTES` const if it now only duplicates core's; keep using `CORE_MAX_BYTES`. If other code in the file references the local const, alias instead: `use sumcp_core::assemble::MAX_TRANSCRIPT_BYTES;` and remove the local `const`.)

(c) Add a helper to stat the subagent files and compare:

```rust
/// Stat a list of subagent paths into (path, mtime, size) fingerprints,
/// skipping any that vanished (a vanished sub file forces a reload).
fn fingerprint_subs(paths: &[PathBuf]) -> Vec<(PathBuf, SystemTime, u64)> {
    paths
        .iter()
        .filter_map(|p| {
            let m = std::fs::metadata(p).ok()?;
            Some((p.clone(), m.modified().ok()?, m.len()))
        })
        .collect()
}
```

(d) In `load_bounded`, extend the freshness check and the parse. Replace the freshness `if let Some(entry) = ...` block's body condition to also require the subagent fingerprints match. Re-stat the *currently discovered* subagent files each call is unnecessary — instead compare the cached `entry.subs` fingerprints against a fresh stat of those same paths:

```rust
        if let Some(entry) = cache.map.get_mut(path)
            && entry.mtime == mtime
            && entry.size == size
            && entry.subs == fingerprint_subs(&entry.subs.iter().map(|(p, _, _)| p.clone()).collect::<Vec<_>>())
        {
            entry.last_used = now;
            return Ok(Arc::clone(&entry.session));
        }
```

(Rationale: a *new* subagent implies the main transcript grew — its `tool_result` line is appended — so `mtime`/`size` on main already catches new spawns; the `subs` comparison catches an existing sub file *growing*.)

(e) Replace the parse+insert region (the `ingest_str` call and `CacheEntry { .. }` literal):

```rust
        let assembled = load_session(path, max_bytes)
            .map_err(|e| std::io::Error::other(format!("assemble failed: {e}")))?;
        let subs = fingerprint_subs(&assembled.subagent_paths);
        let session = Arc::new(assembled.session);

        cache.map.insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                subs,
                session: Arc::clone(&session),
                last_used: now,
            },
        );
```

(f) The `load` wrapper passes `max_bytes` = `CORE_MAX_BYTES`. Update `pub fn load` to call `self.load_bounded(path, CORE_MAX_BYTES)`. `load_bounded` still takes `max_bytes` and passes it to `load_session`.

Note: `load_bounded` currently reads the main file itself for the size ceiling. `load_session` re-reads it. That double-read of the main file is acceptable (the stat is the freshness probe; the read is cheap relative to parse) and keeps the size-ceiling error semantics. Leave the existing `metadata`/`is_file`/ceiling checks in `load_bounded` as the pre-flight; `load_session` repeats the bounded read internally.

- [ ] **Step 4: Run — expect green**

Run: `cargo test -p sumcp-mcp && cargo clippy -p sumcp-mcp -- -D warnings`
Expected: PASS, including the existing cache tests (`second_load_of_unchanged_file_is_cached`, `grown_file_is_reparsed_and_fresh`) and the new subagent test.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-mcp/src/store.rs
git commit -m "T5.0f: MCP store assembles subagents + freshness over sub files"
```

---

### Task 7: Wire assembly into the CLI

Replace the CLI's single-file `ingest_str` with `assemble::load_session` so `sumcp --file` merges subagents when they exist next to the given file.

**Files:**
- Modify: `crates/sumcp-cli/src/main.rs`

**Interfaces:**
- Consumes: `assemble::{load_session, MAX_TRANSCRIPT_BYTES}`.

- [ ] **Step 1: Replace the read+ingest with assembly**

In `crates/sumcp-cli/src/main.rs`, replace the block that reads the file and calls `ingest_str` (lines ~34–42):

```rust
    let assembled = match sumcp_core::assemble::load_session(
        &path,
        sumcp_core::assemble::MAX_TRANSCRIPT_BYTES,
    ) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("could not load {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let session = assembled.session;
```

Remove the now-unused `use sumcp_core::ingest::ingest_str;` and `use ... Lane;` imports if the compiler flags them (leave `Lane` if still referenced elsewhere in the file).

- [ ] **Step 2: Build + run on the real donor fixture**

Run:
```bash
cargo run -q -p sumcp-cli -- --file fixtures/session-2_1_210-subagents.jsonl --json | python3 -c "import sys,json; d=json.load(sys.stdin); print('files_missing:', d['flags']['subagent_files_missing'])"
```
Expected: `files_missing: 12` — the donor spawns 12 subagents whose child files are absent (T1.2), so all 12 are honestly reported missing. This is the real-data check that the counter fires.

- [ ] **Step 3: Full workspace test + clippy**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/sumcp-cli/src/main.rs
git commit -m "T5.0g: CLI loads via assemble (merges subagents when present)"
```

---

### Task 8: Integration fixtures, stdio end-to-end, and doc closeout

Prove the merge end-to-end through the real MCP binary on a synthetic 2.1.x session (evidence lands in a sub lane), add a fixture-level assertion on the real donor's missing count, and update SPEC / task tracker.

**Files:**
- Create: `fixtures/subagent-merge/<uuid>.jsonl` + `fixtures/subagent-merge/<uuid>/subagents/agent-*.jsonl` (synthetic 2.1.x tree)
- Modify: `crates/sumcp-core/tests/fixture.rs` (donor missing-count assertion)
- Modify: `crates/sumcp-mcp/tests/stdio.rs` (sub-lane evidence end-to-end)
- Modify: `SPEC.md` (decision #2 → implemented), `tasks/todo.md`, `tasks/plan.md`

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: Build the synthetic 2.1.x fixture tree**

Create `fixtures/subagent-merge/5717aaaa-1111-2222-3333-444455556666.jsonl` (main), containing one `Agent` spawn (id `a1`, result agentId `helper`) and one main-lane `Edit`. Create `fixtures/subagent-merge/5717aaaa-1111-2222-3333-444455556666/subagents/agent-helper.jsonl` containing two `Edit` lines each carrying `"sessionId":"5717aaaa-1111-2222-3333-444455556666"`. Use timestamps that interleave (sub edits between the main spawn and a later main action) so the merged order mixes lanes. Keep every line sanitized/synthetic (no real paths — follow the `scripts/sanitize.py` conventions; use `/work/…` style paths like the donor).

Write these with the `Write` tool (three files). Verify the tree:

Run: `find fixtures/subagent-merge -type f`
Expected: the main file and one `agent-helper.jsonl` under `.../subagents/`.

- [ ] **Step 2: Add a fixture test for merge + donor missing-count (red-first)**

In `crates/sumcp-core/tests/fixture.rs`, add:

```rust
#[test]
fn donor_reports_all_subagents_missing() {
    // The 2.1.210 donor spawns subagents whose child files are unrecoverable
    // (T1.2), so an honest assembly reports every one as missing.
    let path: std::path::PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "fixtures", "session-2_1_210-subagents.jsonl"].iter().collect();
    let a = sumcp_core::assemble::load_session(&path, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES).unwrap();
    assert_eq!(a.session.subagent_files_missing, 12, "12 spawns, no child files on disk");
}

#[test]
fn synthetic_2_1_x_merges_subagent_actions() {
    let path: std::path::PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "fixtures", "subagent-merge", "5717aaaa-1111-2222-3333-444455556666.jsonl"].iter().collect();
    let a = sumcp_core::assemble::load_session(&path, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES).unwrap();
    let sub = a.session.actions.iter().filter(|x| matches!(x.lane, Lane::Sub(_))).count();
    assert_eq!(sub, 2, "both subagent edits merged");
    assert_eq!(a.session.subagent_files_missing, 0, "the one spawn resolved");
}
```

Run: `cargo test -p sumcp-core --test fixture donor_reports_all_subagents_missing synthetic_2_1_x_merges`
Expected: PASS if Tasks 1–5 are correct. If `donor_reports_all_subagents_missing` shows a number other than 12, recount with:
`grep -c '"agentId"' fixtures/session-2_1_210-subagents.jsonl` (should be 12) and reconcile before proceeding.

- [ ] **Step 3: Add the stdio sub-lane evidence test**

Open `crates/sumcp-mcp/tests/stdio.rs` and read how the existing end-to-end tests spawn the binary and point it at a fixture (they use `fixtures/session-2_1_210-subagents.jsonl`). Add a test that points the server at the synthetic `fixtures/subagent-merge/…jsonl`, calls `session_overview` (asserts it returns and `flags.subagent_files_missing == 0`), then calls `evidence` with an index known to be a subagent action, asserting the dereferenced evidence carries a `Sub` lane marker or the sub file path `/`-style content. Mirror the existing test's request/response plumbing exactly; only the fixture path, the asserted `subagent_files_missing`, and the evidence index differ.

Run: `cargo test -p sumcp-mcp --test stdio`
Expected: PASS.

- [ ] **Step 4: Run the whole suite + contract checker**

Run: `cargo test --workspace && cargo clippy --workspace -- -D warnings && python3 scripts/check_payloads.py`
Expected: all PASS. Count the tests: should be ≥ 107 + the new ones (merge ×6, lane ×2, discovery ×3, assemble ×3, store ×1, fixture ×2, stdio ×1).

- [ ] **Step 5: Update SPEC.md decision #2**

In `SPEC.md`, append to decision #2's cell (or a note beneath the table): mark it **implemented 2026-07-21**; state that `subagent_files_missing` replaces the pre-merge `subagents_excluded` counter; record the constraints — one level deep (no recursion), `MAX_SUBAGENT_FILES = 64`, 2.1.x ownership validated by namespaced dir + lenient `sessionId` check, legacy linked by `toolUseResult.agentId`; note the open item that exact 2.1.x on-disk path derivation is unverified against a real session (donor child files unrecoverable).

- [ ] **Step 6: Update the task tracker**

In `tasks/todo.md`, under Phase 5, add a checked line for this task (call it **T5.0** since it precedes T5.1) summarizing: subagent flat-merge landed — spawn records, `merge_sessions`, lane-scoped findings, discovery (both layouts), assembly, store+CLI wiring, `subagent_files_missing` (fires 12 on the donor); note it unblocks T5.3. Mirror a one-line entry in `tasks/plan.md` if that file enumerates tasks.

- [ ] **Step 7: Commit**

```bash
git add fixtures/subagent-merge crates/sumcp-core/tests/fixture.rs crates/sumcp-mcp/tests/stdio.rs SPEC.md tasks/todo.md tasks/plan.md
git commit -m "T5.0h: subagent-merge fixtures, stdio sub-lane evidence, SPEC/tracker closeout"
```

---

## Self-review notes (for the executor)

- **Spec coverage:** success criteria 1–7 map to Tasks 2 (merge/order/Idx), 8 (evidence sub-lane), 1 (counter rename), 4+5 (no false-merge / validation), 5a→3 (lane-scoping), 5+7 (graceful degradation), 1+8 (regression + lockstep docs).
- **Open item #1 (2.1.x on-disk path):** encoded as `subagents_dir = <dir>/<stem>/subagents`. If a real 2.1.x session later shows a different relation, only `locate::subagents_dir` and the synthetic fixture's directory shape change — no other task is affected. Flagged in SPEC (Task 8 Step 5).
- **`UserText` shape:** Task 2's tests assume `UserText { line_no, text, effective_ts }`. Verify against `model.rs` before running (noted in Task 2 Step 1).
- **Double-read of main in the store:** accepted (pre-flight stat + `load_session`'s bounded read); noted in Task 6 Step 3.
