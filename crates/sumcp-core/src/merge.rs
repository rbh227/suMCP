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
        use crate::model::UserText;
        let mut main = one(Lane::Main, "2026-01-01T00:00:02Z", 5, "/a");
        main.user_texts = vec![UserText {
            line_no: 1,
            text: "human says".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
        }];
        main.auto_accept = false;
        let mut sub = one(Lane::Sub("x".into()), "2026-01-01T00:00:01Z", 3, "/b");
        sub.user_texts = vec![UserText {
            line_no: 1,
            text: "orchestrator prompt".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
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
            tokens: Default::default(),
            type_counts: Default::default(),
            parse_errors: 0,
            untimestamped_lines: 0,
            interrupts: 0,
            auto_accept: false,
            spawns: vec![],
            subagent_files_missing: 0,
        };
        let merged = merge_sessions(main, vec![empty], 1);
        assert_eq!(merged.actions.len(), 1);
        assert_eq!(merged.subagent_files_missing, 1);
    }
}
