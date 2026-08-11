//! Review context: the five deterministic extractions a reviewing agent needs.
//!
//! Pure `&Session -> struct`, no JSON and no capping (that is `payloads.rs`).
//! The invariant this module exists to hold: suMCP reports what was recorded,
//! verbatim, with a citation. It never asserts that anything is acceptable or
//! risky, because that judgement belongs to the agent consuming this.

use crate::model::{ActionKind, Session};
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
