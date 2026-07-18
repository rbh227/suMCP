//! Dynamics signals: per-segment opening moves, true reverts, capitulation
//! flips, user corrections, and the advisory action-loop detector. The
//! reverts/flips fire rarely (SPEC §1 amendment 3) — high-signal when they
//! do — so they exist but never anchor the ranking.
//!
//! Still deferred: standalone pushback-rate stats. Interruptions are already
//! counted in `Session::interrupts`.

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};

/// Only the first N actions of a segment define its "opening move".
const OPENING_WINDOW: usize = 10;
/// Segments smaller than this are not classified: a two-action segment after
/// "yes, do it" is delegation, not a behavioral pattern (metrics-spec #9).
const MIN_SEGMENT_ACTIONS: usize = 5;
/// Pushback markers that turn a plain revert into a capitulation flip.
const PUSHBACK: [&str; 8] = [
    "no ", "don't", "do not", "wrong", "revert", "undo", "instead", "not what",
];

/// Run the dynamics signals that produce findings.
pub fn dynamics(s: &Session) -> Vec<Finding> {
    let mut out = Vec::new();
    out.extend(opening_move(s));
    out.extend(reverts_and_flips(s));
    out.extend(user_corrections(s));
    out.extend(action_loops(s));
    out
}

/// Minimum run length for a stuck-in-loop flag (SEAlign / agentic-eval
/// definition: identical tool+args for ≥3 consecutive turns).
const LOOP_MIN_REPEATS: usize = 3;

/// Stuck-in-loop (metrics-spec #21): ≥3 consecutive byte-identical tool calls
/// (same name + same full input, via `input_hash`) within one lane.
///
/// Always **advisory**: SWE-agent's authors abandoned automated loop detectors
/// because false positives were too common. `Confidence::Low` is the advisory
/// mechanism — ranking multiplies these by `low_confidence_factor`.
fn action_loops(s: &Session) -> Vec<Finding> {
    let mut out = Vec::new();
    // Per lane: a run in the main lane must not be broken (or created) by an
    // interleaved subagent doing its own thing.
    let lanes: std::collections::BTreeSet<&crate::model::Lane> =
        s.actions.iter().map(|a| &a.lane).collect();
    for lane in lanes {
        let lane_actions: Vec<&Action> = s.actions.iter().filter(|a| &a.lane == lane).collect();
        let mut run: Vec<&Action> = Vec::new();
        // `chain(None)` appends one non-action so the loop flushes the final run.
        for a in lane_actions
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
        {
            let extends = match (&a, run.last()) {
                (Some(a), Some(last)) => a.input_hash.is_some() && a.input_hash == last.input_hash,
                _ => false,
            };
            if !extends {
                if run.len() >= LOOP_MIN_REPEATS {
                    let mut nums = std::collections::BTreeMap::new();
                    nums.insert("repeats".into(), run.len() as f64);
                    out.push(Finding {
                        kind: FindingKind::ActionLoop,
                        nums,
                        tier: Tier::T1,
                        exact: true,
                        confidence: Confidence::Low, // advisory by construction
                        note: Some(format!(
                            "{} byte-identical consecutive calls — possible stuck loop \
                             (advisory: loop detectors are false-positive-prone)",
                            run.len()
                        )),
                        idxs: run.iter().map(|a| a.idx).collect(),
                        file: run[0].file_path.clone(),
                    });
                }
                run.clear();
            }
            if let Some(a) = a {
                run.push(a);
            }
        }
    }
    out
}

/// One task segment: the run of main-lane actions between two consecutive
/// user messages. `user_line_no` is the transcript line of the message that
/// opened the segment (`None` for actions before any user text — rare, but a
/// resumed transcript can start mid-flight).
pub(crate) struct Segment<'a> {
    /// Line number of the user message that opened this segment.
    pub user_line_no: Option<usize>,
    /// The segment's main-lane actions, in total order.
    pub actions: Vec<&'a Action>,
}

/// Split the session into task segments at each substantive user message.
///
/// `user_texts` is already isMeta-filtered at ingest, so every entry is a real
/// human turn. Only main-lane actions participate: subagent lines have their
/// own file's line numbering, so comparing their `line_no` against main-
/// transcript user lines would be meaningless.
///
/// Lifetime note for the Rust-learning reader: `Segment<'a>` borrows actions
/// from the `Session` — the `'a` says "these references live only as long as
/// the session does", which the compiler enforces for us.
pub(crate) fn segments(s: &Session) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();
    // user_texts are in source order; walk them alongside the actions.
    let mut boundaries = s.user_texts.iter().peekable();
    let mut current = Segment {
        user_line_no: None,
        actions: Vec::new(),
    };
    for a in s
        .actions
        .iter()
        .filter(|a| a.lane == crate::model::Lane::Main)
    {
        // Every user message at or before this action's line opens a fresh
        // segment (the LAST such message wins when several are adjacent).
        let mut crossed = None;
        while let Some(u) = boundaries.peek() {
            if u.line_no < a.line_no {
                crossed = Some(u.line_no);
                boundaries.next();
            } else {
                break;
            }
        }
        if let Some(line) = crossed {
            // close the running segment (if it saw any actions) and start anew
            if !current.actions.is_empty() || current.user_line_no.is_some() {
                out.push(current);
            }
            current = Segment {
                user_line_no: Some(line),
                actions: Vec::new(),
            };
        }
        current.actions.push(a);
    }
    if !current.actions.is_empty() || current.user_line_no.is_some() {
        out.push(current);
    }
    out
}

/// Classify each task segment's opening move: did the agent gather context
/// (read) before its first edit, or dive straight into editing? Read-first
/// correlates with success (ρ ≈ +0.68); patch-first openings with failure
/// (ρ ≈ −0.78). Computed per segment, not whole-session, because in an
/// interactive session the human may legitimately direct an immediate edit
/// (metrics-spec #9's interactive caveat) — which is also why these findings
/// are heuristic (`exact: false`, Medium confidence) and cite the leading
/// user message so the narrating agent can overrule.
fn opening_move(s: &Session) -> Vec<Finding> {
    segments(s)
        .iter()
        .filter(|seg| seg.actions.len() >= MIN_SEGMENT_ACTIONS)
        .filter_map(|seg| {
            let window = &seg.actions[..seg.actions.len().min(OPENING_WINDOW)];
            let first_read = window
                .iter()
                .position(|a| matches!(a.kind, ActionKind::Read));
            let first_edit = window
                .iter()
                .position(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write))?;
            // no edit in the opening window ⇒ nothing to classify

            let read_first = matches!(first_read, Some(r) if r < first_edit);
            let edits_in_window = window
                .iter()
                .filter(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write))
                .count();
            let mut nums = std::collections::BTreeMap::new();
            // The paper's exact operationalizations, kept as numbers so the
            // binary label never hides the magnitude (metrics-spec #9).
            nums.insert(
                "edit_fraction_first10".into(),
                edits_in_window as f64 / window.len() as f64,
            );
            nums.insert("first_edit_index".into(), first_edit as f64);

            let opened_by = match seg.user_line_no {
                Some(l) => format!("segment opened by user message at line {l}"),
                None => "segment precedes any user message".to_string(),
            };
            let idxs: Vec<Idx> = window.iter().take(first_edit + 1).map(|a| a.idx).collect();
            Some(Finding {
                kind: FindingKind::OpeningMove,
                nums,
                tier: Tier::T1,
                exact: false, // the human may have directed the immediate edit
                confidence: Confidence::Medium,
                note: Some(if read_first {
                    format!("read-first: gathered context before the first edit ({opened_by})")
                } else {
                    format!(
                        "patch-first: edited before reading — the #1 empirical failure \
                         mode, unless the user directed it ({opened_by})"
                    )
                }),
                idxs,
                file: None,
            })
        })
        .collect()
}

/// Share of classified segments that opened patch-first. `None` when no
/// segment was large enough to classify. The session-level roll-up of #9.
pub fn patch_first_segment_share(s: &Session) -> Option<f64> {
    let classified: Vec<bool> = opening_move(s)
        .iter()
        .map(|f| {
            f.note
                .as_deref()
                .is_some_and(|n| n.starts_with("patch-first"))
        })
        .collect();
    if classified.is_empty() {
        return None;
    }
    Some(classified.iter().filter(|p| **p).count() as f64 / classified.len() as f64)
}

/// True reverts and flips: a later edit whose new content restores what an
/// earlier edit removed (`later.new == earlier.old`). It is a `Flip`
/// (capitulation) only when the user pushed back in between AND the agent
/// gathered **no new evidence** — no Read, no Bash — between the pushback and
/// the reverting edit (locked decision #3, FlipFlop caveat): reversing after
/// a failing test or a fresh read is healthy revision, not sycophancy.
fn reverts_and_flips(s: &Session) -> Vec<Finding> {
    let edits: Vec<&Action> = s
        .actions
        .iter()
        .filter(|a| {
            matches!(a.kind, ActionKind::Edit) && a.edit_old.is_some() && a.edit_new.is_some()
        })
        .collect();

    let mut out = Vec::new();
    for (i, later) in edits.iter().enumerate() {
        for earlier in &edits[..i] {
            // same file, and the later edit puts back what the earlier removed
            if earlier.file_path == later.file_path && later.edit_new == earlier.edit_old {
                // A flip needs BOTH: pushback in between, and no evidence
                // gathered between that pushback and the reverting edit.
                let is_flip = pushback_between(s, earlier.line_no, later.line_no)
                    .is_some_and(|push| !evidence_between(s, push, later));
                out.push(Finding {
                    kind: if is_flip {
                        FindingKind::Flip
                    } else {
                        FindingKind::TrueRevert
                    },
                    nums: Default::default(),
                    tier: Tier::T2,
                    exact: true,
                    confidence: Confidence::High,
                    note: Some(
                        if is_flip {
                            "reverted right after user pushback with no new evidence \
                             gathered between (capitulation flip)"
                        } else {
                            "later edit restored earlier-removed content"
                        }
                        .into(),
                    ),
                    idxs: vec![earlier.idx, later.idx],
                    file: later.file_path.clone(),
                });
                break; // one finding per reverting edit
            }
        }
    }
    out
}

/// The first user message with pushback wording between two main-transcript
/// line numbers, if any.
fn pushback_between(s: &Session, lo: usize, hi: usize) -> Option<&crate::model::UserText> {
    s.user_texts.iter().find(|u| {
        u.line_no > lo && u.line_no < hi && {
            let t = u.text.to_lowercase();
            PUSHBACK.iter().any(|p| t.contains(p))
        }
    })
}

/// Did the agent gather any evidence (a Read or a Bash run, any lane) between
/// the pushback message and the reverting edit? Evidence is evidence — a
/// failing test or a read of an *unrelated* file both count; we deliberately
/// do not restrict to the reverted file.
///
/// Main-lane actions compare by transcript line number (same file as the
/// pushback message). Subagent actions live in other files, so they compare
/// by timestamp — *strictly* between, so timestamp ties stay excluded per the
/// order-uncertain contract (SPEC decision 2).
fn evidence_between(s: &Session, push: &crate::model::UserText, later: &Action) -> bool {
    s.actions.iter().any(|a| {
        matches!(a.kind, ActionKind::Read | ActionKind::Bash)
            && match a.lane {
                crate::model::Lane::Main => a.line_no > push.line_no && a.line_no < later.line_no,
                crate::model::Lane::Sub(_) => {
                    // ISO-8601 timestamps of equal format compare correctly
                    // as strings; ties are excluded by the strict `<`/`>`.
                    a.effective_ts > push.effective_ts && a.effective_ts < later.effective_ts
                }
            }
    })
}

/// User corrections: edits the user hand-modified (`userModified: true`).
fn user_corrections(s: &Session) -> Vec<Finding> {
    s.actions
        .iter()
        .filter(|a| a.user_modified)
        .map(|a| Finding {
            kind: FindingKind::UserCorrected,
            nums: Default::default(),
            tier: Tier::T2,
            exact: true,
            confidence: Confidence::High,
            note: Some("the user hand-edited this change".into()),
            idxs: vec![a.idx],
            file: a.file_path.clone(),
        })
        .collect()
}

/// Share of edits preceded (anywhere earlier) by a read of the same file.
/// A validated success predictor (read-before-edit ρ ≈ +0.68).
pub fn read_before_edit_share(s: &Session) -> f64 {
    let mut edits = 0;
    let mut read_first = 0;
    for (pos, a) in s.actions.iter().enumerate() {
        if !matches!(a.kind, ActionKind::Edit) {
            continue;
        }
        let Some(file) = a.file_path.as_deref() else {
            continue;
        };
        edits += 1;
        let saw_read = s.actions[..pos]
            .iter()
            .any(|p| matches!(p.kind, ActionKind::Read) && p.file_path.as_deref() == Some(file));
        if saw_read {
            read_first += 1;
        }
    }
    if edits == 0 {
        return 1.0; // vacuously fine
    }
    read_first as f64 / edits as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::{FindingKind, Lane};

    fn read(id: &str, ts: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
        )
    }
    fn edit(id: &str, ts: &str, file: &str, old: &str, new: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file}","old_string":"{old}","new_string":"{new}"}}}}]}}}}"#
        )
    }
    fn user(ts: &str, text: &str) -> String {
        format!(r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"{text}"}}}}"#)
    }

    /// A user prompt followed by `n` actions, `edit_at` of which (0-based)
    /// is the first Edit; the rest are Reads. Timestamps stay ordered.
    fn segment(prefix: &str, t0: usize, n: usize, edit_at: usize, prompt: &str) -> String {
        let mut lines = vec![user(&format!("2026-01-01T00:00:{:02}Z", t0), prompt)];
        for i in 0..n {
            let ts = format!("2026-01-01T00:00:{:02}Z", t0 + 1 + i);
            let id = format!("{prefix}{i}");
            lines.push(if i == edit_at {
                edit(&id, &ts, "/a.ts", "x", "y")
            } else {
                read(&id, &ts, "/b.ts")
            });
        }
        lines.join("\n")
    }

    #[test]
    fn opening_move_detects_patch_first_per_segment() {
        // first (and only) segment: edit at index 0, then 4 reads
        let raw = segment("s", 1, 5, 0, "please fix the bug");
        let f = opening_move(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        let f = &f[0];
        assert!(f.note.as_ref().unwrap().contains("patch-first"));
        assert!(!f.exact, "interactive caveat: heuristic");
        assert_eq!(f.confidence, crate::model::Confidence::Medium);
        assert_eq!(f.nums["first_edit_index"], 0.0);
        assert_eq!(f.nums["edit_fraction_first10"], 1.0 / 5.0);
        assert!(
            f.note.as_ref().unwrap().contains("line 0"),
            "cites the leading user message so the agent can overrule"
        );
    }

    #[test]
    fn opening_move_detects_read_first_per_segment() {
        // reads at 0..3, edit at index 4
        let raw = segment("s", 1, 5, 4, "please fix the bug");
        let f = opening_move(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert!(f[0].note.as_ref().unwrap().contains("read-first"));
        assert_eq!(f[0].nums["first_edit_index"], 4.0);
    }

    #[test]
    fn tiny_segment_after_directive_is_not_classified() {
        // "yes go ahead" then 3 actions — under MIN_SEGMENT_ACTIONS, silence.
        let raw = segment("s", 1, 3, 0, "yes go ahead");
        assert!(opening_move(&ingest_str(&raw, Lane::Main)).is_empty());
    }

    #[test]
    fn patch_first_share_counts_classified_segments_only() {
        // segment 1: patch-first (5 actions); segment 2: read-first (5);
        // segment 3: 2 actions — not classified.
        let raw = format!(
            "{}\n{}\n{}",
            segment("a", 1, 5, 0, "task one"),
            segment("b", 10, 5, 4, "task two"),
            segment("c", 20, 2, 0, "do it"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert_eq!(opening_move(&s).len(), 2, "third segment too small");
        assert_eq!(patch_first_segment_share(&s), Some(0.5));
    }

    #[test]
    fn no_classifiable_segment_means_no_share() {
        let raw = segment("s", 1, 2, 0, "quick tweak");
        assert_eq!(
            patch_first_segment_share(&ingest_str(&raw, Lane::Main)),
            None
        );
    }

    #[test]
    fn true_revert_detected_without_pushback() {
        // edit A: foo->bar; edit B: bar->foo  (restores foo)
        let raw = format!(
            "{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts", "foo", "bar"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts", "bar", "foo"),
        );
        let f = reverts_and_flips(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::TrueRevert);
    }

    #[test]
    fn flip_detected_when_user_pushes_back_between() {
        let raw = format!(
            "{}\n{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts", "foo", "bar"),
            user("2026-01-01T00:00:015Z", "no revert that please"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts", "bar", "foo"),
        );
        let f = reverts_and_flips(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].kind,
            FindingKind::Flip,
            "pushback between makes it a flip"
        );
    }

    fn bash(id: &str, ts: &str, cmd: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"{cmd}"}}}}]}}}}"#
        )
    }

    #[test]
    fn revert_after_test_run_is_not_a_flip() {
        // pushback, then a test run (evidence — pass or fail), then the revert:
        // healthy revision, not capitulation (FlipFlop caveat).
        let raw = format!(
            "{}\n{}\n{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts", "foo", "bar"),
            user("2026-01-01T00:00:02Z", "no revert that please"),
            bash("b1", "2026-01-01T00:00:03Z", "cargo test"),
            edit("2", "2026-01-01T00:00:04Z", "/a.ts", "bar", "foo"),
        );
        let f = reverts_and_flips(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(
            f[0].kind,
            FindingKind::TrueRevert,
            "evidence between ⇒ not a flip"
        );
    }

    #[test]
    fn revert_after_unrelated_read_is_not_a_flip() {
        // Evidence is evidence — even a read of a DIFFERENT file.
        let raw = format!(
            "{}\n{}\n{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts", "foo", "bar"),
            user("2026-01-01T00:00:02Z", "that's wrong"),
            read("r1", "2026-01-01T00:00:03Z", "/other.ts"),
            edit("2", "2026-01-01T00:00:04Z", "/a.ts", "bar", "foo"),
        );
        let f = reverts_and_flips(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::TrueRevert);
    }

    #[test]
    fn calm_edits_are_not_reverts() {
        // two unrelated edits — no restoration
        let raw = format!(
            "{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts", "foo", "bar"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts", "baz", "qux"),
        );
        assert!(reverts_and_flips(&ingest_str(&raw, Lane::Main)).is_empty());
    }

    fn grep(id: &str, ts: &str, pattern: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Grep","input":{{"pattern":"{pattern}"}}}}]}}}}"#
        )
    }

    #[test]
    fn three_identical_calls_fire_one_advisory_loop() {
        let raw = (0..3)
            .map(|i| {
                grep(
                    &format!("g{i}"),
                    &format!("2026-01-01T00:00:0{i}Z"),
                    "needle",
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let f = action_loops(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::ActionLoop);
        assert_eq!(f[0].nums["repeats"], 3.0);
        assert_eq!(
            f[0].confidence,
            crate::model::Confidence::Low,
            "advisory-only"
        );
    }

    #[test]
    fn two_identical_calls_do_not_fire() {
        let raw = format!(
            "{}\n{}",
            grep("g0", "2026-01-01T00:00:00Z", "needle"),
            grep("g1", "2026-01-01T00:00:01Z", "needle"),
        );
        assert!(action_loops(&ingest_str(&raw, Lane::Main)).is_empty());
    }

    #[test]
    fn five_identical_calls_fire_once_with_repeats_five() {
        let raw = (0..5)
            .map(|i| {
                grep(
                    &format!("g{i}"),
                    &format!("2026-01-01T00:00:0{i}Z"),
                    "needle",
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let f = action_loops(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1, "one finding per run, not overlapping ones");
        assert_eq!(f[0].nums["repeats"], 5.0);
    }

    #[test]
    fn runs_are_per_lane_never_across() {
        // Two identical calls in Main + one identical in a sub lane, times
        // interleaved. No single lane has 3 consecutive ⇒ silence.
        let main = ingest_str(
            &format!(
                "{}\n{}",
                grep("g0", "2026-01-01T00:00:00Z", "needle"),
                grep("g1", "2026-01-01T00:00:02Z", "needle"),
            ),
            Lane::Main,
        );
        let sub = ingest_str(
            &grep("g0", "2026-01-01T00:00:01Z", "needle"),
            Lane::Sub("agent-a".into()),
        );
        // Hand-merge: interleave by timestamp, reindex — the minimal stand-in
        // for the flat-merge the real locate layer performs.
        let mut s = main;
        s.actions.extend(sub.actions);
        s.actions
            .sort_by(|a, b| (&a.effective_ts, &a.lane).cmp(&(&b.effective_ts, &b.lane)));
        for (i, a) in s.actions.iter_mut().enumerate() {
            a.idx = crate::model::Idx(i as u32);
        }
        assert!(action_loops(&s).is_empty(), "3 across lanes is not a loop");
    }

    #[test]
    fn interrupts_counted_in_session() {
        let raw = user("2026-01-01T00:00:01Z", "[Request interrupted by user]");
        assert_eq!(ingest_str(&raw, Lane::Main).interrupts, 1);
    }

    #[test]
    fn read_before_edit_share_is_a_ratio() {
        let raw = format!(
            "{}\n{}\n{}",
            read("1", "2026-01-01T00:00:01Z", "/a.ts"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts", "x", "y"), // read-first
            edit("3", "2026-01-01T00:00:03Z", "/b.ts", "x", "y"), // blind (no prior read)
        );
        let share = read_before_edit_share(&ingest_str(&raw, Lane::Main));
        assert!((share - 0.5).abs() < 1e-9, "1 of 2 edits was read-first");
    }
}
