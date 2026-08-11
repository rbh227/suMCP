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
    // Per (transcript, lane): a run in the main lane must not be broken (or
    // created) by an interleaved subagent doing its own thing. Grouping by
    // `.lane` alone would also be WRONG once several transcripts are merged:
    // every transcript has its own `Lane::Main`, so two unrelated transcripts
    // would be treated as one lane and their calls compared for a loop that
    // never happened. `lane_key()` folds in `session_ix` so lanes from
    // different transcripts never collide. Kept as a `BTreeSet` (not a `Vec`)
    // so each distinct key is visited once: a `Vec` would run this whole
    // O(n) pass once per ACTION instead of once per lane, and would emit the
    // same finding once per action in that lane.
    let keys: std::collections::BTreeSet<(u16, &crate::model::Lane)> =
        s.actions.iter().map(|a| a.lane_key()).collect();
    for key in keys {
        let lane_actions: Vec<&Action> = s.actions.iter().filter(|a| a.lane_key() == key).collect();
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
    /// Which transcript this segment belongs to. Every action in `actions`
    /// has this same `session_ix` (that is the whole point of a "segment" —
    /// see the invariant documented on `segments()` below). Also lets a
    /// consumer disambiguate `user_line_no`: once two transcripts are merged
    /// into one `Session`, "line 40" alone does not say which file it was.
    pub session_ix: u16,
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
/// A task segment belongs to one transcript: a segment boundary must only
/// ever be drawn from a user message in the SAME transcript as the action
/// being placed, never from another transcript merged into the same
/// `Session`. `line_no` is a position inside one transcript's own file, so
/// comparing it across transcripts is comparing two unrelated counters that
/// happen to overlap numerically.
///
/// Lifetime note for the Rust-learning reader: `Segment<'a>` borrows actions
/// from the `Session` — the `'a` says "these references live only as long as
/// the session does", which the compiler enforces for us.
pub(crate) fn segments(s: &Session) -> Vec<Segment<'_>> {
    let mut out: Vec<Segment<'_>> = Vec::new();

    // Group `user_texts` by transcript (`session_ix`) so each transcript's
    // messages are walked independently, in their own source order, with
    // their own cursor. A single shared cursor over the flat `user_texts`
    // list (the old approach) implicitly assumed line numbers were globally
    // increasing, which stops being true the moment two transcripts share a
    // `Session`. A work unit holds at most 16 transcripts (see
    // `Action::session_ix`'s doc), so a `Vec` here (not a `HashMap`, which
    // would make iteration order nondeterministic) costs nothing to scan.
    let mut by_session: Vec<(u16, Vec<&crate::model::UserText>)> = Vec::new();
    // `is_human` gates boundary drawing: a harness-injected turn (a task
    // notification) is recorded in `user_texts` but is not the human coming
    // back to review, so it must not close a task segment. Counting it as a
    // boundary truncated the review-burden window and understated the metric.
    for u in s.user_texts.iter().filter(|u| u.is_human) {
        match by_session.iter_mut().find(|(ix, _)| *ix == u.session_ix) {
            Some((_, msgs)) => msgs.push(u),
            None => by_session.push((u.session_ix, vec![u])),
        }
    }
    // next_unconsumed[i] indexes into by_session[i].1: how far that
    // transcript's own message stream has already been walked past.
    let mut next_unconsumed: Vec<usize> = vec![0; by_session.len()];

    // Rust-learning note: this is the "grouping while iterating" part that
    // looks like magic at first glance. `s.actions` holds every transcript's
    // main-lane actions interleaved together in one global order (sorted by
    // timestamp during merge, so transcript A's and transcript B's actions
    // can alternate arbitrarily). But a `Segment` must never mix two
    // transcripts (see the doc comment above this function). The fix is to
    // stop keeping ONE "currently open" segment and instead keep one PER
    // TRANSCRIPT, in `open`, so that pushing action `a` only ever touches
    // the segment that already belongs to `a.session_ix` — an action from
    // transcript B can arrive in between two transcript-A actions without
    // ever being able to land inside transcript A's open segment.
    //
    // `open` is a plain `Vec`, scanned linearly by `session_ix`, for the
    // same reason `by_session` above is: a work unit holds at most 16
    // transcripts (see `Action::session_ix`'s doc), so the scan costs
    // nothing, and a `HashMap` would make the final flush loop's order
    // nondeterministic, which is exactly what this codebase avoids in any
    // path that shapes output (see `action_loops`'s `BTreeSet`, not
    // `HashSet`, for the identical reasoning).
    let mut open: Vec<Segment<'_>> = Vec::new();

    for a in s.actions.iter().filter(|a| {
        // Deliberately `.lane`, not `.lane_key()`: this asks "is this a
        // main-agent action", which is a meaningful question in any
        // transcript, not "is this the same lane as some other action".
        a.lane == crate::model::Lane::Main
    }) {
        // Find this action's transcript's own in-progress segment, creating
        // an empty one (no boundary crossed yet, no actions yet) the first
        // time we ever see this session_ix — exactly how the single shared
        // `current` used to start out before its very first action.
        let slot = match open.iter().position(|seg| seg.session_ix == a.session_ix) {
            Some(i) => i,
            None => {
                open.push(Segment {
                    user_line_no: None,
                    session_ix: a.session_ix,
                    actions: Vec::new(),
                });
                open.len() - 1
            }
        };

        // Every user message from a's OWN transcript, strictly before this
        // action's line, opens a fresh segment (the LAST such message wins
        // when several are adjacent). A message from any OTHER transcript is
        // never even consulted, no matter what its line_no is.
        let mut crossed = None;
        if let Some(group) = by_session.iter().position(|(ix, _)| *ix == a.session_ix) {
            let msgs = &by_session[group].1;
            while next_unconsumed[group] < msgs.len()
                && msgs[next_unconsumed[group]].line_no < a.line_no
            {
                crossed = Some(msgs[next_unconsumed[group]].line_no);
                next_unconsumed[group] += 1;
            }
        }
        if let Some(line) = crossed {
            // Close out THIS TRANSCRIPT's running segment (if it saw any
            // actions, or already had a boundary of its own) and start a
            // fresh one in its slot, still for the same transcript. Other
            // transcripts' open segments are untouched by this.
            let finished = std::mem::replace(
                &mut open[slot],
                Segment {
                    user_line_no: Some(line),
                    session_ix: a.session_ix,
                    actions: Vec::new(),
                },
            );
            if !finished.actions.is_empty() || finished.user_line_no.is_some() {
                out.push(finished);
            }
        }
        open[slot].actions.push(a);
    }
    // Flush every transcript's still-open segment. For a single-transcript
    // session (session_ix 0 everywhere, the only case that exists today)
    // `open` has exactly one slot, so this is exactly the old single
    // `if !current.actions.is_empty() ...` flush — output is unchanged.
    for seg in open {
        if !seg.actions.is_empty() || seg.user_line_no.is_some() {
            out.push(seg);
        }
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

            // Include the transcript index alongside the line number: once
            // several transcripts are merged into one Session, "line 40"
            // alone is ambiguous (every transcript has its own line 40).
            let opened_by = match seg.user_line_no {
                Some(l) => {
                    format!(
                        "segment opened by user message at line {l} (transcript {})",
                        seg.session_ix
                    )
                }
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
            // same (transcript, lane), same file, and the later edit puts
            // back what the earlier removed. Lane guard (spec §5a): a revert
            // is one actor undoing its own change — a cross-lane content
            // coincidence on a shared path is not a revert, and a subagent
            // edit's line_no indexes a different file than the main-lane
            // pushback stream. `lane_key()` (not `.lane`) additionally keeps
            // this from firing across two merged transcripts: both have a
            // `Lane::Main`, so an edit in transcript A "restoring" what an
            // edit in transcript B removed is a coincidence of two separate
            // starting states, never a real revert.
            if earlier.lane_key() == later.lane_key()
                && earlier.file_path == later.file_path
                && later.edit_new == earlier.edit_old
            {
                // A flip needs BOTH: pushback in between, and no evidence
                // gathered between that pushback and the reverting edit.
                // Main lane ONLY: a Flip means capitulation to HUMAN pushback,
                // and pushback_between compares main-transcript line numbers.
                // A sub-lane edit's line_no indexes a different file, so those
                // numbers are incomparable — a sub self-revert stays a
                // TrueRevert (correct) and is never upgraded to a Flip (§5a).
                //
                // Deliberately `.lane`, not `.lane_key()`: this asks "is this
                // a main-agent action", which is a meaningful question in any
                // transcript. The pairing above already guaranteed both edits
                // come from the same transcript (via `lane_key()`), and that
                // is now also true of the pushback lookup itself: passing
                // `earlier.session_ix` (== `later.session_ix`) into
                // `pushback_between` makes it only ever consider user
                // messages FROM THAT SAME TRANSCRIPT, so a pushback message
                // from an unrelated merged transcript can never masquerade
                // as pushback here just because its `line_no` happens to
                // fall in the same numeric range.
                let is_flip = later.lane == crate::model::Lane::Main
                    && pushback_between(s, earlier.session_ix, earlier.line_no, later.line_no)
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

/// The first user message with pushback wording between two line numbers,
/// scoped to one transcript. `session_ix` must be the transcript both edits
/// belong to (the caller gets this for free, since `lane_key()` already
/// guaranteed the two edits share one). Line numbers are only ever
/// comparable WITHIN one transcript's own file: transcript B's line 5 has
/// nothing to do with transcript A's line 5, so without this filter a
/// pushback message from a completely different merged transcript could
/// numerically land "between" two edits it has nothing to do with.
fn pushback_between(
    s: &Session,
    session_ix: u16,
    lo: usize,
    hi: usize,
) -> Option<&crate::model::UserText> {
    s.user_texts.iter().find(|u| {
        u.session_ix == session_ix && u.line_no > lo && u.line_no < hi && {
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
/// pushback message), gated to `push.session_ix` (the SAME transcript the
/// pushback came from): `line_no` repeats per transcript once a work
/// unit merges several of them, so an unscoped comparison could credit a Read
/// from a totally unrelated transcript as "evidence" here. Subagent actions
/// live in other files entirely, so they compare by timestamp instead,
/// *strictly* between, so timestamp ties stay excluded per the
/// order-uncertain contract (SPEC decision 2) — but ALSO gated to
/// `push.session_ix`: timestamps stay globally meaningful across merged
/// transcripts (that comparison is well defined), but well defined is not
/// the same as wanted. If two transcripts in a work unit overlap in
/// wall-clock time, an ungated comparison would let a subagent Read spawned
/// by an unrelated transcript fall inside this window and count as
/// evidence, suppressing a genuine Flip. `merge_sessions` never rewrites
/// `session_ix`, so a sub-lane action still carries its own transcript's
/// index once assembly stamps it, and this gate relies on that.
fn evidence_between(s: &Session, push: &crate::model::UserText, later: &Action) -> bool {
    s.actions.iter().any(|a| {
        matches!(a.kind, ActionKind::Read | ActionKind::Bash)
            && a.session_ix == push.session_ix
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

    #[test]
    fn cross_lane_revert_is_not_flagged() {
        // A main edit and a subagent edit on the same path, where the sub's
        // new_string restores the main's old_string. Different actors → not a
        // revert. Same content within one lane WOULD fire (asserted below).
        use crate::model::{Action, ActionKind, Idx, Lane};
        let mk = |idx, lane, ts: &str, line, old: &str, new: &str| Action {
            idx: Idx(idx),
            effective_ts: ts.into(),
            ts_inherited: false,
            lane,
            session_ix: 0,
            line_no: line,
            kind: ActionKind::Edit,
            file_path: Some("/a".into()),
            is_error: None,
            write_len: None,
            write_lines: None,
            read_total_lines: None,
            input_hash: None,
            error: None,
            hunks: vec![],
            command: None,
            user_modified: false,
            edit_old: Some(old.into()),
            edit_new: Some(new.into()),
            approval_latency_s: None,
            auto_accept_here: false,
        };
        let mut s = crate::model::Session {
            actions: vec![
                mk(0, Lane::Main, "2026-01-01T00:00:01Z", 1, "foo", "bar"),
                mk(
                    1,
                    Lane::Sub("x".into()),
                    "2026-01-01T00:00:02Z",
                    1,
                    "bar",
                    "foo",
                ),
            ],
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
            session_ids: vec![],
            subagent_files_missing: 0,
        };
        assert!(
            reverts_and_flips(&s).is_empty(),
            "cross-lane pair must not be a revert"
        );

        // Same pair, both main lane → a true_revert fires.
        s.actions[1].lane = Lane::Main;
        assert_eq!(reverts_and_flips(&s).len(), 1, "same-lane revert must fire");
    }

    #[test]
    fn sub_lane_self_revert_stays_a_true_revert_never_a_flip() {
        // WHY: a Flip means "the agent capitulated to HUMAN pushback." That
        // upgrade compares the human pushback message's main-transcript line_no
        // against the edits' line_no. For a SUBAGENT self-revert those line
        // numbers index a different file entirely, so a human pushback whose
        // main-transcript line happens to fall between the two sub-edit line
        // numbers would falsely upgrade a legitimate subagent self-revert into a
        // "capitulation flip." The Flip upgrade must be gated to the main lane.
        // The self-revert itself is real, so it must STILL emit as TrueRevert —
        // only the Flip label is suppressed for sub lanes.
        use crate::model::{Action, ActionKind, Idx, UserText};
        let sub_edit = |idx, line, old: &str, new: &str, ts: &str| Action {
            idx: Idx(idx),
            effective_ts: ts.into(),
            ts_inherited: false,
            lane: Lane::Sub("x".into()),
            session_ix: 0,
            line_no: line,
            kind: ActionKind::Edit,
            file_path: Some("/a".into()),
            is_error: None,
            write_len: None,
            write_lines: None,
            read_total_lines: None,
            input_hash: None,
            error: None,
            hunks: vec![],
            command: None,
            user_modified: false,
            edit_old: Some(old.into()),
            edit_new: Some(new.into()),
            approval_latency_s: None,
            auto_accept_here: false,
        };
        let s = crate::model::Session {
            actions: vec![
                // Sub edits on the SAME sub file: line 1 then line 5 (these are
                // line numbers WITHIN the subagent's own transcript file).
                sub_edit(0, 1, "foo", "bar", "2026-01-01T00:00:01Z"),
                sub_edit(1, 5, "bar", "foo", "2026-01-01T00:00:03Z"), // restores "foo"
            ],
            // A human pushback in the MAIN transcript at line 3 — which lands
            // between the sub edits' 1 and 5 purely by coincidence. The word
            // "revert" is in the PUSHBACK list, so WITHOUT the lane gate this
            // would upgrade to a Flip.
            user_texts: vec![UserText {
                line_no: 3,
                text: "no revert that please".into(),
                effective_ts: "2026-01-01T00:00:02Z".into(),
                session_ix: 0,
                is_human: true,
            }],
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
            session_ids: vec![],
            subagent_files_missing: 0,
        };
        let f = reverts_and_flips(&s);
        assert_eq!(f.len(), 1, "the self-revert is real and must be reported");
        assert_eq!(
            f[0].kind,
            FindingKind::TrueRevert,
            "a subagent self-revert is never a capitulation to human pushback"
        );
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

    #[test]
    // Field-by-field construction (see the same allow in model.rs) keeps the
    // two or three fields each test actually cares about visible, instead of
    // burying them inside one long struct literal.
    #[allow(clippy::field_reassign_with_default)]
    fn true_revert_does_not_fire_across_two_sessions() {
        // Session 0 writes "a" -> "b". Session 1 later writes "b" -> "a".
        // Read as one lane that is a textbook revert. They are different
        // transcripts, so it must NOT fire: the second session simply had a
        // different starting state, and calling that a revert would invent a
        // struggle that never happened.
        let mut s = Session::default();
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("a".into());
        first.edit_new = Some("b".into());

        let mut second = Action::default();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:02Z".into();
        second.lane = Lane::Main;
        second.session_ix = 1; // the only difference that matters
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("b".into());
        second.edit_new = Some("a".into());

        s.actions = vec![first, second];
        s.session_ids = vec!["sess-0".into(), "sess-1".into()];

        let findings = reverts_and_flips(&s);
        assert!(
            findings.is_empty(),
            "a revert must never span two transcripts, got {findings:?}"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn true_revert_still_fires_within_one_session() {
        // The same shape, both actions in session 0: this IS a revert.
        let mut s = Session::default();
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("a".into());
        first.edit_new = Some("b".into());

        let mut second = first.clone();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:02Z".into();
        second.edit_old = Some("b".into());
        second.edit_new = Some("a".into());

        s.actions = vec![first, second];
        s.session_ids = vec!["sess-0".into()];

        assert_eq!(
            reverts_and_flips(&s).len(),
            1,
            "within one session it fires"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn pushback_in_another_transcript_does_not_create_a_flip() {
        // Session 0: edit A ("foo"->"bar") then edit B ("bar"->"foo") -- a
        // textbook revert. Session 1 has an unrelated user message with
        // pushback wording whose `line_no` (2) numerically falls between the
        // two session-0 edits' line numbers (1 and 3). `line_no` only means
        // "line inside ITS OWN transcript's file" (see `Action::lane_key`'s
        // doc), so a message from session 1 must never be read as pushback
        // between two session-0 edits -- doing so would invent a struggle
        // that never happened (the two transcripts are unrelated).
        use crate::model::UserText;
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.line_no = 1;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("foo".into());
        first.edit_new = Some("bar".into());

        let mut second = Action::default();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:03Z".into();
        second.lane = Lane::Main;
        second.session_ix = 0;
        second.line_no = 3;
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("bar".into());
        second.edit_new = Some("foo".into());

        let mut s = Session::default();
        s.actions = vec![first, second];
        s.user_texts = vec![UserText {
            effective_ts: "2026-01-01T00:00:02Z".into(),
            line_no: 2, // between 1 and 3 -- but in a DIFFERENT transcript
            text: "no, revert that".into(),
            session_ix: 1,
            is_human: true,
        }];
        s.session_ids = vec!["sess-0".into(), "sess-1".into()];

        let findings = reverts_and_flips(&s);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            FindingKind::TrueRevert,
            "a pushback from a DIFFERENT transcript must not manufacture a Flip, got {:?}",
            findings[0].kind
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn pushback_in_the_same_transcript_still_creates_a_flip() {
        // Positive control for the test above: identical shape, but the
        // pushback is in session 0 -- the SAME transcript as the two edits
        // -- so it must still upgrade the revert to a Flip. This proves the
        // scoping fix does not simply disable flip detection outright,
        // which is the obvious way to make the test above pass for the
        // wrong reason.
        use crate::model::UserText;
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.line_no = 1;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("foo".into());
        first.edit_new = Some("bar".into());

        let mut second = Action::default();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:03Z".into();
        second.lane = Lane::Main;
        second.session_ix = 0;
        second.line_no = 3;
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("bar".into());
        second.edit_new = Some("foo".into());

        let mut s = Session::default();
        s.actions = vec![first, second];
        s.user_texts = vec![UserText {
            effective_ts: "2026-01-01T00:00:02Z".into(),
            line_no: 2,
            text: "no, revert that".into(),
            session_ix: 0, // same transcript as the edits
            is_human: true,
        }];
        s.session_ids = vec!["sess-0".into()];

        let findings = reverts_and_flips(&s);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            FindingKind::Flip,
            "same-transcript pushback must still produce a Flip"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn flip_still_fires_when_edits_and_pushback_are_all_session_one() {
        // Every delivered test for this file so far places its edits and
        // pushback at session_ix 0. A wrong implementation that filters on
        // the LITERAL `u.session_ix == 0` (instead of comparing against the
        // edits' own session_ix, whatever it is) would pass all of those.
        // The merge step stamps real indices starting from 0 upward, so
        // that wrong version would break the moment a work unit's SECOND
        // transcript needed this exact detection. This test is the same
        // shape as `pushback_in_the_same_transcript_still_creates_a_flip`
        // above, moved entirely to session 1, to pin down that the scoping
        // compares against the actual session_ix rather than a hardcoded 0.
        use crate::model::UserText;
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 1;
        first.line_no = 1;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("foo".into());
        first.edit_new = Some("bar".into());

        let mut second = Action::default();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:03Z".into();
        second.lane = Lane::Main;
        second.session_ix = 1;
        second.line_no = 3;
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("bar".into());
        second.edit_new = Some("foo".into());

        let mut s = Session::default();
        s.actions = vec![first, second];
        s.user_texts = vec![UserText {
            effective_ts: "2026-01-01T00:00:02Z".into(),
            line_no: 2,
            text: "no, revert that".into(),
            session_ix: 1, // same transcript as the edits, but not 0
            is_human: true,
        }];
        s.session_ids = vec!["sess-0".into(), "sess-1".into()];

        let findings = reverts_and_flips(&s);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            FindingKind::Flip,
            "same-transcript pushback must still produce a Flip even when \
             the transcript is not session_ix 0"
        );
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn foreign_transcript_read_does_not_suppress_a_flip() {
        // The main-arm gate in `evidence_between` (`a.session_ix ==
        // push.session_ix`) has no test pinning it: every other test in this
        // file that exercises evidence-suppression uses zero Read/Bash
        // actions from a foreign transcript. Here a session-1 Read sits
        // between the session-0 pushback and the session-0 reverting edit,
        // at a line number that numerically falls in range. Without the
        // gate this foreign Read would count as "evidence gathered" and
        // wrongly downgrade the Flip to a TrueRevert.
        use crate::model::UserText;
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.line_no = 1;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("foo".into());
        first.edit_new = Some("bar".into());

        // Foreign-transcript Read: session_ix 1, line_no 2 -- numerically
        // between the pushback (line 2... see below) and the revert (line
        // 4), but from a DIFFERENT transcript than the pushback and edits.
        let mut foreign_read = Action::default();
        foreign_read.idx = Idx(1);
        foreign_read.effective_ts = "2026-01-01T00:00:02500Z".into();
        foreign_read.lane = Lane::Main;
        foreign_read.session_ix = 1;
        foreign_read.line_no = 3;
        foreign_read.kind = ActionKind::Read;
        foreign_read.file_path = Some("/unrelated.rs".into());

        let mut second = Action::default();
        second.idx = Idx(2);
        second.effective_ts = "2026-01-01T00:00:04Z".into();
        second.lane = Lane::Main;
        second.session_ix = 0;
        second.line_no = 4;
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("bar".into());
        second.edit_new = Some("foo".into());

        let mut s = Session::default();
        s.actions = vec![first, foreign_read, second];
        s.user_texts = vec![UserText {
            effective_ts: "2026-01-01T00:00:02Z".into(),
            line_no: 2,
            text: "no, revert that".into(),
            session_ix: 0,
            is_human: true,
        }];
        s.session_ids = vec!["sess-0".into(), "sess-1".into()];

        let findings = reverts_and_flips(&s);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            FindingKind::Flip,
            "a Read from an unrelated merged transcript must not count as \
             evidence and suppress this Flip"
        );
    }

    #[test]
    fn segments_do_not_draw_boundaries_from_another_transcripts_user_message() {
        // Two session-0 main actions at line_no 1 and 10. A session-1 user
        // message at line_no 5 numerically falls between them, but it
        // belongs to a DIFFERENT transcript, so it must not split the
        // segment: `line_no` only means "position in this transcript's own
        // file", never a global counter comparable across transcripts.
        use crate::model::UserText;
        let mk = |idx, line_no| Action {
            idx: Idx(idx),
            effective_ts: format!("2026-01-01T00:00:{line_no:02}Z"),
            ts_inherited: false,
            lane: Lane::Main,
            session_ix: 0,
            line_no,
            kind: ActionKind::Read,
            file_path: None,
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
        };
        let s = Session {
            actions: vec![mk(0, 1), mk(1, 10)],
            user_texts: vec![UserText {
                effective_ts: "2026-01-01T00:00:05Z".into(),
                line_no: 5,
                text: "an unrelated transcript's message".into(),
                session_ix: 1,
                is_human: true,
            }],
            session_ids: vec!["sess-0".into(), "sess-1".into()],
            ..Session::default()
        };
        let segs = segments(&s);
        assert_eq!(
            segs.len(),
            1,
            "a foreign-transcript user message must not split the segment"
        );
        assert_eq!(
            segs[0].user_line_no, None,
            "no same-transcript boundary was crossed"
        );
        assert_eq!(segs[0].actions.len(), 2);
    }

    #[test]
    fn opening_move_segments_never_mix_two_transcripts() {
        // Two transcripts interleaved by timestamp, with no user messages
        // (so the only thing separating their segments is `session_ix`).
        // Transcript 0 dives straight into an Edit; transcript 1 reads
        // first, then edits. Globally (ignoring session_ix) the very FIRST
        // action overall is transcript 1's Read, at t1 -- before
        // transcript 0's Edit at t2. If actions from both transcripts were
        // pushed into one shared segment (the bug this test pins down),
        // that shared segment's opening move would read "read-first",
        // laundering transcript 0's real patch-first opening into a false
        // negative -- and manufacturing a patch-first accusation is exactly
        // as possible depending on which transcript happens to go first.
        use crate::model::{Action, ActionKind, Idx};
        let mk = |idx: u32, session_ix: u16, ts: &str, kind: ActionKind| Action {
            idx: Idx(idx),
            effective_ts: ts.into(),
            ts_inherited: false,
            lane: Lane::Main,
            session_ix,
            line_no: idx as usize,
            kind,
            file_path: None,
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
        };
        let actions = vec![
            mk(0, 1, "2026-01-01T00:00:01Z", ActionKind::Read), // t1: transcript 1 reads first
            mk(1, 0, "2026-01-01T00:00:02Z", ActionKind::Edit), // t2: transcript 0's FIRST action is an edit
            mk(2, 1, "2026-01-01T00:00:03Z", ActionKind::Read), // t3
            mk(3, 0, "2026-01-01T00:00:04Z", ActionKind::Read), // t4
            mk(4, 1, "2026-01-01T00:00:05Z", ActionKind::Edit), // t5: transcript 1's edit, after its own read
            mk(5, 0, "2026-01-01T00:00:06Z", ActionKind::Read), // t6
            mk(6, 1, "2026-01-01T00:00:07Z", ActionKind::Read), // t7
            mk(7, 0, "2026-01-01T00:00:08Z", ActionKind::Read), // t8
            mk(8, 1, "2026-01-01T00:00:09Z", ActionKind::Read), // t9
            mk(9, 0, "2026-01-01T00:00:10Z", ActionKind::Read), // t10
        ];
        let s = Session {
            actions,
            session_ids: vec!["sess-0".into(), "sess-1".into()],
            ..Session::default()
        };

        let findings = opening_move(&s);
        assert_eq!(findings.len(), 2, "one opening-move finding per transcript");

        // Identify each finding by the transcript its FIRST cited action
        // actually belongs to -- not by scanning note text -- so this
        // cannot pass by an accident of wording.
        let session_of = |f: &Finding| -> u16 {
            s.actions
                .iter()
                .find(|a| a.idx == f.idxs[0])
                .expect("cited action exists")
                .session_ix
        };
        let t0 = findings
            .iter()
            .find(|f| session_of(f) == 0)
            .expect("transcript 0 has its own opening-move finding");
        let t1 = findings
            .iter()
            .find(|f| session_of(f) == 1)
            .expect("transcript 1 has its own opening-move finding");

        assert!(
            t0.note.as_ref().unwrap().contains("patch-first"),
            "transcript 0 dove straight into an Edit and must stay \
             patch-first even though transcript 1's Read sorts earlier \
             globally, got: {:?}",
            t0.note
        );
        assert!(
            t1.note.as_ref().unwrap().contains("read-first"),
            "transcript 1 genuinely read before its own edit, got: {:?}",
            t1.note
        );
    }
}
