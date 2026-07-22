//! Needs-review qualification and plain-language rendering (report redesign,
//! spec 2026-07-22). The evidence floor is countable and explainable in one
//! sentence — deliberately NOT a tuned score threshold (SPEC §7: never a
//! single opaque number). The vocabulary is fixed and strictly descriptive:
//! the tool never editorializes; counts always attach.

use crate::model::{Finding, FindingKind};
use crate::score::FileScore;

/// Categories in the order reasons list them: most alarming first. Fixed, so
/// output is deterministic and independent of BTreeMap's alphabetical order.
pub const SEVERITY_ORDER: [&str; 6] = [
    "failure_loops",
    "fumbles",
    "rework",
    "churn",
    "re_read",
    "action_loops",
];

/// The fixed descriptive vocabulary. Counts always attach; no adjectives.
pub fn category_phrase(category: &str, n: u64) -> String {
    let s = if n == 1 { "" } else { "s" };
    match category {
        "churn" => format!("rewritten {n}x"),
        "rework" => format!("reworked {n}x"),
        "re_read" => format!("re-read {n}x"),
        "failure_loops" => format!("{n} failure loop{s}"),
        "fumbles" => format!("{n} blind-write attempt{s}"),
        "action_loops" => format!("{n} repeated-call loop{s}"),
        other => format!("{other} {n}"),
    }
}

/// Does `all` contain a non-ranking high-signal finding (flip or
/// user-correction) for this file? Those never enter `FileScore.findings`
/// (they don't rank), so qualification has to look at the full finding list.
fn has_flip_or_correction(all: &[Finding], file: &str) -> bool {
    all.iter().any(|f| {
        f.file.as_deref() == Some(file)
            && matches!(f.kind, FindingKind::Flip | FindingKind::UserCorrected)
    })
}

/// The evidence floor (grill decision, 2026-07-22): a file needs review when
/// it has 2+ findings, or a single high-signal one (failure loop, blind-write
/// attempt, flip, user-correction). Cap 3. Order follows `ranked`.
pub fn needs_review<'a>(ranked: &'a [FileScore], all: &[Finding]) -> Vec<&'a FileScore> {
    ranked
        .iter()
        .filter(|fs| {
            let ranked_high = fs.findings.iter().any(|f| {
                matches!(
                    f.kind,
                    FindingKind::FailureLoop | FindingKind::BlindWriteAttempt
                )
            });
            fs.findings.len() >= 2 || ranked_high || has_flip_or_correction(all, &fs.file)
        })
        .take(3)
        .collect()
}

/// One strictly descriptive sentence for a file: category phrases in severity
/// order, then flip/user-correction markers when present.
pub fn reason_sentence(fs: &FileScore, all: &[Finding]) -> String {
    let mut parts: Vec<String> = SEVERITY_ORDER
        .iter()
        .filter_map(|cat| fs.breakdown.get(*cat).map(|n| category_phrase(cat, *n)))
        .collect();
    if all
        .iter()
        .any(|f| f.file.as_deref() == Some(fs.file.as_str()) && f.kind == FindingKind::Flip)
    {
        parts.push("flipped after pushback".into());
    }
    if all.iter().any(|f| {
        f.file.as_deref() == Some(fs.file.as_str()) && f.kind == FindingKind::UserCorrected
    }) {
        parts.push("user-corrected".into());
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Confidence, Finding, FindingKind, Idx, Tier};
    use crate::score::FileScore;
    use std::collections::BTreeMap;

    fn finding(kind: FindingKind, file: &str) -> Finding {
        Finding {
            kind,
            tier: Tier::T1,
            exact: true,
            confidence: Confidence::High,
            idxs: vec![Idx(0)],
            file: Some(file.into()),
            note: None,
            nums: BTreeMap::new(),
        }
    }

    fn fs(file: &str, findings: Vec<Finding>, breakdown: &[(&str, u64)]) -> FileScore {
        FileScore {
            file: file.into(),
            score: 1.0,
            breakdown: breakdown.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            findings,
        }
    }

    #[test]
    fn two_findings_meet_the_floor_one_does_not() {
        let yes = fs(
            "/a.rs",
            vec![
                finding(FindingKind::Churn, "/a.rs"),
                finding(FindingKind::ReRead, "/a.rs"),
            ],
            &[("churn", 4), ("re_read", 2)],
        );
        let no = fs(
            "/b.rs",
            vec![finding(FindingKind::Churn, "/b.rs")],
            &[("churn", 2)],
        );
        let ranked = vec![yes, no];
        let picked = needs_review(&ranked, &[]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].file, "/a.rs");
    }

    #[test]
    fn one_high_signal_finding_qualifies_alone() {
        let loop_file = fs(
            "/c.rs",
            vec![finding(FindingKind::FailureLoop, "/c.rs")],
            &[("failure_loops", 3)],
        );
        let ranked = vec![loop_file];
        assert_eq!(needs_review(&ranked, &[]).len(), 1);
    }

    #[test]
    fn nonranking_flip_qualifies_via_all_findings() {
        // Flip findings never enter FileScore.findings (they don't rank), so
        // qualification must see them through `all`.
        let churn_only = fs(
            "/d.rs",
            vec![finding(FindingKind::Churn, "/d.rs")],
            &[("churn", 2)],
        );
        let all = vec![finding(FindingKind::Flip, "/d.rs")];
        let ranked = vec![churn_only];
        assert_eq!(needs_review(&ranked, &all).len(), 1);
    }

    #[test]
    fn cap_is_three() {
        let mk = |i: usize| {
            fs(
                &format!("/f{i}.rs"),
                vec![
                    finding(FindingKind::Churn, &format!("/f{i}.rs")),
                    finding(FindingKind::ReRead, &format!("/f{i}.rs")),
                ],
                &[("churn", 3), ("re_read", 2)],
            )
        };
        let ranked: Vec<FileScore> = (0..5).map(mk).collect();
        assert_eq!(needs_review(&ranked, &[]).len(), 3);
    }

    #[test]
    fn reason_sentence_uses_fixed_vocabulary_in_severity_order() {
        let f = fs(
            "/a.rs",
            vec![],
            &[("churn", 8), ("re_read", 4), ("failure_loops", 1)],
        );
        assert_eq!(
            reason_sentence(&f, &[]),
            "1 failure loop, rewritten 8x, re-read 4x"
        );
    }

    #[test]
    fn reason_sentence_appends_flip_and_user_corrected() {
        let f = fs("/a.rs", vec![], &[("churn", 2)]);
        let all = vec![
            finding(FindingKind::Flip, "/a.rs"),
            finding(FindingKind::UserCorrected, "/a.rs"),
        ];
        assert_eq!(
            reason_sentence(&f, &all),
            "rewritten 2x, flipped after pushback, user-corrected"
        );
    }

    #[test]
    fn category_phrase_pluralizes() {
        assert_eq!(category_phrase("failure_loops", 1), "1 failure loop");
        assert_eq!(category_phrase("failure_loops", 2), "2 failure loops");
        assert_eq!(category_phrase("fumbles", 1), "1 blind-write attempt");
        assert_eq!(category_phrase("churn", 3), "rewritten 3x");
        // unknown categories fall back to raw "name n" rather than panicking
        assert_eq!(category_phrase("mystery", 2), "mystery 2");
    }
}
