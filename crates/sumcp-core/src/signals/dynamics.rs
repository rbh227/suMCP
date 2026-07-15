//! Dynamics signals (T3.3): opening move, true reverts, capitulation flips,
//! user corrections. The reverts/flips fire rarely (SPEC §1 amendment 3) —
//! high-signal when they do — so they exist but never anchor the ranking.
//!
//! Deferred within T3.3 (lower value, larger surface): n-gram action-loop
//! repetition and standalone pushback-rate stats. Interruptions are already
//! counted in `Session::interrupts`.

use crate::model::{Action, ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};

/// Only the first N actions define the "opening move".
const OPENING_WINDOW: usize = 10;
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
    out
}

/// Classify the opening move: did the agent gather context (read) before its
/// first edit, or dive straight into editing? Read-first correlates with
/// success; patch-first is the #1 empirical failure mode.
fn opening_move(s: &Session) -> Option<Finding> {
    let window = &s.actions[..s.actions.len().min(OPENING_WINDOW)];
    let first_read = window
        .iter()
        .position(|a| matches!(a.kind, ActionKind::Read));
    let first_edit = window
        .iter()
        .position(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write));
    let first_edit = first_edit?; // no edit in the opening ⇒ no opening-move call

    let read_first = matches!(first_read, Some(r) if r < first_edit);
    let idxs: Vec<Idx> = window.iter().take(first_edit + 1).map(|a| a.idx).collect();
    Some(Finding {
        kind: FindingKind::OpeningMove,
        tier: Tier::T1,
        exact: true,
        confidence: Confidence::High,
        note: Some(
            if read_first {
                "read-first: gathered context before the first edit"
            } else {
                "patch-first: edited before reading (the #1 empirical failure mode)"
            }
            .into(),
        ),
        idxs,
        file: None,
    })
}

/// True reverts and flips: a later edit whose new content restores what an
/// earlier edit removed (`later.new == earlier.old`). If the user pushed back
/// in between, it's a flip (capitulation) rather than a plain revert.
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
                let pushed_back = user_pushback_between(s, earlier.line_no, later.line_no);
                out.push(Finding {
                    kind: if pushed_back {
                        FindingKind::Flip
                    } else {
                        FindingKind::TrueRevert
                    },
                    tier: Tier::T2,
                    exact: true,
                    confidence: Confidence::High,
                    note: Some(
                        if pushed_back {
                            "reverted right after user pushback (capitulation flip)"
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

/// Was there a user message with pushback wording between two line numbers?
fn user_pushback_between(s: &Session, lo: usize, hi: usize) -> bool {
    s.user_texts.iter().any(|u| {
        u.line_no > lo && u.line_no < hi && {
            let t = u.text.to_lowercase();
            PUSHBACK.iter().any(|p| t.contains(p))
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

    #[test]
    fn opening_move_detects_patch_first() {
        let raw = edit("1", "2026-01-01T00:00:01Z", "/a.ts", "x", "y");
        let f = opening_move(&ingest_str(&raw, Lane::Main)).unwrap();
        assert!(f.note.unwrap().contains("patch-first"));
    }

    #[test]
    fn opening_move_detects_read_first() {
        let raw = format!(
            "{}\n{}",
            read("1", "2026-01-01T00:00:01Z", "/a.ts"),
            edit("2", "2026-01-01T00:00:02Z", "/a.ts", "x", "y"),
        );
        let f = opening_move(&ingest_str(&raw, Lane::Main)).unwrap();
        assert!(f.note.unwrap().contains("read-first"));
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
