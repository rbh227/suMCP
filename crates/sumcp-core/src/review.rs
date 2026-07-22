//! Needs-review qualification and plain-language rendering (report redesign,
//! spec 2026-07-22). The evidence floor is countable and explainable in one
//! sentence — deliberately NOT a tuned score threshold (SPEC §7: never a
//! single opaque number). The vocabulary is fixed and strictly descriptive:
//! the tool never editorializes; counts always attach.

use crate::model::{Finding, FindingKind};
use crate::score::FileScore;
use std::collections::BTreeMap;

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
        "failure_loops" => format!("{n} looped failing command{s}"),
        "fumbles" => format!("{n} blind-write attempt{s}"),
        "action_loops" => format!("{n} repeated-call loop{s}"),
        other => format!("{other} {n}"),
    }
}

/// One file that needs human eyes: every file-scoped finding about it, plus
/// its ranked entry when the file also ranked. `ranked` is `None` when the
/// file's only findings are non-ranking kinds (flip, user-correction,
/// true-revert, large-write-instant-accept) that never enter `score::rank`.
pub struct ReviewCandidate<'a> {
    /// The file path.
    pub file: String,
    /// The file's ranked entry, if `score::rank` ranked it.
    pub ranked: Option<&'a FileScore>,
    /// Every file-scoped finding about this file (OpeningMove excluded: it's
    /// per-segment information, not a review signal).
    pub findings: Vec<&'a Finding>,
}

/// A single non-ranking finding kind that qualifies a file alone.
fn is_solo_qualifying(kind: &FindingKind) -> bool {
    matches!(
        kind,
        FindingKind::FailureLoop
            | FindingKind::BlindWriteAttempt
            | FindingKind::Flip
            | FindingKind::UserCorrected
    )
}

/// The evidence floor (grill decision, 2026-07-22): a file needs review when
/// it has 2+ collected findings, or a single high-signal one (failure loop,
/// blind-write attempt, flip, user-correction). Candidates are built from
/// ALL file-scoped findings in `all` (not just the ones that made it into
/// `score::rank`'s output), so files whose only findings are non-ranking
/// kinds (flip, user-correction) can still qualify — `score::rank` excludes
/// those kinds entirely, so looking only at `ranked` would silently drop
/// them. Order: qualifying files in `ranked` order first, then qualifying
/// unranked files by path (BTreeMap order). Cap 3.
pub fn needs_review<'a>(ranked: &'a [FileScore], all: &'a [Finding]) -> Vec<ReviewCandidate<'a>> {
    let mut by_file: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    for f in all {
        if f.kind == FindingKind::OpeningMove {
            continue; // per-segment information, not a review signal
        }
        if let Some(file) = f.file.as_deref() {
            by_file.entry(file).or_default().push(f);
        }
    }

    let qualifies = |findings: &[&Finding]| {
        findings.len() >= 2 || findings.iter().any(|f| is_solo_qualifying(&f.kind))
    };

    let mut out: Vec<ReviewCandidate<'a>> = Vec::new();
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();

    for fs in ranked {
        let file = fs.file.as_str();
        if let Some(findings) = by_file.get(file)
            && qualifies(findings)
        {
            out.push(ReviewCandidate {
                file: fs.file.clone(),
                ranked: Some(fs),
                findings: findings.clone(),
            });
            seen.insert(file);
        }
    }
    for (file, findings) in &by_file {
        if seen.contains(file) {
            continue;
        }
        if qualifies(findings) {
            out.push(ReviewCandidate {
                file: (*file).to_string(),
                ranked: None,
                findings: findings.clone(),
            });
        }
    }
    out.truncate(3);
    out
}

/// One strictly descriptive sentence for a candidate: when ranked, category
/// phrases in severity order first; then phrases for non-ranking kinds
/// present in its findings, each rendered once with its count.
pub fn reason_sentence(c: &ReviewCandidate) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(fs) = c.ranked {
        parts.extend(
            SEVERITY_ORDER
                .iter()
                .filter_map(|cat| fs.breakdown.get(*cat).map(|n| category_phrase(cat, *n))),
        );
    }
    let count_of =
        |kind: &FindingKind| c.findings.iter().filter(|f| &f.kind == kind).count() as u64;

    let flip_n = count_of(&FindingKind::Flip);
    if flip_n > 0 {
        parts.push(if flip_n > 1 {
            format!("flipped after pushback {flip_n}x")
        } else {
            "flipped after pushback".into()
        });
    }
    let uc_n = count_of(&FindingKind::UserCorrected);
    if uc_n > 0 {
        parts.push(if uc_n > 1 {
            format!("user-corrected {uc_n}x")
        } else {
            "user-corrected".into()
        });
    }
    let tr_n = count_of(&FindingKind::TrueRevert);
    if tr_n > 0 {
        parts.push(format!("self-reverted {tr_n}x"));
    }
    let lw_n = count_of(&FindingKind::LargeWriteInstantAccept);
    if lw_n > 0 {
        let s = if lw_n == 1 { "" } else { "s" };
        parts.push(format!("{lw_n} large write{s} accepted instantly"));
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

    fn fs(file: &str, breakdown: &[(&str, u64)]) -> FileScore {
        FileScore {
            file: file.into(),
            score: 1.0,
            breakdown: breakdown.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            findings: Vec::new(),
        }
    }

    #[test]
    fn two_findings_meet_the_floor_one_does_not() {
        let yes = fs("/a.rs", &[("churn", 4), ("re_read", 2)]);
        let no = fs("/b.rs", &[("churn", 2)]);
        let ranked = vec![yes, no];
        let all = vec![
            finding(FindingKind::Churn, "/a.rs"),
            finding(FindingKind::ReRead, "/a.rs"),
            finding(FindingKind::Churn, "/b.rs"),
        ];
        let picked = needs_review(&ranked, &all);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].file, "/a.rs");
    }

    #[test]
    fn one_high_signal_finding_qualifies_alone() {
        let loop_file = fs("/c.rs", &[("failure_loops", 3)]);
        let ranked = vec![loop_file];
        let all = vec![finding(FindingKind::FailureLoop, "/c.rs")];
        assert_eq!(needs_review(&ranked, &all).len(), 1);
    }

    #[test]
    fn nonranking_flip_qualifies_via_all_findings() {
        // Flip findings never enter FileScore.findings (they don't rank), so
        // qualification must see them through `all`.
        let churn_only = fs("/d.rs", &[("churn", 2)]);
        let all = vec![
            finding(FindingKind::Churn, "/d.rs"),
            finding(FindingKind::Flip, "/d.rs"),
        ];
        let ranked = vec![churn_only];
        assert_eq!(needs_review(&ranked, &all).len(), 1);
    }

    #[test]
    fn sole_user_corrected_file_qualifies() {
        // No ranked entry at all; a single UserCorrected finding is a
        // solo-qualifying kind, so the file must appear as an unranked
        // candidate.
        let all = vec![finding(FindingKind::UserCorrected, "/x.rs")];
        let picked = needs_review(&[], &all);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].file, "/x.rs");
        assert!(picked[0].ranked.is_none());
        assert_eq!(reason_sentence(&picked[0]), "user-corrected");
    }

    #[test]
    fn two_nonranking_findings_qualify() {
        let all = vec![
            finding(FindingKind::TrueRevert, "/y.rs"),
            finding(FindingKind::Flip, "/y.rs"),
        ];
        let picked = needs_review(&[], &all);
        assert_eq!(picked.len(), 1);
        let reason = reason_sentence(&picked[0]);
        assert!(reason.contains("flipped after pushback"), "{reason}");
        assert!(reason.contains("self-reverted 1x"), "{reason}");
    }

    #[test]
    fn cap_is_three() {
        let mk = |i: usize| fs(&format!("/f{i}.rs"), &[("churn", 3), ("re_read", 2)]);
        let ranked: Vec<FileScore> = (0..5).map(mk).collect();
        let all: Vec<Finding> = (0..5)
            .flat_map(|i| {
                let file = format!("/f{i}.rs");
                vec![
                    finding(FindingKind::Churn, &file),
                    finding(FindingKind::ReRead, &file),
                ]
            })
            .collect();
        assert_eq!(needs_review(&ranked, &all).len(), 3);
    }

    #[test]
    fn reason_sentence_uses_fixed_vocabulary_in_severity_order() {
        let f = fs(
            "/a.rs",
            &[("churn", 8), ("re_read", 4), ("failure_loops", 1)],
        );
        let all = [finding(FindingKind::FailureLoop, "/a.rs")];
        let c = ReviewCandidate {
            file: f.file.clone(),
            ranked: Some(&f),
            findings: all.iter().collect(),
        };
        assert_eq!(
            reason_sentence(&c),
            "1 looped failing command, rewritten 8x, re-read 4x"
        );
    }

    #[test]
    fn reason_sentence_appends_flip_and_user_corrected() {
        let f = fs("/a.rs", &[("churn", 2)]);
        let all = [
            finding(FindingKind::Flip, "/a.rs"),
            finding(FindingKind::UserCorrected, "/a.rs"),
        ];
        let c = ReviewCandidate {
            file: f.file.clone(),
            ranked: Some(&f),
            findings: all.iter().collect(),
        };
        assert_eq!(
            reason_sentence(&c),
            "rewritten 2x, flipped after pushback, user-corrected"
        );
    }

    #[test]
    fn severity_order_covers_every_ranked_category() {
        // Cheap hardening: if `score::ranked_category` ever grows a new
        // ranking category, this fails loudly instead of the category
        // silently vanishing from `reason_sentence` (it just isn't in
        // SEVERITY_ORDER's filter_map). Magnitude is irrelevant to
        // `ranked_category`, so 1 is fine everywhere.
        let all_kinds = [
            FindingKind::Churn,
            FindingKind::Rework,
            FindingKind::ReRead,
            FindingKind::BlindWriteAttempt,
            FindingKind::FailureLoop,
            FindingKind::TrueRevert,
            FindingKind::Flip,
            FindingKind::UserCorrected,
            FindingKind::OpeningMove,
            FindingKind::LargeWriteInstantAccept,
            FindingKind::ActionLoop,
            FindingKind::ReviewBurden,
        ];
        for kind in all_kinds {
            let f = finding(kind, "/a.rs");
            if let Some((category, _magnitude)) = crate::score::ranked_category(&f) {
                assert!(
                    SEVERITY_ORDER.contains(&category),
                    "ranked_category emits {category:?} but SEVERITY_ORDER doesn't list it"
                );
            }
        }
    }

    #[test]
    fn category_phrase_pluralizes() {
        assert_eq!(
            category_phrase("failure_loops", 1),
            "1 looped failing command"
        );
        assert_eq!(
            category_phrase("failure_loops", 2),
            "2 looped failing commands"
        );
        assert_eq!(category_phrase("fumbles", 1), "1 blind-write attempt");
        assert_eq!(category_phrase("churn", 3), "rewritten 3x");
        // unknown categories fall back to raw "name n" rather than panicking
        assert_eq!(category_phrase("mystery", 2), "mystery 2");
    }
}
