//! Edit-shape signals (T3.1): churn, rework, re-read thrash, blind-write
//! attempts. All deterministic counters over the ordered actions — the
//! empirically load-bearing struggle signals (SPEC §1 amendment 3).

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};
use std::collections::BTreeMap;

/// A file is "churned" once it has been edited this many times or more.
const CHURN_MIN_EDITS: usize = 2;
/// A file is "thrashed" once it has been re-read this many times or more.
const THRASH_MIN_READS: usize = 3;
/// The harness error that marks a blind-write attempt (SPEC §1 amendment 1).
const BLIND_WRITE_ERROR: &str = "File has not been read yet";

/// Run every edit-shape signal over the session.
pub fn edit_shape(s: &Session) -> Vec<Finding> {
    let mut findings = Vec::new();
    findings.extend(churn(s));
    findings.extend(rework(s));
    findings.extend(thrash(s));
    findings.extend(blind_write_attempts(s));
    findings
}

/// Group action indices by file for actions of a given kind. Uses a BTreeMap
/// so iteration order is deterministic (stable findings, stable `Idx`s).
fn by_file<'a>(s: &'a Session, want: &dyn Fn(&ActionKind) -> bool) -> BTreeMap<&'a str, Vec<Idx>> {
    let mut map: BTreeMap<&str, Vec<Idx>> = BTreeMap::new();
    for a in &s.actions {
        if want(&a.kind)
            && let Some(f) = a.file_path.as_deref()
        {
            map.entry(f).or_default().push(a.idx);
        }
    }
    map
}

/// Churn: files edited (Edit/Write) `CHURN_MIN_EDITS`+ times.
///
/// When the file's size is known (see [`last_known_total_lines`]), the finding
/// also carries `nums["relative_churn"]` = churned lines / file lines —
/// size-normalized churn predicts defects where absolute churn does not
/// (Nagappan & Ball, ICSE 2005; metrics-spec #7). Heuristic refinement: the
/// denominator can be stale after edits, so raw count stays the headline.
fn churn(s: &Session) -> Vec<Finding> {
    by_file(s, &|k| matches!(k, ActionKind::Edit | ActionKind::Write))
        .into_iter()
        .filter(|(_, idxs)| idxs.len() >= CHURN_MIN_EDITS)
        .map(|(file, idxs)| {
            let mut nums = BTreeMap::new();
            if let Some(total) = last_known_total_lines(s, file) {
                let churned: usize = s
                    .actions
                    .iter()
                    .filter(|a| {
                        matches!(a.kind, ActionKind::Edit | ActionKind::Write)
                            && a.file_path.as_deref() == Some(file)
                    })
                    .filter_map(|a| a.write_lines)
                    .sum();
                if churned > 0 {
                    nums.insert("relative_churn".into(), churned as f64 / total as f64);
                }
            }
            Finding {
                kind: FindingKind::Churn,
                nums,
                tier: Tier::T1,
                exact: true,
                confidence: Confidence::High,
                note: Some(format!("edited {} times", idxs.len())),
                idxs,
                file: Some(file.to_string()),
            }
        })
        .collect()
}

/// The most recent file size the harness reported for `file` — the
/// `totalLines` of its latest Read result. `None` when the file was never
/// read with a size-bearing result (then relative churn is unknowable).
fn last_known_total_lines(s: &Session, file: &str) -> Option<usize> {
    s.actions
        .iter()
        .rev() // latest Read wins — sizes drift as the session edits the file
        .find(|a| {
            matches!(a.kind, ActionKind::Read)
                && a.file_path.as_deref() == Some(file)
                && a.read_total_lines.is_some()
        })
        .and_then(|a| a.read_total_lines)
}

/// Thrash: files re-read `THRASH_MIN_READS`+ times (losing the mental model).
fn thrash(s: &Session) -> Vec<Finding> {
    by_file(s, &|k| matches!(k, ActionKind::Read))
        .into_iter()
        .filter(|(_, idxs)| idxs.len() >= THRASH_MIN_READS)
        .map(|(file, idxs)| Finding {
            kind: FindingKind::ReRead,
            nums: Default::default(),
            tier: Tier::T1,
            exact: true,
            confidence: Confidence::High,
            note: Some(format!("re-read {} times", idxs.len())),
            idxs,
            file: Some(file.to_string()),
        })
        .collect()
}

/// Rework: a later edit whose patch hunks overlap an earlier edit's — the
/// coherence-collapse signature (editing the right place, repeatedly).
fn rework(s: &Session) -> Vec<Finding> {
    let mut findings = Vec::new();
    // Collect edits (with hunks) per file, preserving order.
    let mut edits_by_file: BTreeMap<&str, Vec<&Action>> = BTreeMap::new();
    for a in &s.actions {
        if matches!(a.kind, ActionKind::Edit)
            && !a.hunks.is_empty()
            && let Some(f) = a.file_path.as_deref()
        {
            edits_by_file.entry(f).or_default().push(a);
        }
    }
    for (file, edits) in edits_by_file {
        // Compare each later edit against every earlier one on the same file.
        for (i, later) in edits.iter().enumerate() {
            for earlier in &edits[..i] {
                if hunks_overlap(&earlier.hunks, &later.hunks) {
                    findings.push(Finding {
                        kind: FindingKind::Rework,
                        nums: Default::default(),
                        tier: Tier::T2,
                        exact: true,
                        confidence: Confidence::High,
                        note: Some("later edit overlaps an earlier edit's lines".into()),
                        idxs: vec![earlier.idx, later.idx],
                        file: Some(file.to_string()),
                    });
                    break; // one rework finding per later edit is enough
                }
            }
        }
    }
    findings
}

/// Do any two line ranges overlap? Ranges are `(start, end)` half-open-ish;
/// `a < d && c < b` is the standard interval-overlap test.
fn hunks_overlap(a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
    a.iter()
        .any(|&(a0, a1)| b.iter().any(|&(b0, b1)| a0 < b1 && b0 < a1))
}

/// Blind-write attempts: an Edit the harness rejected because the file had not
/// been read. Real edits can't be blind (the harness enforces read-first), so
/// we count the *attempt* via the error string.
fn blind_write_attempts(s: &Session) -> Vec<Finding> {
    s.actions
        .iter()
        .filter(|a| {
            matches!(a.kind, ActionKind::Edit | ActionKind::Write)
                && a.error
                    .as_deref()
                    .is_some_and(|e| e.contains(BLIND_WRITE_ERROR))
        })
        .map(|a| Finding {
            kind: FindingKind::BlindWriteAttempt,
            nums: Default::default(),
            tier: Tier::T1,
            exact: true,
            confidence: Confidence::High,
            note: Some(BLIND_WRITE_ERROR.to_string()),
            idxs: vec![a.idx],
            file: a.file_path.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    fn edit(id: &str, ts: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file}","new_string":"x"}}}}]}}}}"#
        )
    }
    fn read(id: &str, ts: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
        )
    }

    #[test]
    fn churn_fires_on_repeat_edits_of_one_file() {
        let raw = format!(
            "{}\n{}\n{}",
            edit("1", "2026-01-01T00:00:01Z", "/a.ts"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts"),
            edit("3", "2026-01-01T00:00:03Z", "/b.ts"),
        );
        let f = churn(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1, "only /a.ts (edited twice) churns");
        assert_eq!(f[0].file.as_deref(), Some("/a.ts"));
        assert_eq!(f[0].idxs.len(), 2);
    }

    /// An Edit whose new_string has `lines` lines, plus a sized Read result.
    fn edit_lines(id: &str, ts: &str, file: &str, lines: usize) -> String {
        let content = vec!["x"; lines].join("\\n");
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file}","new_string":"{content}"}}}}]}}}}"#
        )
    }
    fn sized_read(id: &str, ts: &str, file: &str, total: usize) -> String {
        format!(
            concat!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#,
                "\n",
                r#"{{"type":"user","timestamp":"{ts}","message":{{"content":[{{"type":"tool_result","tool_use_id":"{id}","is_error":false}}]}},"toolUseResult":{{"type":"text","file":{{"filePath":"{file}","totalLines":{total}}}}}}}"#,
            ),
            ts = ts,
            id = id,
            file = file,
            total = total,
        )
    }

    #[test]
    fn relative_churn_uses_last_known_file_size() {
        // 300-line file, three 10-line edits ⇒ relative churn 30/300 = 0.1
        let raw = format!(
            "{}\n{}\n{}\n{}",
            sized_read("r1", "2026-01-01T00:00:01Z", "/a.ts", 300),
            edit_lines("e1", "2026-01-01T00:00:02Z", "/a.ts", 10),
            edit_lines("e2", "2026-01-01T00:00:03Z", "/a.ts", 10),
            edit_lines("e3", "2026-01-01T00:00:04Z", "/a.ts", 10),
        );
        let f = churn(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert!((f[0].nums["relative_churn"] - 0.1).abs() < 1e-9);
    }

    #[test]
    fn relative_churn_absent_without_a_sized_read() {
        let raw = format!(
            "{}\n{}",
            edit_lines("e1", "2026-01-01T00:00:01Z", "/a.ts", 10),
            edit_lines("e2", "2026-01-01T00:00:02Z", "/a.ts", 10),
        );
        let f = churn(&ingest_str(&raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert!(
            !f[0].nums.contains_key("relative_churn"),
            "no denominator ⇒ raw count only"
        );
    }

    #[test]
    fn relative_churn_denominator_tracks_the_latest_read() {
        // First read says 100 lines, a later one says 400 — the 400 wins.
        let raw = format!(
            "{}\n{}\n{}\n{}",
            sized_read("r1", "2026-01-01T00:00:01Z", "/a.ts", 100),
            edit_lines("e1", "2026-01-01T00:00:02Z", "/a.ts", 20),
            edit_lines("e2", "2026-01-01T00:00:03Z", "/a.ts", 20),
            sized_read("r2", "2026-01-01T00:00:04Z", "/a.ts", 400),
        );
        let f = churn(&ingest_str(&raw, Lane::Main));
        assert!((f[0].nums["relative_churn"] - 0.1).abs() < 1e-9, "40/400");
    }

    #[test]
    fn thrash_needs_three_reads() {
        let two = format!(
            "{}\n{}",
            read("1", "2026-01-01T00:00:01Z", "/a.ts"),
            read("2", "2026-01-01T00:00:02Z", "/a.ts"),
        );
        assert!(
            thrash(&ingest_str(&two, Lane::Main)).is_empty(),
            "2 reads is not thrash"
        );
        let three = format!("{two}\n{}", read("3", "2026-01-01T00:00:03Z", "/a.ts"));
        assert_eq!(thrash(&ingest_str(&three, Lane::Main)).len(), 1);
    }

    #[test]
    fn rework_fires_on_overlapping_hunks_and_is_quiet_otherwise() {
        // Two edits to /a.ts touching overlapping line ranges (10-15, 12-18).
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":false}]},"toolUseResult":{"structuredPatch":[{"oldStart":10,"oldLines":5}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/a.ts","new_string":"y"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"e2","is_error":false}]},"toolUseResult":{"structuredPatch":[{"oldStart":12,"oldLines":6}]}}"#,
        );
        let f = rework(&ingest_str(raw, Lane::Main));
        assert_eq!(f.len(), 1, "overlapping edits are rework");

        // Non-overlapping ranges (10-15, 90-95) must NOT fire (zero-fire).
        let calm = raw.replace(
            r#""oldStart":12,"oldLines":6"#,
            r#""oldStart":90,"oldLines":5"#,
        );
        assert!(
            rework(&ingest_str(&calm, Lane::Main)).is_empty(),
            "distant edits are not rework"
        );
    }

    #[test]
    fn blind_write_attempt_detected_from_error() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":true,"content":"File has not been read yet. Read it first."}]}}"#,
        );
        let f = blind_write_attempts(&ingest_str(raw, Lane::Main));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::BlindWriteAttempt);
        assert_eq!(f[0].file.as_deref(), Some("/a.ts"));
    }

    #[test]
    fn calm_session_produces_no_findings() {
        // one read, one edit of different files — nothing struggles.
        let raw = format!(
            "{}\n{}",
            read("1", "2026-01-01T00:00:01Z", "/a.ts"),
            edit("2", "2026-01-01T00:00:02Z", "/b.ts"),
        );
        assert!(edit_shape(&ingest_str(&raw, Lane::Main)).is_empty());
    }
}
