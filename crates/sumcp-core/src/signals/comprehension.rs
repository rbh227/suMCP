//! Comprehension signals: the thesis layer — where the developer likely
//! shipped code they didn't read. **Explicitly heuristic** (SPEC decision 5):
//! every finding here is `exact: false`.
//!
//! Two signals with different suppression rules (2026-07-18 re-grounding):
//! - **Review burden** (#27, the layer's anchor): agent LOC between two human
//!   turns vs the 200–400 LOC human review band. Runs ALWAYS — including
//!   under auto-accept, which is precisely when nobody is gating the writes.
//! - **Large-write-instant-accept** (#16, corroborating): a timestamp delta
//!   cannot tell "read carefully" from "auto-accept" from "got coffee", so it
//!   is suppressed when the session ran under an auto-accept permission mode.

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Lane, Session, Tier};
use crate::signals::dynamics::segments;

/// A write this many chars or larger is "large".
const LARGE_WRITE_CHARS: usize = 2000;
/// Accepted within this many seconds counts as "instant".
const INSTANT_SECS: f64 = 3.0;
/// Ceiling of the human code-review band: "LOC under review should be under
/// 200, not to exceed 400" (SmartBear/Cisco study, ~2,500 reviews, 3.2M LOC;
/// defect detection falls from ~87% under 100 lines to ~28% over 1,000).
/// Above this, a human plausibly could not have reviewed the volume.
const REVIEW_BAND_HI: usize = 400;

/// Run the comprehension signals.
pub fn comprehension(s: &Session) -> Vec<Finding> {
    // Review burden first: it is the anchor, and it never suppresses.
    let mut out = review_burden(s);
    if !s.auto_accept {
        // Auto-accept in play ⇒ approval timing is meaningless. Say nothing
        // rather than emit false latency-based findings.
        out.extend(large_write_instant_accept(s));
    }
    out
}

/// Review-burden ratio (metrics-spec #27, the comprehension-layer anchor):
/// lines the agent wrote between two consecutive human turns, flagged when
/// they exceed the review band a human could plausibly keep up with.
///
/// Framed strictly as RISK — "this volume plausibly could not have been
/// reviewed" — never as a verdict that the human didn't read: the transcript
/// cannot see their editor (metrics-spec "NOT computable").
fn review_burden(s: &Session) -> Vec<Finding> {
    segments(s)
        .iter()
        .filter_map(|seg| {
            let edits: Vec<&Action> = seg
                .actions
                .iter()
                .filter(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write))
                .copied()
                .collect();
            let loc: usize = edits.iter().filter_map(|a| a.write_lines).sum();
            if loc <= REVIEW_BAND_HI {
                return None;
            }
            let mut nums = std::collections::BTreeMap::new();
            nums.insert("loc".into(), loc as f64);
            nums.insert("band_hi".into(), REVIEW_BAND_HI as f64);
            Some(Finding {
                kind: FindingKind::ReviewBurden,
                nums,
                tier: Tier::T1,
                exact: false, // risk inference, not an observed human behavior
                confidence: Confidence::Medium,
                note: Some(format!(
                    "{loc} lines written before the next human turn — above the \
                     200–400 LOC review band"
                )),
                idxs: edits.iter().map(|a| a.idx).collect(),
                file: None, // spans files; per-file detail via evidence(idxs)
            })
        })
        .collect()
}

/// Large writes accepted almost instantly — the canonical comprehension-debt
/// pattern (#16): a big diff went in faster than a human could have read it.
fn large_write_instant_accept(s: &Session) -> Vec<Finding> {
    s.actions
        .iter()
        // Main lane only: comprehension debt is a claim about the human, and a
        // subagent lane has no human gating its writes (design §5 corollary).
        .filter(|a| a.lane == Lane::Main && is_large_write(a) && accepted_instantly(a))
        .map(|a| {
            let latency = a.approval_latency_s.unwrap_or(0.0);
            let chars = a.write_len.unwrap_or(0);
            Finding {
                kind: FindingKind::LargeWriteInstantAccept,
                nums: Default::default(),
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
    // `Lane` comes in via `super::*` (now imported by the parent module).

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
    fn subagent_large_write_does_not_fire_but_main_does() {
        // WHY: comprehension debt is a claim about the HUMAN — "you shipped code
        // you didn't read." A subagent has no human gating its writes, so a big
        // fast subagent write is NOT comprehension debt (design §5 corollary:
        // comprehension signals consider main-lane actions only). This test
        // proves the lane filter is load-bearing: the SAME action fires on Main
        // and stays silent on a Sub lane.
        let raw = write_then_accept("w1", 5000, "2026-01-01T00:00:00Z", "2026-01-01T00:00:02Z");
        // Ingested as a subagent lane: the qualifying write exists, but no human.
        let mut s = ingest_str(&raw, Lane::Sub("x".into()));
        assert!(
            comprehension(&s).is_empty(),
            "a subagent lane has no human to owe comprehension — no finding"
        );
        // Flip the very same action to the main lane: now it must fire.
        s.actions[0].lane = Lane::Main;
        let f = comprehension(&s);
        assert_eq!(
            f.len(),
            1,
            "same write on the main lane IS comprehension debt"
        );
        assert_eq!(f[0].kind, FindingKind::LargeWriteInstantAccept);
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
    fn auto_accept_suppresses_latency_signals() {
        // 5000 chars on ONE line: large for latency purposes, but only 1 LOC,
        // so review burden stays quiet and suppression is what's under test.
        let mode = r#"{"type":"permission-mode","mode":"acceptEdits","sessionId":"s"}"#;
        let raw = format!(
            "{}\n{}",
            mode,
            write_then_accept("w1", 5000, "2026-01-01T00:00:00Z", "2026-01-01T00:00:01Z"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert!(s.auto_accept);
        assert!(comprehension(&s).is_empty(), "latency findings suppressed");
        assert!(!approval_latency_active(&s));
    }

    /// A Write whose content has `lines` lines, instantly accepted.
    fn write_lines_burst(id: &str, lines: usize, t0: &str, t1: &str) -> String {
        let content = vec!["x"; lines].join("\\n");
        let call = format!(
            r#"{{"type":"assistant","timestamp":"{t0}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"/a.ts","content":"{content}"}}}}]}}}}"#
        );
        let result = format!(
            r#"{{"type":"user","timestamp":"{t1}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}}}}"#
        );
        format!("{call}\n{result}")
    }
    fn user(ts: &str, text: &str) -> String {
        format!(r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"{text}"}}}}"#)
    }

    #[test]
    fn review_burden_fires_over_the_band_in_one_segment() {
        let raw = format!(
            "{}\n{}",
            user("2026-01-01T00:00:00Z", "build the feature"),
            write_lines_burst("w1", 500, "2026-01-01T00:00:01Z", "2026-01-01T00:05:00Z"),
        );
        let f = comprehension(&ingest_str(&raw, Lane::Main));
        let burden: Vec<_> = f
            .iter()
            .filter(|f| f.kind == FindingKind::ReviewBurden)
            .collect();
        assert_eq!(burden.len(), 1);
        assert_eq!(burden[0].nums["loc"], 500.0);
        assert_eq!(burden[0].nums["band_hi"], 400.0);
        let note = burden[0].note.as_deref().unwrap();
        assert!(
            !note.contains("unread") && !note.contains("didn't read"),
            "risk framing only, never a verdict: {note}"
        );
    }

    #[test]
    fn review_burden_respects_segment_boundaries() {
        // 250 + 250 lines, but a human turn between ⇒ neither segment exceeds.
        let raw = format!(
            "{}\n{}\n{}\n{}",
            user("2026-01-01T00:00:00Z", "part one"),
            write_lines_burst("w1", 250, "2026-01-01T00:00:01Z", "2026-01-01T00:02:00Z"),
            user("2026-01-01T00:03:00Z", "looks fine, part two"),
            write_lines_burst("w2", 250, "2026-01-01T00:03:01Z", "2026-01-01T00:05:00Z"),
        );
        let f = comprehension(&ingest_str(&raw, Lane::Main));
        assert!(
            !f.iter().any(|f| f.kind == FindingKind::ReviewBurden),
            "a human turn resets the review window"
        );
    }

    #[test]
    fn review_burden_fires_under_auto_accept() {
        // THE point of the anchor: auto-accept suppresses latency signals but
        // never review burden — that mode is when burden matters most.
        let mode = r#"{"type":"permission-mode","mode":"acceptEdits","sessionId":"s"}"#;
        let raw = format!(
            "{}\n{}\n{}",
            mode,
            user("2026-01-01T00:00:00Z", "go"),
            write_lines_burst("w1", 500, "2026-01-01T00:00:01Z", "2026-01-01T00:00:02Z"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert!(s.auto_accept);
        let f = comprehension(&s);
        assert_eq!(f.len(), 1, "burden only — instant-accept stays suppressed");
        assert_eq!(f[0].kind, FindingKind::ReviewBurden);
    }
}
