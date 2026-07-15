//! The session model: ordered actions with first-class ordering.
//!
//! Ordering contract (SPEC decision 2 + amendment 5): actions are sorted by
//! `(effective_timestamp, agent lane [main first], source line number)`.
//! `effective_timestamp` is the line's own timestamp or the last-seen one
//! carried forward — because ~20% of real lines have none. Source line number
//! is always present and monotonic, so the order is *total and deterministic*
//! even under missing or tied timestamps. `Idx` is the stable handle every
//! finding cites as evidence and every payload exposes for `evidence()`.

use serde::{Deserialize, Serialize};

/// Stable index of an action in the session's total order.
///
/// Monotonic within a session; findings cite `Idx` values as evidence and
/// the `evidence()` MCP tool dereferences them back to raw actions.
// `derive` auto-generates trait impls: Ord/PartialOrd give us `<`/sorting,
// Serialize/Deserialize give JSON conversion, Copy makes it cheap to pass by
// value (it's just a u32 under the hood).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)] // serializes as the bare number `102`, not `{"0":102}`
pub struct Idx(pub u32);

/// Which lane an action came from: the main agent or a named subagent.
///
/// The derived `Ord` orders variants by declaration order, so `Main` sorts
/// before every `Sub(..)` — exactly the "main first" tie-break the ordering
/// contract wants. `Sub`s then order by their id string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Lane {
    /// The primary session transcript.
    Main,
    /// A subagent transcript, identified by its agent id.
    Sub(String),
}

/// The kind of thing an action is — kept coarse for v0.1's overview.
///
/// `Other` keeps the original tool name so nothing is silently dropped when a
/// new tool appears (schema drift is expected, not exceptional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// A `Read` tool call.
    Read,
    /// An `Edit` tool call.
    Edit,
    /// A `Write` tool call.
    Write,
    /// A `Bash` tool call.
    Bash,
    /// Any other tool, preserving its reported name.
    Other(String),
}

impl ActionKind {
    /// Classify a raw tool name into an [`ActionKind`].
    pub fn from_tool(name: &str) -> Self {
        // `match` is exhaustive — the compiler checks every path returns a
        // value, and the final `other` arm binds the leftover name.
        match name {
            "Read" => ActionKind::Read,
            "Edit" => ActionKind::Edit,
            "Write" => ActionKind::Write,
            "Bash" => ActionKind::Bash,
            other => ActionKind::Other(other.to_string()),
        }
    }
}

/// One agent action (a single tool call) placed in the session's total order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Position in the session's total order.
    pub idx: Idx,
    /// Timestamp used for ordering (own, or carried forward if the line had none).
    pub effective_ts: String,
    /// Whether this line carried its own timestamp (vs an inherited one).
    pub ts_inherited: bool,
    /// Main or subagent lane.
    pub lane: Lane,
    /// Original 0-based line number in its source transcript (the total-order tiebreak).
    pub line_no: usize,
    /// What the action did.
    pub kind: ActionKind,
    /// The file the tool acted on, if any (`file_path` input).
    pub file_path: Option<String>,
    /// Whether the tool result was an error (`is_error: true`), if known.
    pub is_error: Option<bool>,
    /// Size in chars of a Write/Edit's new content, if known (large-write signal).
    pub write_len: Option<usize>,
    /// Error text from the tool result, if it errored (drives fumble detection).
    pub error: Option<String>,
    /// Edited line ranges `(start, end)` from `structuredPatch` (rework signal).
    pub hunks: Vec<(u32, u32)>,
    /// The Bash command string, if this is a Bash action (failure attribution).
    pub command: Option<String>,
}

/// Token accounting, summed once per `message.id` (dedup layer a).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Tokens {
    /// Uncached input tokens.
    pub input: u64,
    /// Generated output tokens.
    pub output: u64,
    /// Tokens served from cache.
    pub cache_read: u64,
    /// Tokens written to cache.
    pub cache_creation: u64,
}

impl Tokens {
    /// Cache-hit ratio: cache reads over all input-side tokens. `None` when
    /// there is no input-side traffic to divide by (avoids 0/0).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let denom = self.input + self.cache_read + self.cache_creation;
        // `?`-free guard: return None rather than divide by zero.
        if denom == 0 {
            None
        } else {
            Some(self.cache_read as f64 / denom as f64)
        }
    }
}

/// Field-reliability tier (metrics-spec parser rules): T1 stable, T2 needs
/// edge handling, T3 unstable. Every finding declares the tier of the data it
/// rests on, so a schema break is triaged by blast radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    /// Stable fields.
    #[serde(rename = "T1")]
    T1,
    /// Fields needing edge handling.
    #[serde(rename = "T2")]
    T2,
    /// Unstable/undocumented fields.
    #[serde(rename = "T3")]
    T3,
}

/// Confidence in a finding; low-confidence findings count ×0.5 in ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Directly evidenced.
    High,
    /// Reasonable inference.
    Medium,
    /// Weak attribution.
    Low,
}

/// The kind of finding — serializes to the exact strings in the payload enum
/// (`docs/payload-schema.md`), so Rust output matches the frozen contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Repeat edits to one file.
    Churn,
    /// A later edit overlapping an earlier edit's lines.
    Rework,
    /// A file re-read many times.
    Thrash,
    /// An edit attempted before the file was read (harness-blocked).
    BlindWriteAttempt,
    /// Repeated failing commands attributed to a file.
    FailureLoop,
}

/// One evidence-backed observation about the session.
///
/// Every finding carries the action `idxs` proving it — the honesty invariant.
/// `note` explains heuristics; `file` scopes file-level findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// What was observed.
    pub kind: FindingKind,
    /// Reliability tier of the underlying fields.
    pub tier: Tier,
    /// True = deterministic count; false = heuristic (requires a `note`).
    pub exact: bool,
    /// Confidence in the finding.
    pub confidence: Confidence,
    /// Action indices proving it (dereferenceable via `evidence()`).
    pub idxs: Vec<Idx>,
    /// The file this finding is about, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Human-readable explanation (required when `exact` is false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A fully parsed session: ordered actions plus parse-health counters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// All actions, in the total order (so `actions[i].idx == Idx(i)`).
    pub actions: Vec<Action>,
    /// Token totals (deduped per `message.id`).
    pub tokens: Tokens,
    /// Histogram of every event `type` seen (including ones we don't model).
    pub type_counts: std::collections::BTreeMap<String, u64>,
    /// Lines that were not valid JSON (counted, never fatal).
    pub parse_errors: u64,
    /// How many lines carried no timestamp (amendment 5 visibility).
    pub untimestamped_lines: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idx_is_ordered_and_serializes_transparently() {
        assert!(Idx(2) > Idx(1));
        assert_eq!(serde_json::to_string(&Idx(102)).unwrap(), "102");
        assert_eq!(serde_json::from_str::<Idx>("102").unwrap(), Idx(102));
    }

    #[test]
    fn main_lane_sorts_before_subagents() {
        let mut lanes = vec![Lane::Sub("b".into()), Lane::Main, Lane::Sub("a".into())];
        lanes.sort();
        assert_eq!(
            lanes,
            vec![Lane::Main, Lane::Sub("a".into()), Lane::Sub("b".into())]
        );
    }

    #[test]
    fn cache_hit_ratio_guards_against_zero() {
        assert_eq!(Tokens::default().cache_hit_ratio(), None);
        let t = Tokens {
            input: 10,
            cache_read: 90,
            ..Default::default()
        };
        assert_eq!(t.cache_hit_ratio(), Some(0.9));
    }
}
