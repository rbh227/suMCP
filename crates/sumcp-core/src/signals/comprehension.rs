//! Comprehension signals (T3.4): the thesis layer — where the developer likely
//! shipped code they didn't read. **Explicitly heuristic** (SPEC decision 5):
//! a timestamp delta cannot tell "read carefully" from "auto-accept" from "got
//! coffee", so every finding here is `exact: false`, and the whole signal is
//! suppressed when the session ran under an auto-accept permission mode.

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Session, Tier};

/// A write this many chars or larger is "large".
const LARGE_WRITE_CHARS: usize = 2000;
/// Accepted within this many seconds counts as "instant".
const INSTANT_SECS: f64 = 3.0;

/// Run the comprehension signals. Returns empty when suppressed.
pub fn comprehension(s: &Session) -> Vec<Finding> {
    if s.auto_accept {
        // Auto-accept in play ⇒ approval timing is meaningless. Say nothing
        // rather than emit false comprehension-debt findings.
        return Vec::new();
    }
    large_write_instant_accept(s)
}

/// Large writes accepted almost instantly — the canonical comprehension-debt
/// pattern (#16): a big diff went in faster than a human could have read it.
fn large_write_instant_accept(s: &Session) -> Vec<Finding> {
    s.actions
        .iter()
        .filter(|a| is_large_write(a) && accepted_instantly(a))
        .map(|a| {
            let latency = a.approval_latency_s.unwrap_or(0.0);
            let chars = a.write_len.unwrap_or(0);
            Finding {
                kind: FindingKind::LargeWriteInstantAccept,
                tier: Tier::T2,
                exact: false, // heuristic: latency ≈ decision time, not proof
                confidence: Confidence::Medium,
                note: Some(format!(
                    "{chars}-char write accepted in {latency:.1}s — likely unread"
                )),
                idxs: vec![a.idx],
                file: a.file_path.clone(),
            }
        })
        .collect()
}

fn is_large_write(a: &Action) -> bool {
    matches!(a.kind, ActionKind::Edit | ActionKind::Write)
        && a.write_len.is_some_and(|n| n >= LARGE_WRITE_CHARS)
}

fn accepted_instantly(a: &Action) -> bool {
    a.approval_latency_s.is_some_and(|l| l <= INSTANT_SECS)
}

/// Whether approval-latency signals are active for this session (for the
/// `blind_spots` payload's `suppression` field).
pub fn approval_latency_active(s: &Session) -> bool {
    !s.auto_accept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    // A Write of `content`, then its result `dt` seconds later.
    fn write_then_accept(id: &str, content_len: usize, t0: &str, t1: &str) -> String {
        let content = "x".repeat(content_len);
        let call = format!(
            r#"{{"type":"assistant","timestamp":"{t0}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"/a.ts","content":"{content}"}}}}]}}}}"#
        );
        let result = format!(
            r#"{{"type":"user","timestamp":"{t1}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}}}}"#
        );
        format!("{call}\n{result}")
    }

    #[test]
    fn large_write_accepted_instantly_fires() {
        let raw = write_then_accept("w1", 5000, "2026-01-01T00:00:00Z", "2026-01-01T00:00:02Z");
        let f = comprehension(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::LargeWriteInstantAccept);
        assert!(!f[0].exact, "always heuristic");
    }

    #[test]
    fn small_write_does_not_fire() {
        let raw = write_then_accept("w1", 50, "2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z");
        assert!(comprehension(&ingest_str(&raw, Lane::Main)).is_empty());
    }

    #[test]
    fn slowly_reviewed_large_write_does_not_fire() {
        // 5 minutes to accept — the human plausibly read it.
        let raw = write_then_accept("w1", 5000, "2026-01-01T00:00:00Z", "2026-01-01T00:05:00Z");
        assert!(comprehension(&ingest_str(&raw, Lane::Main)).is_empty());
    }

    #[test]
    fn auto_accept_suppresses_everything() {
        let mode = r#"{"type":"permission-mode","mode":"acceptEdits","sessionId":"s"}"#;
        let raw = format!(
            "{}\n{}",
            mode,
            write_then_accept("w1", 5000, "2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert!(s.auto_accept);
        assert!(comprehension(&s).is_empty(), "suppressed under auto-accept");
        assert!(!approval_latency_active(&s));
    }
}
