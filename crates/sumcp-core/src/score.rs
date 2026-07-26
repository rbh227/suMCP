//! Ranking: a stated rule, not a score (spec 2026-07-26 §2b).
//!
//! Files are ordered by four keys, in this order:
//!
//! 1. edited files before never-edited ones, because a file with no change
//!    has nothing to review;
//! 2. [`crate::file_class::FileClass::tier`], because documentation and
//!    config churn does not predict recurrence;
//! 3. edit count, descending;
//! 4. path, so the order is total and stable.
//!
//! **Why there is no weighted score.** Until 2026-07-26 this module summed
//! `weight[category] * magnitude * confidence_factor` per file. Fitting those
//! weights to maximize hits WITH THE OUTCOMES IN HAND bought at most 4 hits
//! out of 39 on the only corpus it has been measured against, and the fit
//! assigned maximum weight to edit count regardless. A tuned sum that cannot
//! beat counting edits even while cheating is not worth the opacity it costs,
//! so the sum, the `Weights` type, and its TOML override (ADR A6) are gone.
//! See `docs/validation/2026-07-22-predictive-validity.md` and
//! `docs/validation/2026-07-26-file-class-measurement.md`.
//!
//! The findings stay. They are the explanation and the citations, and every
//! one still carries its tier, its exact-versus-heuristic flag, its
//! confidence, and the action indices that prove it.

/// The ranking rule, as one sentence. A constant so the payload, the HTML
/// report, and the terminal output cannot drift apart.
pub const RANKING_RULE: &str =
    "edited files first, then code before docs and config, then by edit count, ties by path";

use crate::model::{ActionKind, Finding, FindingKind, Idx, Session};
use crate::signals;
use serde::Serialize;
use std::collections::BTreeMap;

/// One file's place in the ranking, with the evidence that explains it.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// The file path.
    pub file: String,
    /// What kind of file this is. First ranking key after edited-ness.
    pub class: crate::file_class::FileClass,
    /// How many Edit or Write actions targeted this file. Second ranking key.
    pub edits: u64,
    /// Per-category magnitudes (churn/rework/failure_loops/re_read/fumbles/action_loops).
    pub breakdown: BTreeMap<String, u64>,
    /// The findings backing this file, in a stable order.
    pub findings: Vec<Finding>,
}

/// Every finding from every signal group, in a deterministic order.
pub fn all_findings(s: &Session) -> Vec<Finding> {
    let mut f = Vec::new();
    f.extend(signals::edit_shape(s));
    f.extend(signals::failures(s));
    f.extend(signals::dynamics(s));
    f.extend(signals::comprehension(s));
    f
}

/// Map a finding to its ranking `(category, magnitude)`, or `None` if it is
/// informational (opening move, reverts, comprehension) and doesn't rank.
/// `pub(crate)`: `review.rs`'s `severity_order_covers_every_ranked_category`
/// test cross-checks it against `SEVERITY_ORDER` so the two lists can't drift.
pub(crate) fn ranked_category(f: &Finding) -> Option<(&'static str, u64)> {
    match f.kind {
        FindingKind::Churn => Some(("churn", f.idxs.len() as u64)),
        FindingKind::Rework => Some(("rework", 1)),
        FindingKind::FailureLoop => Some(("failure_loops", f.idxs.len() as u64)),
        FindingKind::ReRead => Some(("re_read", f.idxs.len() as u64)),
        FindingKind::BlindWriteAttempt => Some(("fumbles", 1)),
        FindingKind::ActionLoop => Some(("action_loops", 1)),
        _ => None,
    }
}

/// Edit/Write actions per file. Not a signal: the ranking's second key and a
/// displayed number. Parallels `Overview::edits` in COUNTING ATTEMPTS rather
/// than confirmed successes, but is not the same number: `Overview::edits`
/// counts only `ActionKind::Edit` and tracks `writes` separately, while this
/// sums Edit and Write together.
fn edit_counts(s: &Session) -> BTreeMap<&str, u64> {
    let mut out: BTreeMap<&str, u64> = BTreeMap::new();
    for a in &s.actions {
        if matches!(a.kind, ActionKind::Edit | ActionKind::Write)
            && let Some(f) = a.file_path.as_deref()
        {
            *out.entry(f).or_insert(0) += 1;
        }
    }
    out
}

/// Rank the files by struggle. Four declared keys (module doc); file path
/// breaks ties (deterministic). Only files with at least one ranking finding
/// appear.
pub fn rank(s: &Session) -> Vec<FileScore> {
    // Per-file accumulator: per-category magnitudes, findings.
    type Acc = (BTreeMap<String, u64>, Vec<Finding>);
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let edits = edit_counts(s);

    for f in all_findings(s) {
        let Some(file) = f.file.clone() else { continue };
        let Some((category, magnitude)) = ranked_category(&f) else {
            continue;
        };

        let entry = acc.entry(file).or_insert((BTreeMap::new(), Vec::new()));
        *entry.0.entry(category.to_string()).or_insert(0) += magnitude;
        entry.1.push(f);
    }

    let mut scores: Vec<FileScore> = acc
        .into_iter()
        .map(|(file, (breakdown, findings))| {
            let edits = edits.get(file.as_str()).copied().unwrap_or(0);
            FileScore {
                class: crate::file_class::classify(&file),
                edits,
                file,
                breakdown,
                findings,
            }
        })
        .collect();
    // Four keys, in the order RANKING_RULE and the module doc declare. Class
    // outranks edit count because documentation/config churn does not
    // predict recurrence (file_class's module doc; studies cited there and above).
    scores.sort_by(|a, b| {
        (a.edits == 0)
            .cmp(&(b.edits == 0))
            .then_with(|| a.class.tier().cmp(&b.class.tier()))
            .then_with(|| b.edits.cmp(&a.edits))
            .then_with(|| a.file.cmp(&b.file))
    });
    scores
}

/// Collect the action indices behind a set of findings (for `evidence()`).
pub fn finding_idxs(findings: &[Finding]) -> Vec<Idx> {
    let mut idxs: Vec<Idx> = findings.iter().flat_map(|f| f.idxs.clone()).collect();
    idxs.sort();
    idxs.dedup();
    idxs
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

    #[test]
    fn ranking_is_transparent_and_ordered() {
        // /hot.ts edited 4x (churn 4), /warm.ts edited 2x (churn 2).
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(
                &format!("h{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/hot.ts",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("w{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/warm.ts",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].file, "/hot.ts", "more churn ranks first");
        assert_eq!(ranked[0].breakdown.get("churn"), Some(&4));
        assert!(ranked[0].edits > ranked[1].edits);
    }

    #[test]
    fn breakdown_key_is_re_read_not_thrash() {
        let read = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/a.ts"}}}}]}}}}"#
            )
        };
        let mut lines: Vec<String> = (0..3)
            .map(|i| read(&format!("r{i}"), &format!("2026-01-01T00:00:0{i}Z")))
            .collect();
        // two edits so the file also churns (any ranked file works)
        for i in 0..2 {
            lines.push(edit(
                &format!("e{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a.ts",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        assert!(ranked[0].breakdown.contains_key("re_read"));
        assert!(!ranked[0].breakdown.contains_key("thrash"));
    }

    /// Read-only files carry ReRead findings and so enter the ranking with
    /// zero edits. On the demo fixture a never-edited `.jpg` ranked FOURTH,
    /// above a `.py` file whose commands were failing, purely for having been
    /// read four times. A review queue is about changes, so anything unedited
    /// sorts last regardless of class.
    #[test]
    fn edited_files_outrank_unedited_ones() {
        let read = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
            )
        };
        let mut lines: Vec<String> = (0..4)
            .map(|i| {
                read(
                    &format!("r{i}"),
                    &format!("2026-01-01T00:00:0{i}Z"),
                    "/a/hero.jpg",
                )
            })
            .collect();
        // One edited code file, fewer signals than the read-thrashed image.
        for i in 0..2 {
            lines.push(edit(
                &format!("e{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        assert_eq!(ranked[0].file, "/a/main.rs", "edited file first");
        assert_eq!(ranked[0].edits, 2);
        assert_eq!(ranked.last().unwrap().file, "/a/hero.jpg");
        assert_eq!(ranked.last().unwrap().edits, 0, "never edited");
    }

    #[test]
    fn code_outranks_docs_even_with_fewer_edits() {
        let mut lines = Vec::new();
        // Docs edited 5x, code edited 2x. Code still wins on class.
        for i in 0..5 {
            lines.push(edit(
                &format!("d{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/a/NOTES-FOR-RELEASE.md",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("c{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        assert_eq!(ranked[0].file, "/a/main.rs");
        assert_eq!(ranked[0].class, crate::file_class::FileClass::Code);
        assert_eq!(ranked[1].file, "/a/NOTES-FOR-RELEASE.md");
        assert_eq!(ranked[1].class, crate::file_class::FileClass::Docs);
    }

    #[test]
    fn within_a_class_more_edits_ranks_first_and_path_breaks_ties() {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(
                &format!("h{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/a/hot.rs",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("m{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/mid.rs",
            ));
        }
        // Same edit count as mid.rs, so only the path can separate them.
        for i in 0..2 {
            lines.push(edit(
                &format!("z{i}"),
                &format!("2026-01-01T00:02:0{i}Z"),
                "/a/also.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        let files: Vec<&str> = ranked.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(files, vec!["/a/hot.rs", "/a/also.rs", "/a/mid.rs"]);
    }

    #[test]
    fn edits_counts_writes_as_well_as_edits() {
        let write = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"{file}","content":"x"}}}}]}}}}"#
            )
        };
        let raw = format!(
            "{}\n{}\n{}",
            write("w1", "2026-01-01T00:00:01Z", "/a/main.rs"),
            edit("e1", "2026-01-01T00:00:02Z", "/a/main.rs"),
            edit("e2", "2026-01-01T00:00:03Z", "/a/main.rs"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let ranked = rank(&s);
        assert_eq!(ranked[0].edits, 3, "Write counts toward edits");
    }

    /// A file that was never edited but carries an advisory ActionLoop
    /// finding still enters the ranking (`ranked_category` scores
    /// `ActionLoop`). This is the coverage the deleted
    /// `action_loop_contributes_at_half_weight` test used to carry: not the
    /// removed half-weight arithmetic, but the fact of entry itself.
    #[test]
    fn action_loop_only_file_still_enters_the_ranking() {
        let read = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/a/loop.rs"}}}}]}}}}"#
            )
        };
        let lines: Vec<String> = (0..3)
            .map(|i| read(&format!("r{i}"), &format!("2026-01-01T00:00:0{i}Z")))
            .collect();
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s);
        assert_eq!(
            ranked.len(),
            1,
            "the action-loop-only file still enters the ranking"
        );
        assert_eq!(ranked[0].file, "/a/loop.rs");
        assert_eq!(ranked[0].edits, 0, "never edited");
        assert!(ranked[0].breakdown.contains_key("action_loops"));
    }
}
