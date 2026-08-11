//! Review context: the five deterministic extractions a reviewing agent needs.
//!
//! Pure `&Session -> struct`, no JSON and no capping (that is `payloads.rs`).
//! The invariant this module exists to hold: suMCP reports what was recorded,
//! verbatim, with a citation. It never asserts that anything is acceptable or
//! risky, because that judgement belongs to the agent consuming this.

use crate::model::{ActionKind, Idx, Lane, Session, TurnOrigin};
use crate::signals::failures::is_validation;
use std::collections::{BTreeMap, BTreeSet};

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
    /// Every EXPLICITLY human turn, in order. Only `TurnOrigin::Human`
    /// qualifies: a turn whose origin is merely `Unknown` is NOT quoted
    /// here, even though `UserText::is_human()` (a different consumer's
    /// question) would count it as human. This module's whole value is
    /// that what it quotes is ground truth, never inference, so an
    /// unattributed turn (measured on real transcripts: things like
    /// `"[Request interrupted by user]"` or a raw slash-command echo) must
    /// never be handed to a reviewer as "the human asked for this". See
    /// `unattributed_turns` below for where those turns are disclosed
    /// instead of silently dropped.
    pub requests: Vec<Request>,
    /// Paths this session successfully edited or wrote via Edit/Write tool
    /// calls, deduped and sorted. This is deliberately NOT an inference
    /// about which files the request "meant" to cover: it is the observed
    /// set, and a reviewer comparing it against the requests can draw its
    /// own conclusion about anything out of scope.
    ///
    /// This is NOT the commit's changed-file set. It excludes any file
    /// touched only through a Bash command (create/delete/rename never go
    /// through Edit/Write, so they never appear here), and a failed
    /// Edit/Write is excluded too (see the filter in `scope()`). A reviewer
    /// comparing this against a diff must read it as "what this session's
    /// own edit tools touched", a lower bound on session activity, never as
    /// a complete list of what changed.
    pub files_edited: Vec<String>,
    /// How many textual user turns had NO `origin` field at all
    /// (`TurnOrigin::Unknown`) and were, for that reason, excluded from
    /// `requests`. Disclosure instead of silence: without this a payload
    /// could show a suspiciously short request list with no sign that
    /// turns existed but could not be attributed. Same ethic as
    /// `Session::subagent_files_missing` and `Session::agent_texts_excluded`.
    /// A payload can say "N turns of unknown origin were not quoted".
    pub unattributed_turns: usize,
}

/// Extract what was asked and what was touched.
pub fn scope(s: &Session) -> Scope {
    let requests = s
        .user_texts
        .iter()
        // Only an EXPLICIT `origin.kind == "human"` is quoted as a request.
        // `TurnOrigin::NonHuman` (task notifications, hook output) is
        // correctly excluded, but so is `TurnOrigin::Unknown`: unlike
        // `UserText::is_human()`, this filter must NOT fold Unknown into
        // Human. Measured on 12 real transcripts (286 textual user turns):
        // 13.3% carry no `origin` at all, and sampling them shows they are
        // NOT old-format human requests. They are things like
        // "[Request interrupted by user]" and a raw slash-command echo.
        // Quoting those as the human's stated intent would be exactly the
        // failure this module exists to prevent.
        .filter(|u| matches!(u.origin, TurnOrigin::Human))
        .map(|u| Request {
            text: u.text.clone(),
            line_no: u.line_no,
            session_ix: u.session_ix,
        })
        .collect();

    let unattributed_turns = s
        .user_texts
        .iter()
        .filter(|u| matches!(u.origin, TurnOrigin::Unknown))
        .count();

    // BTreeSet gives dedup and sort in one step, which is what makes two runs
    // over an unchanged transcript byte-identical.
    let files_edited: BTreeSet<String> = s
        .actions
        .iter()
        .filter(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write))
        // A failed Edit/Write did not touch the file: the tool call was
        // attempted, not completed, so including it would tell a reviewer
        // this session changed a file it only tried to change. `is_error`
        // is `None` when the harness recorded no result at all (no matching
        // tool_result line, rare, but seen on truncated/streaming
        // transcripts); that case is INCLUDED here, matching how the rest
        // of this crate treats an unconfirmed action: absence of a
        // "failed" signal is not treated as a failure. Only a confirmed
        // `Some(true)` is excluded.
        .filter(|a| a.is_error != Some(true))
        .filter_map(|a| a.file_path.clone())
        .collect();

    Scope {
        requests,
        files_edited: files_edited.into_iter().collect(),
        unattributed_turns,
    }
}

/// How `decisions()` resolved a decision's asking action.
///
/// `(session_ix, line_no)` alone is not a unique key into `Session::actions`:
/// one assistant JSONL record can carry several `tool_use` blocks (an
/// AskUserQuestion call sitting next to an unrelated Bash or Edit in the
/// same message), and ingest creates one `Action` per block, all sharing
/// that record's `line_no`. Narrowing the search by kind too
/// (`ActionKind::Other("AskUserQuestion")`, since there is no dedicated
/// variant) makes a match unique in the overwhelmingly common case, but not
/// always: nothing stops two separate AskUserQuestion blocks landing on one
/// message, or the asking call being deduped away entirely as a replay. Zero
/// matches and more than one match are real, distinct outcomes, not bugs,
/// and collapsing either into a silently empty `idxs` would look exactly
/// like a clean resolution to a caller that only checks `is_empty()`. This
/// enum makes all three outcomes explicit instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one action matched `(session_ix, line_no, AskUserQuestion)`.
    /// `idxs` holds that one index.
    Resolved,
    /// No action matched. Possible if the asking call was deduped away as a
    /// replay. `idxs` is empty.
    NotFound,
    /// More than one action matched: two or more AskUserQuestion blocks were
    /// issued on the very same JSONL line, so the key cannot say which one
    /// this decision belongs to. `idxs` is empty rather than guessing.
    Ambiguous,
}

/// A recorded human choice, rendered for the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOut {
    /// The question, verbatim.
    pub question: String,
    /// What the human chose. `None` when the session ended unanswered, or
    /// when one call asked two questions with identical text and its answers
    /// map therefore cannot disambiguate them.
    pub chosen: Option<String>,
    /// Options that were offered and are not the answer text. This is NOT a
    /// claim that the human rejected them: `answer` can be arbitrary free
    /// text (the "Other" escape hatch lets the human answer outside the
    /// menu), so an answer like "SQLite with WAL" against an offered option
    /// "SQLite" still lands "SQLite" here, even though the human affirmed
    /// and refined that option rather than turning it down. Read this field
    /// as "not the literal answer string", never as "the human said no to
    /// this".
    pub options_not_chosen: Vec<String>,
    /// The asking action's index within the merged session: WHERE in the
    /// session timeline this decision happened, so a reviewer can order it
    /// against the edits and other actions around it.
    ///
    /// This is NOT a citation that `evidence()` can turn into excerpt text.
    /// `evidence()` only excerpts an action's `command`, `error`, or
    /// `edit_new`, and an AskUserQuestion action carries none of those, so
    /// `evidence(idxs)` on a decision returns the action's identity (idx,
    /// timestamp, tool name) with an empty excerpt string. That is fine:
    /// the decision block above is already self-contained (question,
    /// answer, and options verbatim), so nothing here needs to substantiate
    /// it further, only to place it in time.
    ///
    /// Empty exactly when `resolution` is not `Resolved`; see [`Resolution`]
    /// for what an empty `idxs` here actually means.
    pub idxs: Vec<Idx>,
    /// Source line, kept alongside `idxs` because it is the key the index was
    /// resolved from and it stays meaningful if resolution fails.
    pub line_no: usize,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
    /// How `idxs` was resolved. See [`Resolution`].
    pub resolution: Resolution,
}

/// Extract the recorded human decisions.
///
/// WHY THE INDEX IS RESOLVED HERE AND NOT AT INGEST (decided 2026-08-11 after
/// an adversarial review): both `merge_sessions` and `merge_work_unit`
/// globally renumber `Action::idx` after interleaving, so an index captured
/// during parsing would be stale by the time anything read it, and a stale
/// citation is worse than an absent one. `Decision` therefore stores only
/// `(session_ix, line_no)`, which never changes, and the index is looked up
/// here against the already-merged session. Correct by construction, with no
/// remapping step to forget.
///
/// WHY THE LOOKUP ALSO FILTERS BY KIND (decided 2026-08-11, second
/// adversarial review): `(session_ix, line_no)` on its own is not a unique
/// action key. One assistant JSONL record can carry several `tool_use`
/// blocks, and ingest gives each block its own `Action`, all sharing that
/// record's `line_no`. Without a kind filter, a decision would cite the
/// AskUserQuestion action AND every unrelated Bash or Edit issued in the
/// same message. There is no dedicated `ActionKind` variant for this tool,
/// so it lives in `Other("AskUserQuestion".to_string())`, matched by exact
/// string. `Action` deliberately does NOT also carry `tool_use_id` to make
/// this uniqueness airtight: that id is dropped after the ingest join, and
/// re-adding it costs one `String` per action across tens of thousands of
/// actions, just to cover the rare case (two AskUserQuestion blocks on one
/// line) that `Resolution::Ambiguous` already reports honestly instead.
pub fn decisions(s: &Session) -> Vec<DecisionOut> {
    s.decisions
        .iter()
        .map(|d| {
            let matches: Vec<Idx> = s
                .actions
                .iter()
                .filter(|a| {
                    a.session_ix == d.session_ix
                        && a.line_no == d.line_no
                        && a.kind == ActionKind::Other("AskUserQuestion".to_string())
                })
                .map(|a| a.idx)
                .collect();
            // Exactly one match is the only case treated as resolved. Zero
            // and "more than one" are represented explicitly rather than
            // both collapsing into an empty idxs, which would look like an
            // ordinary (if unlucky) resolution to any caller that only
            // checks is_empty().
            let (idxs, resolution) = match matches.len() {
                1 => (matches, Resolution::Resolved),
                0 => (Vec::new(), Resolution::NotFound),
                _ => (Vec::new(), Resolution::Ambiguous),
            };
            DecisionOut {
                question: d.question.clone(),
                chosen: d.answer.clone(),
                options_not_chosen: d
                    .options
                    .iter()
                    // Not the literal answer string. This is arithmetic, not
                    // a judgement about intent: a free-text answer (the
                    // "Other" escape hatch) can affirm and refine an option
                    // ("SQLite with WAL" against offered option "SQLite"),
                    // in which case that option still lands here even
                    // though the human chose it. See the field's doc
                    // comment on `DecisionOut`. An unanswered question
                    // (answer: None) leaves every option here too, since no
                    // choice was made at all to compare against.
                    .filter(|o| d.answer.as_deref().is_some_and(|a| a != o.as_str()))
                    .cloned()
                    .collect(),
                idxs,
                line_no: d.line_no,
                session_ix: d.session_ix,
                resolution,
            }
        })
        .collect()
}

/// A task's real identity for the replay below.
///
/// `TaskEvent::id`'s own doc comment explains why: each transcript's harness
/// numbers task ids from 1 independently, so a bare `id` is only unique
/// WITHIN one `(session_ix, lane)` pair, never across a merged work unit. A
/// main-lane task "1" and a subagent's task "1" are unrelated tasks that
/// merely happen to share a label; replaying on `id` alone would silently
/// fold them into one entry.
type TaskKey = (u16, Lane, String);

/// A task that was planned and never reached `completed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfinishedTask {
    /// The task id, exactly as the harness reported it. See [`TaskKey`] for
    /// why `session_ix` and `lane` below are needed alongside it to name one
    /// real task.
    pub id: String,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
    /// Main or subagent lane.
    pub lane: Lane,
    /// Its subject, if one was ever recorded.
    pub subject: Option<String>,
    /// The last status it reached.
    pub last_status: String,
    /// Source line of the last event about it, for citation.
    pub line_no: usize,
}

/// A validation command (per `is_validation`: test/lint/build/typecheck)
/// whose LAST recorded invocation explicitly failed.
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
    /// Tasks that never reached a terminal status (`completed` or
    /// `deleted`).
    pub unfinished_tasks: Vec<UnfinishedTask>,
    /// The last RECORDED outcome of each distinct validation command
    /// string (spec §"incomplete": "Bash actions matching the existing
    /// test/build regexes"), keyed by exact command text. This is NOT a
    /// claim that "the test suite is still failing" as a whole: a command
    /// with no paired result at all (its last invocation was retried and
    /// then interrupted, say) is neither passing nor failing, so it is
    /// simply absent from this list rather than guessed at either way. Two
    /// invocations that differ by even one flag (`cargo test` vs
    /// `cargo test --release`) are tracked as separate command strings,
    /// deliberately: no fuzzy grouping is attempted here.
    pub failing_commands: Vec<FailingCommand>,
}

/// Extract work that was planned or attempted and did not finish.
pub fn incomplete(s: &Session) -> Incomplete {
    // Replay the event list into a final state per (session_ix, lane, id) --
    // see `TaskKey`. BTreeMap keeps the output ordered by that key, so two
    // runs over an unchanged session agree byte for byte, the determinism
    // rule every extraction in this module follows.
    let mut final_state: BTreeMap<TaskKey, (Option<String>, String, usize)> = BTreeMap::new();
    for e in &s.task_events {
        let key = (e.session_ix, e.lane.clone(), e.id.clone());
        let entry = final_state.entry(key).or_insert((None, String::new(), 0));
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
        // "completed" and "deleted" are the terminal statuses measured on
        // real transcripts: `TaskUpdate` statuses observed across 15
        // sessions are `completed` (71), `in_progress` (48), and `deleted`
        // (1). A task the human explicitly deleted was cancelled, not left
        // undone, so it must not be reported as work still pending.
        // Nothing else is treated as finished; inventing a further terminal
        // state without evidence would risk quietly hiding real unfinished
        // work.
        .filter(|(_, (_, status, _))| status != "completed" && status != "deleted")
        .map(
            |((session_ix, lane, id), (subject, last_status, line_no))| UnfinishedTask {
                id,
                session_ix,
                lane,
                subject,
                last_status,
                line_no,
            },
        )
        .collect();

    // Last run wins, per distinct VALIDATION command string (per
    // `is_validation`: test/lint/build/typecheck). A non-validation command
    // (e.g. `rg nomatch`, which exits non-zero on no match) is not work in
    // progress at all, so it must never reach this replay.
    //
    // The state is tri-state, not boolean: `is_error` is `Some(true)`
    // (failed), `Some(false)` (passed), or `None` (no confirmed result --
    // the invocation was retried and then interrupted, say). Every
    // invocation of a command is replayed in order, last-run-wins, so an
    // unconfirmed retry SUPERSEDES an earlier confirmed failure instead of
    // being skipped in its favor: a validation command that failed, was
    // retried, and then interrupted has an unknown final outcome, not a
    // failed one. `failing_commands` is populated only from a final state
    // that is explicitly `Some(true)`; `None` emits nothing, since absence
    // of a result is not evidence of failure any more than of success.
    // `incomplete()` runs on the already-merged session, so `Action::idx` is
    // the final global index and safe to cite directly, unlike a task event
    // (which carries none, for the reason `TaskEvent`'s doc gives).
    let mut last_run: BTreeMap<String, (Option<bool>, Idx)> = BTreeMap::new();
    for a in &s.actions {
        if a.kind == ActionKind::Bash
            && let Some(cmd) = a.command.as_deref()
            && is_validation(cmd)
        {
            last_run.insert(cmd.to_string(), (a.is_error, a.idx));
        }
    }
    let failing_commands = last_run
        .into_iter()
        .filter(|(_, (err, _))| *err == Some(true))
        .map(|(command, (_, idx))| FailingCommand { command, idx })
        .collect();

    Incomplete {
        unfinished_tasks,
        failing_commands,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::merge::{merge_sessions, merge_work_unit};
    use crate::model::{Action, Decision, Lane};

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
        assert_eq!(scope.files_edited, vec!["/loader.rs".to_string()]);
    }

    #[test]
    fn scope_files_are_deduped_and_sorted() {
        // Deterministic output is a repo-wide rule: two runs over an
        // unchanged transcript must produce byte-identical payloads.
        let e1 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/b.rs","new_string":"x"}}]}}"#;
        let e2 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/a.rs","new_string":"y"}}]}}"#;
        let e3 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"e3","name":"Edit","input":{"file_path":"/b.rs","new_string":"z"}}]}}"#;
        let s = ingest_str(&format!("{e1}\n{e2}\n{e3}"), Lane::Main);

        assert_eq!(
            scope(&s).files_edited,
            vec!["/a.rs".to_string(), "/b.rs".to_string()]
        );
    }

    // ---- DEFECT 1 regressions: unattributed turns must never be quoted ----

    #[test]
    fn a_turn_with_no_origin_is_not_quoted_and_is_counted_unattributed() {
        // The adversarial review's core finding: `origin.kind` absent
        // entirely (not merely non-human) must not be quoted as the human's
        // request, because sampling real transcripts shows those lines are
        // things like interrupt markers and slash-command echoes, not
        // old-format human requests.
        let no_origin = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"<command-name>/clear</command-name>"}}"#;
        let s = ingest_str(no_origin, Lane::Main);

        let scope = scope(&s);
        assert_eq!(
            scope.requests.len(),
            0,
            "an origin-less turn must not be quoted as a request"
        );
        assert_eq!(
            scope.unattributed_turns, 1,
            "it must instead be disclosed as unattributed, not silently dropped"
        );
    }

    #[test]
    fn a_task_notification_is_neither_quoted_nor_counted_as_unattributed() {
        // A task notification HAS an origin: it is known non-human, not
        // unknown. Conflating "known non-human" with "unattributed" would
        // make the disclosure count lie about how many turns are genuinely
        // unaccounted for.
        let notification = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>agent done</task-notification>"}}"#;
        let s = ingest_str(notification, Lane::Main);

        let scope = scope(&s);
        assert_eq!(scope.requests.len(), 0, "not a human turn, not quoted");
        assert_eq!(
            scope.unattributed_turns, 0,
            "known non-human is not the same fact as unknown origin"
        );
    }

    #[test]
    fn an_interrupt_line_is_not_quoted_as_a_request() {
        // A real sample from the measurement that motivated this fix: an
        // interrupt marker carries no `origin` field and must not be handed
        // to a reviewer as something the human asked for.
        let interrupt = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"[Request interrupted by user]"}}"#;
        let s = ingest_str(interrupt, Lane::Main);

        let scope = scope(&s);
        assert_eq!(
            scope.requests.len(),
            0,
            "an interrupt marker is not a human request"
        );
        assert_eq!(scope.unattributed_turns, 1);
    }

    // ---- DEFECT 2 regression: a failed edit is not a touched file ----

    #[test]
    fn a_failed_edit_does_not_appear_in_files_edited() {
        let edit = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","old_string":"x","new_string":"y"}}]}}"#;
        let failed_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":true,"content":"old_string not found"}]}}"#;
        let s = ingest_str(&format!("{edit}\n{failed_result}"), Lane::Main);

        assert_eq!(
            scope(&s).files_edited,
            Vec::<String>::new(),
            "a failed Edit was attempted, not completed, so it must not appear as touched"
        );
    }

    // ---- decisions() ----

    #[test]
    fn decisions_report_what_was_chosen_and_what_it_beat() {
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"},{"label":"memory"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        let d = decisions(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].chosen.as_deref(), Some("JSONL"));
        assert_eq!(
            d[0].options_not_chosen,
            vec!["SQLite".to_string(), "memory".to_string()]
        );
    }

    #[test]
    fn a_free_text_answer_not_matching_any_option_lists_every_option_as_not_chosen() {
        // The human answered "Other" with text matching no option's label.
        // Every option lands in `options_not_chosen`: none of them is the
        // literal answer string. Framed as "not chosen", not "rejected",
        // because the field makes no claim about intent (see DEFECT 2 test
        // below for the case where that distinction actually matters).
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"keep it in memory"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        let d = decisions(&s);
        assert_eq!(d[0].chosen.as_deref(), Some("keep it in memory"));
        assert_eq!(
            d[0].options_not_chosen,
            vec!["SQLite".to_string(), "JSONL".to_string()]
        );
    }

    // ---- DEFECT 2 regression: options_not_chosen is not a rejection claim ----

    #[test]
    fn a_free_text_answer_that_contains_an_options_label_still_lists_it_as_not_chosen_this_is_not_a_rejection_claim()
     {
        // The human answered free text that AFFIRMS and refines one of the
        // offered options: "SQLite with WAL" picked SQLite, with detail.
        // `options_not_chosen` still lists "SQLite", because the field is
        // populated by exact string inequality against the answer, not by
        // any judgement about intent. That arithmetic is correct and
        // deliberately unchanged; what this test guards is the field's
        // NAME and doc comment no longer claiming the human turned SQLite
        // down; they did the opposite.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"SQLite with WAL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        let d = decisions(&s);
        assert_eq!(d[0].chosen.as_deref(), Some("SQLite with WAL"));
        assert_eq!(
            d[0].options_not_chosen,
            vec!["SQLite".to_string(), "JSONL".to_string()],
            "SQLite appears here only because it is not the literal answer \
             string; the human chose and refined it, so this is NOT a claim \
             that SQLite was rejected"
        );
    }

    // ---- DEFECT 1 regressions: (session_ix, line_no) is not a unique
    // action key, because one assistant JSONL record can carry several
    // tool_use blocks, all sharing that record's line_no. ----

    #[test]
    fn an_askuserquestion_sharing_a_line_with_a_bash_call_cites_only_the_askuserquestion() {
        // One assistant message, two tool_use blocks: AskUserQuestion and an
        // unrelated Bash call. Both become Actions with the same line_no.
        // Before this fix, the (session_ix, line_no) filter alone matched
        // both, so a decision's idxs cited the Bash action too, as if it
        // were part of answering the question.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}},{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"ls"}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        // Sanity check: both actions really do share one line_no.
        assert_eq!(s.actions.len(), 2);
        assert_eq!(s.actions[0].line_no, s.actions[1].line_no);
        let askuserquestion_idx = s
            .actions
            .iter()
            .find(|a| a.kind == ActionKind::Other("AskUserQuestion".to_string()))
            .expect("the asking action was ingested")
            .idx;

        let d = decisions(&s);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].resolution, Resolution::Resolved);
        assert_eq!(
            d[0].idxs,
            vec![askuserquestion_idx],
            "must cite only the AskUserQuestion action, never the Bash call \
             that happened to share its line"
        );
    }

    #[test]
    fn a_decision_whose_asking_action_is_absent_reports_not_found_not_an_empty_vec() {
        // Before this fix, zero matches and the (impossible before this fix)
        // exactly-one-match case both produced idxs: vec![], indistinguishable
        // to any caller checking is_empty(). Built by hand rather than via
        // ingest_str, standing in for a decision whose asking call was
        // deduped away as a replay: the decision is recorded, but no action
        // at that (session_ix, line_no, kind) exists to resolve it to.
        let d = Decision {
            question: "Which store?".to_string(),
            options: vec!["SQLite".to_string(), "JSONL".to_string()],
            answer: Some("JSONL".to_string()),
            line_no: 5,
            session_ix: 0,
        };
        let s = Session {
            decisions: vec![d],
            actions: vec![Action {
                // A real action exists, but at a different line_no, so it
                // must not match.
                line_no: 99,
                kind: ActionKind::Other("AskUserQuestion".to_string()),
                ..Action::default()
            }],
            ..Session::default()
        };

        let out = decisions(&s);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].resolution, Resolution::NotFound);
        assert!(out[0].idxs.is_empty());
    }

    #[test]
    fn two_askuserquestion_calls_in_one_message_report_ambiguous() {
        // Two SEPARATE tool_use blocks, both named AskUserQuestion, on the
        // very same assistant message. Both become Actions sharing the same
        // line_no and the same Other("AskUserQuestion") kind, so kind-plus-
        // (session_ix, line_no) still cannot tell them apart. Rather than
        // guessing (e.g. citing the first one found), decisions() must say
        // so explicitly.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"}]}]}},{"type":"tool_use","id":"q2","name":"AskUserQuestion","input":{"questions":[{"question":"Which cache?","options":[{"label":"LRU"}]}]}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"},{"type":"tool_result","tool_use_id":"q2"}]},"toolUseResult":{"answers":{"Which store?":"SQLite","Which cache?":"LRU"}}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        assert_eq!(
            s.decisions.len(),
            2,
            "each AskUserQuestion block recorded its own decision"
        );

        let d = decisions(&s);
        assert_eq!(d.len(), 2);
        for out in &d {
            assert_eq!(
                out.resolution,
                Resolution::Ambiguous,
                "two AskUserQuestion actions on one line: neither decision can \
                 be resolved to a single asking action"
            );
            assert!(out.idxs.is_empty());
        }
    }

    #[test]
    fn a_decisions_idxs_survives_renumbering_by_merge_work_unit() {
        // The whole point of resolving idxs here, against the already-merged
        // session, rather than storing an index at parse time: both merge
        // functions globally renumber Action::idx after interleaving two
        // transcripts, so a parse-time index would go stale. Build a
        // two-transcript work unit where the decision comes from the SECOND
        // transcript (whose own pre-merge idx is 0, since it is that
        // transcript's only action) and assert the resolved idxs points at
        // the post-merge global idx, not the pre-merge local one.
        let first_call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","old_string":"x","new_string":"y"}}]}}"#;
        let first = ingest_str(first_call, Lane::Main);

        let question_call = r#"{"type":"assistant","timestamp":"2026-01-01T00:01:00Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let question_result = r#"{"type":"user","timestamp":"2026-01-01T00:01:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let second = ingest_str(&format!("{question_call}\n{question_result}"), Lane::Main);

        // Sanity check: within its own transcript, the asking action's idx
        // is 0, the pre-merge local value that must NOT leak into the result.
        assert_eq!(second.actions[0].idx, Idx(0));

        let merged = merge_work_unit(vec![
            ("first".to_string(), first),
            ("second".to_string(), second),
        ]);

        // Post-merge, the first transcript's edit sorts before the second
        // transcript's question (earlier timestamp), so the question's
        // global idx is 1, not its pre-merge local 0.
        assert_eq!(merged.actions.len(), 2);
        let asking_idx = merged
            .actions
            .iter()
            .find(|a| a.kind == crate::model::ActionKind::Other("AskUserQuestion".to_string()))
            .expect("the asking action survives the merge")
            .idx;
        assert_eq!(asking_idx, Idx(1));

        let d = decisions(&merged);
        assert_eq!(d.len(), 1);
        assert_eq!(
            d[0].session_ix, 1,
            "the decision came from the second transcript"
        );
        assert_eq!(
            d[0].idxs,
            vec![Idx(1)],
            "idxs must resolve to the post-merge global idx, not the pre-merge local 0"
        );
    }

    // ---- incomplete() ----

    #[test]
    fn a_task_never_completed_is_reported_unfinished() {
        let c1 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"Wire the cache"}}]}}"#;
        let c1_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]},"toolUseResult":{"task":{"id":"1","subject":"Wire the cache"}}}"#;
        let c2 = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        let c2_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b"}]},"toolUseResult":{"task":{"id":"2","subject":"Add tests"}}}"#;
        let done = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"c","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let done_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"c"}]},"toolUseResult":{"success":true,"taskId":"1","statusChange":"completed"}}"#;
        let s = ingest_str(
            &format!("{c1}\n{c1_result}\n{c2}\n{c2_result}\n{done}\n{done_result}"),
            Lane::Main,
        );

        let inc = incomplete(&s);
        assert_eq!(inc.unfinished_tasks.len(), 1, "task 2 never completed");
        assert_eq!(
            inc.unfinished_tasks[0].subject.as_deref(),
            Some("Add tests")
        );
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

    #[test]
    fn a_main_task_and_a_subagent_task_sharing_id_one_are_not_merged() {
        // The brief this task was built from replayed task events keyed on
        // `id` alone -- exactly the bug `TaskEvent::id`'s doc comment warns
        // about. Each transcript's harness numbers its own tasks from 1, so
        // a main-lane task "1" and a subagent's task "1" are unrelated
        // tasks that merely share a label. Keying the replay on the bare id
        // would silently fold them into one entry: whichever event happened
        // to come last in `s.task_events` would then decide whether the
        // completed main task or the still-open subagent task "won", and
        // the other would vanish. Build one of each, with the main one
        // completed and the subagent one not, and require exactly one
        // unfinished task out the other end: not zero (both cancel out) and
        // not one merged entry with the wrong status.
        let main_create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"Main task"}}]}}"#;
        let main_create_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]},"toolUseResult":{"task":{"id":"1","subject":"Main task"}}}"#;
        let main_done = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b","name":"TaskUpdate","input":{"taskId":"1","status":"completed"}}]}}"#;
        let main_done_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b"}]},"toolUseResult":{"success":true,"taskId":"1","statusChange":"completed"}}"#;
        let main = ingest_str(
            &format!("{main_create}\n{main_create_result}\n{main_done}\n{main_done_result}"),
            Lane::Main,
        );

        let sub_create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"Subagent task"}}]}}"#;
        let sub_create_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]},"toolUseResult":{"task":{"id":"1","subject":"Subagent task"}}}"#;
        let sub = ingest_str(
            &format!("{sub_create}\n{sub_create_result}"),
            Lane::Sub("agent-1".to_string()),
        );

        let merged = merge_sessions(main, vec![sub], 0);
        let inc = incomplete(&merged);

        assert_eq!(
            inc.unfinished_tasks.len(),
            1,
            "one unfinished task: the subagent's, not zero and not a merged single entry"
        );
        assert_eq!(
            inc.unfinished_tasks[0].subject.as_deref(),
            Some("Subagent task")
        );
        assert_eq!(
            inc.unfinished_tasks[0].lane,
            Lane::Sub("agent-1".to_string())
        );
        assert_eq!(inc.unfinished_tasks[0].last_status, "pending");
    }

    // ---- DEFECT: failing_commands must only replay VALIDATION commands ----

    #[test]
    fn a_failing_non_validation_command_does_not_appear_in_failing_commands() {
        // `rg nomatch` fails (exit 1 on no match) but is not a test, lint,
        // build, or typecheck invocation. Reporting it to a reviewer as
        // unfinished work would be exactly the false positive `is_validation`
        // exists to prevent.
        let call = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"rg nomatch"}}]}}"#;
        let result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#;
        let s = ingest_str(&format!("{call}\n{result}"), Lane::Main);

        assert!(
            incomplete(&s).failing_commands.is_empty(),
            "a failing non-validation command must not be reported"
        );
    }

    // ---- DEFECT: "deleted" is a terminal status, same as "completed" ----

    #[test]
    fn a_task_confirmed_deleted_is_not_reported_as_unfinished() {
        let create = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"Scratch task"}}]}}"#;
        let create_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"a"}]},"toolUseResult":{"task":{"id":"1","subject":"Scratch task"}}}"#;
        let delete = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b","name":"TaskUpdate","input":{"taskId":"1","status":"deleted"}}]}}"#;
        let delete_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b"}]},"toolUseResult":{"success":true,"taskId":"1","statusChange":"deleted"}}"#;
        let s = ingest_str(
            &format!("{create}\n{create_result}\n{delete}\n{delete_result}"),
            Lane::Main,
        );

        assert!(
            incomplete(&s).unfinished_tasks.is_empty(),
            "a task the human explicitly deleted was cancelled, not left undone"
        );
    }

    // ---- DEFECT: replay must be tri-state (passed/failed/unknown), last-run-wins ----

    #[test]
    fn a_validation_command_retried_with_no_paired_result_has_unknown_final_outcome_and_does_not_appear()
     {
        // First invocation of a validation command fails. It is retried, but
        // the retry's tool_use has no matching tool_result at all (the
        // transcript was interrupted mid-call). The final recorded outcome
        // is therefore unknown, not failed: the old `let Some(err)` guard
        // skipped the unconfirmed retry entirely and let the earlier failure
        // stand as if it were the last word.
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#;
        // The retry: a tool_use with no corresponding tool_result line.
        let retry = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let s = ingest_str(&format!("{fail}\n{fail_r}\n{retry}"), Lane::Main);

        // Sanity check: the retry really is unconfirmed.
        let retry_action = s
            .actions
            .iter()
            .find(|a| a.command.as_deref() == Some("cargo test") && a.is_error.is_none());
        assert!(
            retry_action.is_some(),
            "the retry must be an action with no confirmed result"
        );

        assert!(
            incomplete(&s).failing_commands.is_empty(),
            "the final invocation's outcome is unknown, not failed, so nothing is reported"
        );
    }

    #[test]
    fn a_validation_command_that_fails_then_passes_on_an_identical_retry_does_not_appear() {
        let fail = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let fail_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true}]}}"#;
        let pass = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let pass_r = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"b2","is_error":false}]}}"#;
        let s = ingest_str(&format!("{fail}\n{fail_r}\n{pass}\n{pass_r}"), Lane::Main);

        assert!(
            incomplete(&s).failing_commands.is_empty(),
            "the last recorded outcome of this command is a pass"
        );
    }
}
