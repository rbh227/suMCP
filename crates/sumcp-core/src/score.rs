//! Transparent weighted ranking (T3.5, SPEC decision 6).
//!
//! Rank = Σ (`weight[category]` × magnitude × confidence-factor). Nothing is an
//! opaque score: every ranked file exposes its per-category breakdown and the
//! weights used. Low-confidence findings count ×`low_confidence_factor`.
//!
//! **The default weights are editorial by construction** (metrics-spec "Why
//! these default weights", 2026-07-18): no study provides per-file struggle-
//! category weights, and the published ρ values are session-level correlations
//! from different papers — not commensurable numbers. Only the rank ORDER is
//! research-derived; the decimals are tuning knobs. Payloads echo the weights
//! used and must never present them as literature-derived.
//!
//! `Weights` derives `Deserialize` (so a binary can load a TOML override) but
//! this module stays pure — file I/O lives in the binaries, keeping
//! `sumcp-core` serde-only (ADR A2).

use crate::model::{Confidence, Finding, FindingKind, Idx, Session};
use crate::signals;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Ranking weights. Defaults are documented; `~/.config/sumcp/config.toml`
/// may override them (loaded by the binaries, not here).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Weights {
    /// Weight per churned edit. Lowest scored category: churn fires
    /// constantly and is mostly benign iteration; `relative_churn` refines it
    /// when a denominator is known (Nagappan & Ball: size-normalized churn
    /// predicts defects, absolute churn does not).
    pub churn: f64,
    /// Weight per rework event. Highest tier: re-patch/coherence-collapse is
    /// the dominant edit-quality failure theme (TRAJEVAL: 39.7% on
    /// SWE-bench Verified).
    pub rework: f64,
    /// Weight per attributed failing command. Mid tier: validation-linked and
    /// directly attributed, but below the two dominant failure themes.
    pub failure_loop: f64,
    /// Weight per re-read (renamed from `thrash`, 2026-07-18 — the TOML key
    /// renamed with it). Lower tier: our own corpus observation, no external
    /// validation.
    pub re_read: f64,
    /// Weight per blind-write attempt. Highest tier with rework: premature
    /// editing appears in 63% of failed runs (IDE-Bench).
    pub fumble: f64,
    /// Weight per detected action loop. Advisory: always emitted with Low
    /// confidence, so its effective weight is this × `low_confidence_factor`
    /// (SWE-agent abandoned loop detectors over false positives).
    pub action_loop: f64,
    /// Multiplier applied to low-confidence findings.
    pub low_confidence_factor: f64,
    /// Where these weights came from (`defaults` or a config path).
    pub source: String,
}

impl Default for Weights {
    fn default() -> Self {
        // Rank order is research-derived (see module doc); decimals are not.
        Weights {
            churn: 1.0,
            rework: 3.0,
            failure_loop: 2.0,
            re_read: 1.5,
            fumble: 3.0,
            action_loop: 1.0,
            low_confidence_factor: 0.5,
            source: "defaults".into(),
        }
    }
}

/// Clamp bounds for the relative-churn multiplier: a known-tiny relative
/// churn halves a churn finding's contribution at most, a huge one doubles it
/// at most. Not `Weights` fields — they are not evidence-bearing knobs.
const REL_CHURN_CLAMP: (f64, f64) = (0.5, 2.0);

/// One file's rank, with the breakdown that explains it.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// The file path.
    pub file: String,
    /// The weighted score.
    pub score: f64,
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

fn category_weight(w: &Weights, category: &str) -> f64 {
    match category {
        "churn" => w.churn,
        "rework" => w.rework,
        "failure_loops" => w.failure_loop,
        "re_read" => w.re_read,
        "fumbles" => w.fumble,
        "action_loops" => w.action_loop,
        _ => 0.0,
    }
}

/// Within-category refinement (metrics-spec #7): a churn finding with a known
/// `relative_churn` scales its contribution by the clamped ratio; without a
/// denominator the multiplier is 1.0 (raw count is the fallback).
fn finding_multiplier(f: &Finding) -> f64 {
    match f.nums.get("relative_churn") {
        Some(rel) if f.kind == FindingKind::Churn => {
            rel.clamp(REL_CHURN_CLAMP.0, REL_CHURN_CLAMP.1)
        }
        _ => 1.0,
    }
}

/// Rank the files by weighted struggle. Descending score; file path breaks ties
/// (deterministic). Only files with at least one ranking finding appear.
pub fn rank(s: &Session, w: &Weights) -> Vec<FileScore> {
    // Per-file accumulator: running score, per-category magnitudes, findings.
    type Acc = (f64, BTreeMap<String, u64>, Vec<Finding>);
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();

    for f in all_findings(s) {
        let Some(file) = f.file.clone() else { continue };
        let Some((category, magnitude)) = ranked_category(&f) else {
            continue;
        };
        let factor = if f.confidence == Confidence::Low {
            w.low_confidence_factor
        } else {
            1.0
        };
        let contribution =
            category_weight(w, category) * magnitude as f64 * factor * finding_multiplier(&f);

        let entry = acc
            .entry(file)
            .or_insert((0.0, BTreeMap::new(), Vec::new()));
        entry.0 += contribution;
        *entry.1.entry(category.to_string()).or_insert(0) += magnitude;
        entry.2.push(f);
    }

    let mut scores: Vec<FileScore> = acc
        .into_iter()
        .map(|(file, (score, breakdown, findings))| FileScore {
            file,
            score,
            breakdown,
            findings,
        })
        .collect();
    // Descending score; file name breaks ties so the order is total & stable.
    scores.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
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
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].file, "/hot.ts", "more churn ranks first");
        assert_eq!(ranked[0].breakdown.get("churn"), Some(&4));
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn default_weight_order_matches_evidence_strength() {
        // The ordinal rationale (metrics-spec "Why these default weights"):
        // rework = fumble > failure_loop > re_read > churn; loops advisory.
        let w = Weights::default();
        assert_eq!(w.low_confidence_factor, 0.5);
        assert_eq!(w.rework, w.fumble, "the two dominant failure themes tie");
        assert!(w.rework > w.failure_loop);
        assert!(w.failure_loop > w.re_read);
        assert!(w.re_read > w.churn);
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
        let ranked = rank(&s, &Weights::default());
        assert!(ranked[0].breakdown.contains_key("re_read"));
        assert!(!ranked[0].breakdown.contains_key("thrash"));
    }

    #[test]
    fn tiny_relative_churn_halves_the_churn_contribution() {
        // Same churn count on two files; /small.ts has relative_churn 0.05
        // (clamped to 0.5), /nofile.ts has no denominator (multiplier 1.0).
        let sized_read = |id: &str, ts: &str, file: &str, total: usize| {
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
        };
        let raw = format!(
            "{}\n{}\n{}\n{}\n{}",
            sized_read("r1", "2026-01-01T00:00:00Z", "/small.ts", 40),
            edit("s1", "2026-01-01T00:00:01Z", "/small.ts"), // 1-line edits
            edit("s2", "2026-01-01T00:00:02Z", "/small.ts"), // 2/40 = 0.05
            edit("n1", "2026-01-01T00:01:01Z", "/nofile.ts"),
            edit("n2", "2026-01-01T00:01:02Z", "/nofile.ts"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let ranked = rank(&s, &Weights::default());
        let score_of = |file: &str| {
            ranked
                .iter()
                .find(|r| r.file == file)
                .map(|r| r.score)
                .unwrap()
        };
        assert!(
            (score_of("/small.ts") - score_of("/nofile.ts") * 0.5).abs() < 1e-9,
            "clamp floor 0.5 applied to the known-tiny relative churn"
        );
    }

    #[test]
    fn action_loop_contributes_at_half_weight() {
        let grep = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Grep","input":{{"pattern":"x","path":"/a.ts"}}}}]}}}}"#
            )
        };
        // 3 identical greps carrying a file_path ⇒ one ActionLoop on /a.ts.
        // Note Grep's `path` is not `file_path`, so attach file via file_path:
        let grep_fp = |id: &str, ts: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Grep","input":{{"pattern":"x","file_path":"/a.ts"}}}}]}}}}"#
            )
        };
        let _ = grep; // keep the non-file variant documented above
        let raw = (0..3)
            .map(|i| grep_fp(&format!("g{i}"), &format!("2026-01-01T00:00:0{i}Z")))
            .collect::<Vec<_>>()
            .join("\n");
        let s = ingest_str(&raw, Lane::Main);
        let w = Weights::default();
        let ranked = rank(&s, &w);
        assert_eq!(ranked.len(), 1);
        assert!(
            (ranked[0].score - w.action_loop * w.low_confidence_factor).abs() < 1e-9,
            "Low confidence ⇒ ×low_confidence_factor"
        );
    }
}
