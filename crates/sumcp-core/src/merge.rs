//! Flat-merge of a main session with its subagent sessions (SPEC decision 2).
//!
//! Pure: takes already-parsed `Session`s and produces one merged `Session`.
//! All filesystem work (finding and reading the child transcripts) lives in
//! `assemble.rs`; this module only combines what it is handed.

// `Lane` is used only by the tests (the merge itself sorts on `a.lane`, never
// naming the type), so it lives in the test module's imports to keep the
// non-test build free of unused-import warnings under `clippy -D warnings`.
use crate::model::{Action, Idx, Session};

/// Merge a main session with its subagent sessions into one totally-ordered
/// `Session`. `files_missing` is computed by the caller (assembly) and stored
/// verbatim; this function does no filesystem work.
pub fn merge_sessions(main: Session, subs: Vec<Session>, files_missing: u64) -> Session {
    // Start from main's counters and user_texts; main is privileged.
    let mut actions: Vec<Action> = main.actions;
    let user_texts = main.user_texts; // subagent user turns are dropped
    let cwd = main.cwd.clone();
    let mut tokens = main.tokens;
    let mut type_counts = main.type_counts;
    let mut parse_errors = main.parse_errors;
    let mut untimestamped_lines = main.untimestamped_lines;
    let mut interrupts = main.interrupts;
    let auto_accept = main.auto_accept; // NOT OR'd — see spec §5
    let spawns = main.spawns;
    // Main only: a subagent has no channel to ask the human anything, so a
    // decision can only ever appear in the main transcript. Same reasoning
    // that drops sub.user_texts.
    let decisions = main.decisions;
    // Extended, NOT main-only (unlike decisions and user_texts): a subagent
    // can create tasks, and a task a subagent left unfinished is exactly as
    // unfinished as one the main lane abandoned.
    let mut task_events = main.task_events;
    // Main only: subagent prose is internal reasoning the human never saw,
    // and folding it in would multiply the payload's largest block for no
    // reviewer benefit.
    let agent_texts = main.agent_texts;
    // Unlike agent_texts itself, the excluded COUNT is additive: every
    // subagent's tally of prose it couldn't keep is real, disclosable scope,
    // the same way subagent_files_missing sums across subagents below.
    let mut agent_texts_excluded = main.agent_texts_excluded;

    // Fold every subagent's actions and additive counters in.
    for sub in subs {
        actions.extend(sub.actions);
        task_events.extend(sub.task_events);
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
        agent_texts_excluded += sub.agent_texts_excluded;
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
        cwd,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines,
        interrupts,
        auto_accept,
        spawns,
        decisions,
        task_events,
        agent_texts,
        agent_texts_excluded,
        // This merge combines a main transcript with its own subagents into a
        // single work unit's worth of data, so it does not own the transcript id
        // table. The assembly step fills in session_ids when it merges multiple
        // transcripts into a work unit.
        session_ids: vec![],
        subagent_files_missing: files_missing,
    }
}

/// Merge the per-transcript sessions of one work unit into a single Session.
///
/// Each input is an already-assembled transcript (its own main lane plus any
/// subagent lanes, already through `merge_sessions`), paired with its
/// transcript id, oldest first.
///
/// HOW THIS DIFFERS FROM `merge_sessions`. That one merges a privileged main
/// with its subordinate subagents, so it keeps only main's user turns and
/// refuses to let a subagent's auto-accept mode suppress the main lane's
/// latency signals. Here every part is a real human-facing session, so user
/// turns all carry and `auto_accept` is OR'd: if any transcript in the stretch
/// ran under auto-accept, the latency heuristics are meaningless for the unit
/// and must be suppressed.
///
/// IMPORTANT: this stamps `session_ix` onto every action AND every user text
/// with the part's slot in `session_ids`. Stamping only actions would leave
/// every user text at ingest's default of `0`, and `pushback_between`
/// (signals/dynamics.rs) only matches user messages whose `session_ix`
/// equals the edit's `session_ix`, so transcripts after the first would
/// silently lose all pushback/Flip detection, with no error anywhere.
pub fn merge_work_unit(parts: Vec<(String, Session)>) -> Session {
    let mut actions: Vec<Action> = Vec::new();
    let mut user_texts = Vec::new();
    let mut session_ids: Vec<String> = Vec::new();
    let mut cwd = None;
    let mut tokens = crate::model::Tokens::default();
    let mut type_counts: std::collections::BTreeMap<String, u64> = Default::default();
    let mut parse_errors = 0u64;
    let mut untimestamped_lines = 0u64;
    let mut interrupts = 0u64;
    let mut auto_accept = false;
    let mut spawns = Vec::new();
    let mut decisions = Vec::new();
    let mut task_events = Vec::new();
    let mut agent_texts = Vec::new();
    // Summed across parts the same way every other additive counter in this
    // loop is: each part's own count of subagent prose it could not keep is
    // real, disclosable scope regardless of which transcript in the unit it
    // came from.
    let mut agent_texts_excluded = 0u64;
    // Strictly the sum of the parts' own subagent-missing counts. A work-unit
    // MEMBER that failed to load is a different kind of gap (it is not a
    // subagent spawn) and is disclosed as `work_unit.members_unreadable`
    // instead of being folded in here, where its meaning would be misstated.
    let mut subagent_files_missing = 0u64;

    for (ix, (id, part)) in parts.into_iter().enumerate() {
        // `ix` is this transcript's slot in `session_ids` (parts arrive
        // oldest first, and we push into session_ids in that same order
        // below, so `session_ix` is simply "the position in that list").
        // `as u16` never truncates in practice: the caller caps a work unit
        // at 16 transcripts, far under u16::MAX.
        let ix = ix as u16;
        session_ids.push(id);
        // Stamp every action AND every user text from this part with its
        // transcript's slot, before folding them into the shared vectors.
        // Doing both in the same loop, right next to each other, is exactly
        // the point: it is easy to remember to restamp actions and forget
        // user texts, since only actions carry session_ix in most other
        // code paths. See the doc comment above for what silently breaks if
        // the user_texts stamp is skipped.
        for mut a in part.actions {
            a.session_ix = ix;
            actions.push(a);
        }
        for mut u in part.user_texts {
            u.session_ix = ix;
            user_texts.push(u);
        }
        // Stamped like user_texts: a decision must be attributable to the
        // transcript it came from, or the payload cannot cite it correctly
        // in a multi-transcript work unit.
        for mut d in part.decisions {
            d.session_ix = ix;
            decisions.push(d);
        }
        // Stamped like decisions: a task event must be attributable to the
        // transcript it came from, or the payload cannot cite it correctly
        // in a multi-transcript work unit.
        for mut t in part.task_events {
            t.session_ix = ix;
            task_events.push(t);
        }
        // Stamped like decisions and task_events: an agent-text block must be
        // attributable to the transcript it came from, or the payload cannot
        // cite it correctly in a multi-transcript work unit.
        for mut at in part.agent_texts {
            at.session_ix = ix;
            agent_texts.push(at);
        }
        // First non-None cwd wins; every transcript in a unit is in the same
        // project, so they agree, but a synthetic session can have None.
        if cwd.is_none() {
            cwd = part.cwd;
        }
        tokens.input += part.tokens.input;
        tokens.output += part.tokens.output;
        tokens.cache_read += part.tokens.cache_read;
        tokens.cache_creation += part.tokens.cache_creation;
        for (t, n) in part.type_counts {
            *type_counts.entry(t).or_insert(0) += n;
        }
        parse_errors += part.parse_errors;
        untimestamped_lines += part.untimestamped_lines;
        interrupts += part.interrupts;
        auto_accept |= part.auto_accept;
        spawns.extend(part.spawns);
        subagent_files_missing += part.subagent_files_missing;
        agent_texts_excluded += part.agent_texts_excluded;
    }

    // One sort of the whole concatenation: O(n log n) total, done once. Never
    // a pairwise merge loop that folds transcripts in two at a time: at 16
    // transcripts that would mean 16 passes over the growing action stream
    // instead of one pass over all of it.
    //
    // The sort key is (timestamp, transcript slot, lane, source line number).
    // Each piece breaks a tie left by the one before it:
    //   - effective_ts: the real ordering signal, wall-clock time.
    //   - session_ix: two transcripts can share a timestamp (second
    //     resolution, or a synthetic session's borrowed clock); this decides
    //     between them the same way every time, regardless of which order
    //     the caller listed the parts in.
    //   - lane: within one transcript, Main sorts before its subagent lanes
    //     (mirrors merge_sessions's own tie-break).
    //   - line_no: only meaningful within one (transcript, lane) pair, which
    //     the two keys before it already pin down.
    // Because this key is total (no two actions can tie on all four parts
    // and still be genuinely ambiguous), the result is deterministic no
    // matter what order `parts` arrived in or whether the sort is stable.
    actions.sort_by(|a, b| {
        (&a.effective_ts, a.session_ix, &a.lane, a.line_no).cmp(&(
            &b.effective_ts,
            b.session_ix,
            &b.lane,
            b.line_no,
        ))
    });
    // User turns are read in order by the flip detector, so they need the
    // same total-ordering treatment as actions (timestamp, then line number
    // within a transcript to break ties).
    user_texts.sort_by(|a, b| (&a.effective_ts, a.line_no).cmp(&(&b.effective_ts, b.line_no)));

    // Re-number Idx across the merged whole. Before this loop, every action's
    // `idx` is only valid within its own transcript (e.g. two different
    // parts can both have an action with idx 0); after sorting them together
    // those old values are meaningless. This loop walks the now-sorted
    // vector and overwrites idx with its actual position, so
    // `actions[i].idx == Idx(i)` holds for the merged whole. Every payload
    // and the `evidence()` tool rely on that equality to look an action up
    // by index.
    for (i, a) in actions.iter_mut().enumerate() {
        a.idx = Idx(i as u32);
    }

    Session {
        actions,
        user_texts,
        cwd,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines,
        interrupts,
        auto_accept,
        spawns,
        decisions,
        task_events,
        agent_texts,
        agent_texts_excluded,
        subagent_files_missing,
        session_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ActionKind, Lane, Spawn};

    /// Build a minimal one-action Session in a given lane at a given timestamp.
    fn one(lane: Lane, ts: &str, line_no: usize, file: &str) -> Session {
        Session {
            actions: vec![Action {
                idx: Idx(0),
                effective_ts: ts.to_string(),
                ts_inherited: false,
                lane,
                session_ix: 0,
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
                auto_accept_here: false,
            }],
            user_texts: vec![],
            cwd: None,
            tokens: Default::default(),
            type_counts: Default::default(),
            parse_errors: 0,
            untimestamped_lines: 0,
            interrupts: 0,
            auto_accept: false,
            spawns: vec![],
            decisions: vec![],
            task_events: vec![],
            agent_texts: vec![],
            agent_texts_excluded: 0,
            session_ids: vec![],
            subagent_files_missing: 0,
        }
    }

    #[test]
    fn merged_idx_is_contiguous_and_ordered() {
        // main action at 00:02, sub action at 00:01 → sub sorts first.
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.spawns = vec![Spawn {
            agent_id: Some("x".into()),
        }];
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
        use crate::model::{TurnOrigin, UserText};
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.user_texts = vec![UserText {
            line_no: 1,
            text: "human says".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
            session_ix: 0,
            origin: TurnOrigin::Human,
        }];
        main.auto_accept = false;
        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.user_texts = vec![UserText {
            line_no: 1,
            text: "orchestrator prompt".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
            session_ix: 0,
            origin: TurnOrigin::Human,
        }];
        sub.auto_accept = true; // a sub in auto-accept must NOT flip the merged flag

        let merged = merge_sessions(main, vec![sub], 0);
        assert_eq!(merged.user_texts.len(), 1);
        assert_eq!(merged.user_texts[0].text, "human says");
        assert!(
            !merged.auto_accept,
            "sub auto-accept must not suppress main latency"
        );
    }

    #[test]
    fn counters_sum_and_files_missing_passthrough() {
        use crate::model::Tokens;
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.parse_errors = 1;
        main.untimestamped_lines = 2;
        main.interrupts = 1;
        // WHY these two extra counters: every other sub in the suite uses zero
        // tokens and empty type_counts, so a dropped `+=` (or an overwrite) on
        // either would still pass every test. Give BOTH main and sub nonzero
        // values on overlapping and non-overlapping keys so the assertions
        // below can only pass if the merge is genuinely additive.
        main.tokens = Tokens {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 40,
        };
        main.type_counts.insert("assistant".into(), 6); // overlaps with sub
        main.type_counts.insert("user".into(), 2); // main-only key

        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.parse_errors = 3;
        sub.untimestamped_lines = 4;
        sub.tokens = Tokens {
            input: 5,
            output: 7,
            cache_read: 3,
            cache_creation: 2,
        };
        sub.type_counts.insert("assistant".into(), 4); // adds to main's 6
        sub.type_counts.insert("tool_result".into(), 9); // sub-only key

        let merged = merge_sessions(main, vec![sub], 7);
        assert_eq!(merged.parse_errors, 4);
        assert_eq!(merged.untimestamped_lines, 6);
        assert_eq!(merged.interrupts, 1);
        assert_eq!(merged.subagent_files_missing, 7);
        // Token fields must be element-wise SUMS of main + sub.
        assert_eq!(merged.tokens.input, 15);
        assert_eq!(merged.tokens.output, 27);
        assert_eq!(merged.tokens.cache_read, 33);
        assert_eq!(merged.tokens.cache_creation, 42);
        // type_counts must merge additively: shared key sums, distinct keys kept.
        assert_eq!(merged.type_counts["assistant"], 10); // 6 + 4
        assert_eq!(merged.type_counts["user"], 2); // main-only, untouched
        assert_eq!(merged.type_counts["tool_result"], 9); // sub-only, carried in
    }

    #[test]
    fn merge_sessions_sums_a_subagents_excluded_count_into_main() {
        // agent_texts_excluded is additive, unlike agent_texts itself (which
        // stays main-only). A subagent's tally of prose it couldn't keep is
        // real, disclosable scope and must survive the merge even though the
        // strings behind it never do.
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.agent_texts_excluded = 2;
        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.agent_texts_excluded = 5;

        let merged = merge_sessions(main, vec![sub], 0);
        assert_eq!(
            merged.agent_texts_excluded, 7,
            "main's own count plus the subagent's, summed"
        );
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
            actions: vec![],
            user_texts: vec![],
            cwd: None,
            tokens: Default::default(),
            type_counts: Default::default(),
            parse_errors: 0,
            untimestamped_lines: 0,
            interrupts: 0,
            auto_accept: false,
            spawns: vec![],
            decisions: vec![],
            task_events: vec![],
            agent_texts: vec![],
            agent_texts_excluded: 0,
            session_ids: vec![],
            subagent_files_missing: 0,
        };
        let merged = merge_sessions(main, vec![empty], 1);
        assert_eq!(merged.actions.len(), 1);
        assert_eq!(merged.subagent_files_missing, 1);
    }

    #[test]
    fn work_unit_merge_stamps_session_ix_and_renumbers_idx() {
        // Two transcripts, the second starting earlier in wall-clock time than
        // the first ends, so the total order interleaves them.
        let a = one(Lane::Main, "2026-01-01T00:00:03Z", 1, "/a");
        let b = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/b");

        let merged = merge_work_unit(vec![("sess-a".to_string(), a), ("sess-b".to_string(), b)]);

        // The table records both transcripts, in the order given.
        assert_eq!(merged.session_ids, vec!["sess-a", "sess-b"]);
        // Two actions, interleaved by timestamp: b's is earlier.
        assert_eq!(merged.actions.len(), 2);
        assert_eq!(merged.actions[0].file_path.as_deref(), Some("/b"));
        assert_eq!(merged.actions[0].session_ix, 1, "b is index 1");
        assert_eq!(merged.actions[1].session_ix, 0, "a is index 0");
        // Idx renumbered across the merged whole, the invariant evidence() needs.
        for (i, act) in merged.actions.iter().enumerate() {
            assert_eq!(act.idx, Idx(i as u32));
        }
    }

    #[test]
    fn work_unit_merge_sums_counters_across_transcripts() {
        use crate::model::Tokens;
        let mut a = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/a");
        a.tokens = Tokens {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_creation: 40,
        };
        a.parse_errors = 1;
        a.interrupts = 2;
        a.subagent_files_missing = 1;

        let mut b = one(Lane::Main, "2026-01-01T00:00:02Z", 1, "/b");
        b.tokens = Tokens {
            input: 5,
            output: 7,
            cache_read: 3,
            cache_creation: 2,
        };
        b.parse_errors = 3;
        b.interrupts = 4;
        b.subagent_files_missing = 2;

        let merged = merge_work_unit(vec![("a".to_string(), a), ("b".to_string(), b)]);
        assert_eq!(merged.tokens.input, 15);
        assert_eq!(merged.tokens.output, 27);
        assert_eq!(merged.tokens.cache_read, 33);
        assert_eq!(merged.tokens.cache_creation, 42);
        assert_eq!(merged.parse_errors, 4);
        assert_eq!(merged.interrupts, 6);
        assert_eq!(
            merged.subagent_files_missing, 3,
            "per-transcript subagent misses sum across the unit"
        );
    }

    #[test]
    fn work_unit_merge_ors_auto_accept_and_keeps_every_user_text() {
        use crate::model::{TurnOrigin, UserText};
        // Unlike the subagent merge, which deliberately ignores a subagent's
        // user turns and auto-accept, every transcript in a work unit is a
        // real human-facing session, so both must carry.
        let mut a = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/a");
        a.user_texts = vec![UserText {
            line_no: 1,
            text: "first".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
            session_ix: 0,
            origin: TurnOrigin::Human,
        }];
        a.auto_accept = false;
        let mut b = one(Lane::Main, "2026-01-01T00:00:02Z", 1, "/b");
        b.user_texts = vec![UserText {
            line_no: 1,
            text: "second".into(),
            effective_ts: "2026-01-01T00:00:02Z".into(),
            session_ix: 0,
            origin: TurnOrigin::Human,
        }];
        b.auto_accept = true;

        let merged = merge_work_unit(vec![("a".into(), a), ("b".into(), b)]);
        assert_eq!(merged.user_texts.len(), 2);
        assert!(
            merged.auto_accept,
            "a transcript that ran under auto-accept must suppress latency signals for the unit"
        );
    }

    #[test]
    fn work_unit_merge_stamps_session_ix_on_user_texts_too() {
        // CRITICAL: pushback_between (signals/dynamics.rs) only matches user
        // messages whose session_ix equals the edit's session_ix. If the
        // merge stamped actions but left every user_text at its ingest-time
        // default of 0, then for the second transcript and beyond,
        // pushback_between would silently find nothing and Flip detection
        // would die for those transcripts with no error and no failing test
        // elsewhere: this test is the only thing pinning that behavior down.
        use crate::model::{TurnOrigin, UserText};
        let a = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/a");
        let mut b = one(Lane::Main, "2026-01-01T00:00:02Z", 1, "/b");
        b.user_texts = vec![UserText {
            line_no: 1,
            text: "second transcript's user turn".into(),
            effective_ts: "2026-01-01T00:00:02Z".into(),
            session_ix: 0, // as ingest always leaves it; the merge must restamp
            origin: TurnOrigin::Human,
        }];

        let merged = merge_work_unit(vec![("a".into(), a), ("b".into(), b)]);
        assert_eq!(merged.user_texts.len(), 1);
        assert_eq!(
            merged.user_texts[0].session_ix, 1,
            "b's user text must carry b's slot (1), not the ingest-time default 0"
        );
    }

    #[test]
    fn main_and_subagent_lanes_can_each_hold_task_id_1_without_colliding() {
        // Each transcript's harness numbers task ids from 1 independently, so
        // a subagent's task "1" and the main lane's task "1" are unrelated
        // tasks that merely share a string id. `merge_sessions` concatenates
        // the subagent's task_events into the main lane's without
        // renumbering anything, so if identity were `id` alone, a later
        // final-state replay keyed on it would silently merge these two
        // distinct tasks into one. `lane` must keep them apart.
        use crate::ingest::ingest_str;

        let main_create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"m1","name":"TaskCreate","input":{"subject":"Main lane task"}}]}}"#;
        let main_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"m1"}]},"toolUseResult":{"task":{"id":"1","subject":"Main lane task"}}}"#;
        let mut main = ingest_str(&format!("{main_create}\n{main_result}"), Lane::Main);
        main.spawns = vec![Spawn {
            agent_id: Some("x".into()),
        }];

        let sub_create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"s1","name":"TaskCreate","input":{"subject":"Subagent lane task"}}]}}"#;
        let sub_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"s1"}]},"toolUseResult":{"task":{"id":"1","subject":"Subagent lane task"}}}"#;
        let sub = ingest_str(
            &format!("{sub_create}\n{sub_result}"),
            Lane::Sub("x".into()),
        );

        let merged = merge_sessions(main, vec![sub], 0);

        assert_eq!(merged.task_events.len(), 2, "both tasks kept, not merged");
        assert!(merged.task_events.iter().any(|t| t.id == "1"
            && t.lane == Lane::Main
            && t.subject.as_deref() == Some("Main lane task")));
        assert!(merged.task_events.iter().any(|t| t.id == "1"
            && t.lane == Lane::Sub("x".into())
            && t.subject.as_deref() == Some("Subagent lane task")));
    }
}
