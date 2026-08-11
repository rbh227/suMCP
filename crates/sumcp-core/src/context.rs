//! Review context: the five deterministic extractions a reviewing agent needs.
//!
//! Pure `&Session -> struct`, no JSON and no capping (that is `payloads.rs`).
//! The invariant this module exists to hold: suMCP reports what was recorded,
//! verbatim, with a citation. It never asserts that anything is acceptable or
//! risky, because that judgement belongs to the agent consuming this.

use crate::model::{ActionKind, Idx, Session, TurnOrigin};
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

/// A recorded human choice, rendered for the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionOut {
    /// The question, verbatim.
    pub question: String,
    /// What the human chose. `None` when the session ended unanswered, or
    /// when one call asked two questions with identical text and its answers
    /// map therefore cannot disambiguate them.
    pub chosen: Option<String>,
    /// The options that were turned down. When the answer was free text
    /// matching no option, every option is here, which is the correct
    /// reading: nothing on the menu was picked.
    pub rejected: Vec<String>,
    /// The asking action's index, so `evidence(idxs)` resolves this decision
    /// to the raw transcript. Empty when no action matches, which is
    /// possible if the asking call was deduped away as a replay.
    pub idxs: Vec<Idx>,
    /// Source line, kept alongside `idxs` because it is the key the index was
    /// resolved from and it stays meaningful if resolution fails.
    pub line_no: usize,
    /// Which transcript of the work unit it came from.
    pub session_ix: u16,
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
            idxs: s
                .actions
                .iter()
                .filter(|a| a.session_ix == d.session_ix && a.line_no == d.line_no)
                .map(|a| a.idx)
                .collect(),
            line_no: d.line_no,
            session_ix: d.session_ix,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::merge::merge_work_unit;
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
        assert_eq!(d[0].session_ix, 1, "the decision came from the second transcript");
        assert_eq!(
            d[0].idxs,
            vec![Idx(1)],
            "idxs must resolve to the post-merge global idx, not the pre-merge local 0"
        );
    }
}
