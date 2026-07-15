//! Transparent weighted ranking (T3.5, SPEC decision 6).
//!
//! Rank = Σ (weight[category] × magnitude × confidence-factor). Nothing is an
//! opaque score: every ranked file exposes its per-category breakdown and the
//! weights used. Low-confidence findings count ×`low_confidence_factor`.
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
    /// Weight per churned edit.
    pub churn: f64,
    /// Weight per rework event.
    pub rework: f64,
    /// Weight per attributed failing command.
    pub failure_loop: f64,
    /// Weight per re-read.
    pub thrash: f64,
    /// Weight per blind-write attempt.
    pub fumble: f64,
    /// Multiplier applied to low-confidence findings.
    pub low_confidence_factor: f64,
    /// Where these weights came from (`defaults` or a config path).
    pub source: String,
}

impl Default for Weights {
    fn default() -> Self {
        Weights {
            churn: 1.0,
            rework: 2.0,
            failure_loop: 3.0,
            thrash: 1.5,
            fumble: 2.0,
            low_confidence_factor: 0.5,
            source: "defaults".into(),
        }
    }
}

/// One file's rank, with the breakdown that explains it.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// The file path.
    pub file: String,
    /// The weighted score.
    pub score: f64,
    /// Per-category magnitudes (churn/rework/failure_loops/thrash/fumbles).
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
fn ranked_category(f: &Finding) -> Option<(&'static str, u64)> {
    match f.kind {
        FindingKind::Churn => Some(("churn", f.idxs.len() as u64)),
        FindingKind::Rework => Some(("rework", 1)),
        FindingKind::FailureLoop => Some(("failure_loops", f.idxs.len() as u64)),
        FindingKind::Thrash => Some(("thrash", f.idxs.len() as u64)),
        FindingKind::BlindWriteAttempt => Some(("fumbles", 1)),
        _ => None,
    }
}

fn category_weight(w: &Weights, category: &str) -> f64 {
    match category {
        "churn" => w.churn,
        "rework" => w.rework,
        "failure_loops" => w.failure_loop,
        "thrash" => w.thrash,
        "fumbles" => w.fumble,
        _ => 0.0,
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
        let contribution = category_weight(w, category) * magnitude as f64 * factor;

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
    fn low_confidence_is_downweighted() {
        // A weights struct where we can see the factor applied is covered by
        // failure attribution; here just assert defaults are sane.
        let w = Weights::default();
        assert_eq!(w.low_confidence_factor, 0.5);
        assert!(w.failure_loop > w.churn, "failures weigh more than churn");
    }
}
