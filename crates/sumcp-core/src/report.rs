//! Overview report: the counts behind `session_overview` and the bare CLI.
//!
//! v0.1 slice — struggle findings arrive in later tasks. This computes the
//! deterministic totals from a [`Session`] and shapes them for display.

use crate::model::{ActionKind, Session};
use serde::Serialize;
use std::collections::BTreeSet;

/// The overview counts for one session.
#[derive(Debug, Serialize)]
pub struct Overview {
    /// Total actions (tool calls) after dedup.
    pub actions: usize,
    /// Distinct files touched by Read/Edit/Write.
    pub files_touched: usize,
    /// Edit count.
    pub edits: usize,
    /// Write count.
    pub writes: usize,
    /// Read count.
    pub reads: usize,
    /// Bash count.
    pub bash: usize,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_tokens: u64,
    /// Cache-hit ratio, if computable.
    pub cache_hit_ratio: Option<f64>,
    /// First → last effective timestamp (ISO strings), if any actions exist.
    pub span: Option<(String, String)>,
    /// Event-type histogram.
    pub type_counts: std::collections::BTreeMap<String, u64>,
    /// Lines that failed to parse (never fatal).
    pub parse_errors: u64,
    /// Lines with no timestamp (ordering used carry-forward).
    pub untimestamped_lines: u64,
}

impl Overview {
    /// Compute the overview from a parsed session.
    pub fn from_session(s: &Session) -> Self {
        // Count by kind in one pass. `iter().filter(...).count()` is the
        // idiomatic Rust way to count matching elements.
        let count = |k: &ActionKind| s.actions.iter().filter(|a| &a.kind == k).count();

        // Distinct files: collect into a set. `flatten()` drops the `None`s
        // from `Option<&String>`, so only real paths land in the set.
        let files: BTreeSet<&String> = s
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a.kind,
                    ActionKind::Read | ActionKind::Edit | ActionKind::Write
                )
            })
            .filter_map(|a| a.file_path.as_ref())
            .collect();

        // Actions are already in total order, so first/last give the span.
        let span = match (s.actions.first(), s.actions.last()) {
            (Some(f), Some(l)) => Some((f.effective_ts.clone(), l.effective_ts.clone())),
            _ => None,
        };

        Overview {
            actions: s.actions.len(),
            files_touched: files.len(),
            edits: count(&ActionKind::Edit),
            writes: count(&ActionKind::Write),
            reads: count(&ActionKind::Read),
            bash: count(&ActionKind::Bash),
            output_tokens: s.tokens.output,
            cache_read_tokens: s.tokens.cache_read,
            cache_hit_ratio: s.tokens.cache_hit_ratio(),
            span,
            type_counts: s.type_counts.clone(),
            parse_errors: s.parse_errors,
            untimestamped_lines: s.untimestamped_lines,
        }
    }

    /// Render a human-readable overview (the bare-`sumcp` view).
    pub fn to_text(&self) -> String {
        let ratio = self
            .cache_hit_ratio
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "n/a".into());
        let mut out = String::new();
        out.push_str("── session overview ──\n");
        out.push_str(&format!(
            "actions {}  |  files {}  |  edits {}  writes {}  reads {}  bash {}\n",
            self.actions, self.files_touched, self.edits, self.writes, self.reads, self.bash
        ));
        out.push_str(&format!(
            "tokens: output {}  cache-read {}  (cache hit {})\n",
            self.output_tokens, self.cache_read_tokens, ratio
        ));
        if let Some((a, b)) = &self.span {
            out.push_str(&format!("span: {a} → {b}\n"));
        }
        if self.parse_errors > 0 || self.untimestamped_lines > 0 {
            out.push_str(&format!(
                "parse: {} bad lines, {} untimestamped\n",
                self.parse_errors, self.untimestamped_lines
            ));
        }
        out
    }
}

/// Gaps between consecutive actions longer than this are counted at the cap
/// when summing "active" time (a session left open over lunch is not 3h of
/// work). A documented constant, not a Weights field: it shapes display,
/// never ranking, so it must not appear in the weights payload echo.
pub const ACTIVE_GAP_CAP_SECS: i64 = 300;

/// Parse an ISO-8601 timestamp ("2026-01-01T10:00:00Z", fractional seconds
/// and numeric offsets tolerated) into Unix seconds. Dependency-free by
/// design (core's budget is serde-only): date math is Howard Hinnant's
/// days-from-civil algorithm. Returns `None` on anything malformed — callers
/// treat unparseable time as absent, never as an error.
pub fn ts_secs(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> { ts.get(r)?.parse().ok() };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let (y2, mo2) = if mo <= 2 { (y - 1, mo + 12) } else { (y, mo) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * (mo2 - 3) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let mut secs = days * 86_400 + h * 3_600 + mi * 60 + sec;
    // After seconds: optional ".fff", then "Z" or "+HH:MM"/"-HH:MM".
    let rest = &ts[19..];
    let off = rest.trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign @ ('+' | '-')) = off.chars().next() {
        let oh: i64 = off.get(1..3)?.parse().ok()?;
        let om: i64 = off.get(4..6)?.parse().ok()?;
        let delta = oh * 3_600 + om * 60;
        secs += if sign == '+' { -delta } else { delta };
    }
    Some(secs)
}

/// Active vs wall-clock time for a session.
pub struct ActiveSpan {
    /// Sum of inter-action gaps, each capped at the given cap.
    pub active_secs: i64,
    /// Last minus first action timestamp.
    pub span_secs: i64,
}

/// Compute active/span durations over the session's action timestamps.
/// `None` when no action has a parseable timestamp.
pub fn active_span(s: &Session, cap_secs: i64) -> Option<ActiveSpan> {
    let times: Vec<i64> = s
        .actions
        .iter()
        .filter_map(|a| ts_secs(&a.effective_ts))
        .collect();
    let (first, last) = (times.first()?, times.last()?);
    let span_secs = (last - first).max(0);
    let active_secs = times
        .windows(2)
        .map(|w| (w[1] - w[0]).clamp(0, cap_secs))
        .sum();
    Some(ActiveSpan {
        active_secs,
        span_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    #[test]
    fn overview_counts_kinds_and_distinct_files() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"3","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.actions, 3);
        assert_eq!(o.reads, 1);
        assert_eq!(o.edits, 1);
        assert_eq!(o.bash, 1);
        assert_eq!(o.files_touched, 1, "same file read+edited counts once");
        assert_eq!(o.span.unwrap().0, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn ts_secs_parses_iso_zulu_fractions_and_offsets() {
        assert_eq!(ts_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(ts_secs("1970-01-02T00:00:00Z"), Some(86_400));
        // fractional seconds are ignored, not fatal
        assert_eq!(ts_secs("1970-01-01T00:00:01.500Z"), Some(1));
        // +02:00 is two hours EARLIER in UTC
        assert_eq!(ts_secs("1970-01-01T02:00:00+02:00"), Some(0));
        // a leap-year day: 2024-03-01 is day 60 of 2024
        assert_eq!(ts_secs("2024-03-01T00:00:00Z"), Some(1_709_251_200));
        assert_eq!(ts_secs("garbage"), None);
        assert_eq!(ts_secs(""), None);
    }

    #[test]
    fn active_span_caps_idle_gaps() {
        // Three actions: 0s, 60s, then a 2-hour gap. Span = 7260s;
        // active = 60 + cap(300) = 360s.
        let mk = |ts: &str, id: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/a"}}}}]}}}}"#
            )
        };
        let raw = [
            mk("2026-01-01T10:00:00Z", "a"),
            mk("2026-01-01T10:01:00Z", "b"),
            mk("2026-01-01T12:01:00Z", "c"),
        ]
        .join("\n");
        let s = crate::ingest::ingest_str(&raw, crate::model::Lane::Main);
        let d = active_span(&s, ACTIVE_GAP_CAP_SECS).unwrap();
        assert_eq!(d.span_secs, 7_260);
        assert_eq!(d.active_secs, 360);
        // empty session -> None
        let empty = crate::ingest::ingest_str("", crate::model::Lane::Main);
        assert!(active_span(&empty, ACTIVE_GAP_CAP_SECS).is_none());
    }
}
