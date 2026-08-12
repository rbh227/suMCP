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
    /// File-modifying operations that CONFIRMED success (`is_error ==
    /// Some(false)`). Reported because `edits` alone reads as "everything that
    /// changed a file" but omits Write, which undercounts by 20-50% on a
    /// typical session.
    ///
    /// Only confirmed-successful actions count: ingest records an action from
    /// the proposed tool call, before the result is known, so a failed Edit
    /// (old_string mismatch, rejected write) would otherwise be reported as
    /// work that changed a file when it changed nothing. Anything not confirmed
    /// successful is disclosed in [`Self::file_ops_unconfirmed`].
    pub file_ops: usize,
    /// Lines of NEW content across every CONFIRMED-successful Edit
    /// (`new_string`) and Write (`content`). A tool-call count says how often a
    /// tool fired, not how much changed — one Edit can rewrite hundreds of
    /// lines. Summed from `Action::write_lines`, which ingest counts on the
    /// full string before capping, so large writes stay accurate.
    ///
    /// Scope: lines *written*. Deletions are not counted (an Edit that removes
    /// 50 lines and adds 2 contributes 2), because `edit_old` is stored capped
    /// and would undercount exactly the largest edits.
    pub lines_written: usize,
    /// Edit/Write actions whose result did NOT confirm success: an explicit
    /// `is_error: true`, or no tool result at all (a truncated or mid-flight
    /// session). Excluded from `file_ops`/`lines_written` and surfaced here so
    /// the gap is visible rather than silently counted either way.
    pub file_ops_unconfirmed: usize,
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

        let edits = count(&ActionKind::Edit);
        let writes = count(&ActionKind::Write);

        // Headline totals count only file-modifying actions whose result
        // CONFIRMED success. `write_lines` is captured from the proposed input
        // before the result is known, so an errored or result-less action would
        // otherwise report lines that never reached the file. Every lane is
        // included, so subagent work counts.
        let modifying = s
            .actions
            .iter()
            .filter(|a| matches!(a.kind, ActionKind::Edit | ActionKind::Write));
        let (mut file_ops, mut lines_written, mut file_ops_unconfirmed) = (0, 0, 0);
        for a in modifying {
            if a.is_error == Some(false) {
                file_ops += 1;
                // `unwrap_or(0)` because an Edit whose input carried no
                // `new_string` (a malformed line) contributes no known volume
                // rather than breaking the total.
                lines_written += a.write_lines.unwrap_or(0);
            } else {
                // Some(true) = confirmed failure; None = no tool result, so the
                // outcome is unknown. Neither is a confirmed write.
                file_ops_unconfirmed += 1;
            }
        }

        Overview {
            actions: s.actions.len(),
            files_touched: files.len(),
            edits,
            writes,
            file_ops,
            lines_written,
            file_ops_unconfirmed,
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

    /// Render a human-readable overview (the bare-`backstory` view).
    pub fn to_text(&self) -> String {
        let ratio = self
            .cache_hit_ratio
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "n/a".into());
        let mut out = String::new();
        out.push_str("── session overview ──\n");
        // Lead with the two honest numbers: how many operations changed files,
        // and how much they changed. The edits/writes split stays on the second
        // line for anyone who wants the breakdown.
        out.push_str(&format!(
            "actions {}  |  files {}  |  file ops {}  lines written {}\n",
            self.actions, self.files_touched, self.file_ops, self.lines_written
        ));
        out.push_str(&format!(
            "  attempted: edits {}  writes {}  |  reads {}  bash {}\n",
            self.edits, self.writes, self.reads, self.bash
        ));
        // Only shown when nonzero: on a clean session this line is noise, but
        // when edits failed the headline is smaller than the attempt counts
        // above and that difference must be explained, not left to inference.
        if self.file_ops_unconfirmed > 0 {
            out.push_str(&format!(
                "  {} file op(s) did not confirm success (errored or no result)\n",
                self.file_ops_unconfirmed
            ));
        }
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
/// work). A documented constant, not a ranking input: it shapes display,
/// never the ranking order, so it plays no part in `score::rank`.
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

    // Validate month
    if !(1..=12).contains(&mo) {
        return None;
    }

    // Validate day (accounting for month and leap year)
    let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let days_in_month = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    if !(1..=days_in_month).contains(&d) {
        return None;
    }

    // Validate time components
    if !(0..=23).contains(&h) || !(0..=59).contains(&mi) || !(0..=59).contains(&sec) {
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
        // Validate offset format: must have ':' at position 3 and valid ranges
        if off.len() < 6 || off.as_bytes()[3] != b':' {
            return None;
        }
        let oh: i64 = off.get(1..3)?.parse().ok()?;
        let om: i64 = off.get(4..6)?.parse().ok()?;
        if !(0..=23).contains(&oh) || !(0..=59).contains(&om) {
            return None;
        }
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
///
/// Actions are ordered by ingest on the RAW TIMESTAMP STRING (a locked SPEC
/// contract; ingest itself is never reordered by parsed value). String order
/// is lexicographic, so it matches chronological (UTC) order only when every
/// timestamp shares the same numeric offset — a transcript that mixes "Z" and
/// "-02:00" lines can have string order invert UTC order. Sorting the parsed
/// seconds here (not relying on `s.actions`' string-sorted order) makes span
/// and active-time computation robust to that: whatever order the strings
/// happened to sort in, the durations reported are always the true elapsed
/// time between the earliest and latest real timestamps.
pub fn active_span(s: &Session, cap_secs: i64) -> Option<ActiveSpan> {
    let mut times: Vec<i64> = s
        .actions
        .iter()
        .filter_map(|a| ts_secs(&a.effective_ts))
        .collect();
    times.sort_unstable();
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
    fn file_ops_sums_edits_and_writes() {
        // WHY: `edits` alone reads as "all file modifications" but excludes
        // Write, undercounting file-modifying operations. `file_ops` is the
        // honest single number: every Edit plus every Write.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"/a.ts","new_string":"a\nb"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":false}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/b.ts","new_string":"c"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"2","is_error":false}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"3","name":"Write","input":{"file_path":"/c.ts","content":"x\ny\nz"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"3","is_error":false}]}}"#,
            "\n",
            // A Read must NOT count as a file-modifying operation.
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06Z","message":{"content":[{"type":"tool_use","id":"4","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:07Z","message":{"content":[{"type":"tool_result","tool_use_id":"4","is_error":false}]}}"#,
        );
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.edits, 2);
        assert_eq!(o.writes, 1);
        assert_eq!(o.file_ops, 3, "2 edits + 1 write, the Read excluded");
        assert_eq!(o.file_ops_unconfirmed, 0, "all three confirmed");
    }

    #[test]
    fn lines_written_sums_new_content_over_edits_and_writes() {
        // WHY: a tool-call count undershoots the volume of change — one Edit
        // can rewrite hundreds of lines. `lines_written` sums the NEW content
        // of every Edit (`new_string`) and Write (`content`), so the headline
        // reports scale, not just how many times a tool fired.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"/a.ts","new_string":"a\nb\nc"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":false}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"2","name":"Write","input":{"file_path":"/c.ts","content":"x\ny"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"2","is_error":false}]}}"#,
            "\n",
            // Reads carry no new content and must contribute nothing.
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"3","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"3","is_error":false}]}}"#,
        );
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.lines_written, 5, "3 edited lines + 2 written lines");
        assert_eq!(o.file_ops, 2);
    }

    #[test]
    fn lines_written_counts_subagent_work_too() {
        // WHY: the whole point of the merge is that subagent work lands in the
        // headline. A session whose edits happened almost entirely inside
        // subagents must not report a near-zero volume.
        use crate::merge::merge_sessions;
        let main = ingest_str(
            concat!(
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"/a.ts","new_string":"only\nline"}}]}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":false}]}}"#,
            ),
            Lane::Main,
        );
        let sub = ingest_str(
            concat!(
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"2","name":"Write","input":{"file_path":"/big.rs","content":"1\n2\n3\n4\n5\n6\n7\n8"}}]}}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"2","is_error":false}]}}"#,
            ),
            Lane::Sub("helper".into()),
        );
        let o = Overview::from_session(&merge_sessions(main, vec![sub], 0));
        assert_eq!(o.file_ops, 2, "main edit + subagent write");
        assert_eq!(o.lines_written, 10, "2 from main + 8 from the subagent");
    }

    #[test]
    fn failed_edits_are_excluded_from_written_totals() {
        // WHY: ingest records `write_lines` from the PROPOSED input, before the
        // tool result is known. An Edit that failed (old_string mismatch, or a
        // rejected write) changed no file, so counting its lines as "written"
        // overstates real work. Confirmed failures must land in the
        // `unconfirmed` bucket instead of the headline.
        let raw = concat!(
            // Succeeded: 3 lines, counts.
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"ok","name":"Edit","input":{"file_path":"/a.ts","new_string":"a\nb\nc"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"ok","is_error":false}]}}"#,
            "\n",
            // Failed: 99 lines proposed, none written.
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"bad","name":"Write","input":{"file_path":"/b.ts","content":"x\ny\nz"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"bad","is_error":true}]}}"#,
        );
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.file_ops, 1, "only the confirmed-successful Edit");
        assert_eq!(o.lines_written, 3, "the failed Write's 3 lines excluded");
        assert_eq!(o.file_ops_unconfirmed, 1, "the failed Write is disclosed");
        // The raw per-kind counts stay unfiltered: they describe what the agent
        // ATTEMPTED, which the failure signals still need.
        assert_eq!(o.edits, 1);
        assert_eq!(o.writes, 1);
    }

    #[test]
    fn edits_with_no_tool_result_are_unconfirmed_not_written() {
        // WHY: a truncated or mid-flight session leaves the last Edit with no
        // tool_result, so `is_error` is None — outcome unknown, not success.
        // Claiming those lines were "written" would be a guess; they belong in
        // `unconfirmed` so the gap is visible rather than silently optimistic.
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"dangling","name":"Edit","input":{"file_path":"/a.ts","new_string":"a\nb"}}]}}"#;
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.file_ops, 0, "unknown outcome is not a confirmed write");
        assert_eq!(o.lines_written, 0);
        assert_eq!(o.file_ops_unconfirmed, 1, "disclosed, not dropped");
    }

    #[test]
    fn text_view_reports_file_ops_and_lines_written() {
        // WHY: the regression that started this — the rendered headline said
        // "edits N" and silently excluded writes and all volume. Assert the
        // rendered string carries both the operation count and the volume.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"/a.ts","new_string":"a\nb\nc"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":false}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"2","name":"Write","input":{"file_path":"/c.ts","content":"x\ny"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"2","is_error":false}]}}"#,
        );
        let text = Overview::from_session(&ingest_str(raw, Lane::Main)).to_text();
        assert!(
            text.contains("file ops 2"),
            "operations must be one honest number, got:\n{text}"
        );
        assert!(
            text.contains("lines written 5"),
            "volume must appear alongside the op count, got:\n{text}"
        );
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
    fn ts_secs_rejects_malformed_components() {
        // Hour, minute, second out of range
        assert_eq!(ts_secs("1970-01-01T99:00:00Z"), None);
        assert_eq!(ts_secs("1970-01-01T00:99:00Z"), None);
        assert_eq!(ts_secs("1970-01-01T00:00:99Z"), None);
        // Day out of range for month (with leap year logic)
        assert_eq!(ts_secs("2025-02-30T00:00:00Z"), None);
        assert_eq!(ts_secs("2025-04-31T00:00:00Z"), None);
        assert!(ts_secs("2024-02-29T00:00:00Z").is_some()); // leap day valid
        assert_eq!(ts_secs("2023-02-29T00:00:00Z"), None); // not a leap year
        // Offset validation: bad separator and out-of-range
        assert_eq!(ts_secs("1970-01-01T02:00:00+02X00"), None); // bad offset separator
        assert_eq!(ts_secs("1970-01-01T00:00:00+02:99"), None); // offset minutes out of range
        assert_eq!(ts_secs("1970-01-01T00:00:00+99:00"), None); // offset hours out of range
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

    #[test]
    fn active_span_handles_mixed_offset_ordering() {
        // Line 1 is "2026-01-01T00:00:00-02:00" (02:00 UTC), line 2 is
        // "2026-01-01T00:30:00Z" (00:30 UTC). Ingest orders actions by raw
        // timestamp STRING (locked contract), and "00:00:00-02:00" sorts
        // before "00:30:00Z" lexicographically — so the action list is in
        // the OPPOSITE order from real UTC time. Without sorting parsed
        // seconds first, `active_span` would clamp the negative gap to 0 and
        // report "active 0m (span 0m)" for a real 1h30m gap. True elapsed
        // time is 5400s; one capped 300s gap.
        let mk = |ts: &str, id: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"/a"}}}}]}}}}"#
            )
        };
        let raw = [
            mk("2026-01-01T00:00:00-02:00", "a"),
            mk("2026-01-01T00:30:00Z", "b"),
        ]
        .join("\n");
        let s = crate::ingest::ingest_str(&raw, crate::model::Lane::Main);
        // Sanity: ingest really did keep the string-inverted order.
        assert_eq!(s.actions[0].effective_ts, "2026-01-01T00:00:00-02:00");
        assert_eq!(s.actions[1].effective_ts, "2026-01-01T00:30:00Z");
        let d = active_span(&s, ACTIVE_GAP_CAP_SECS).unwrap();
        assert_eq!(d.span_secs, 5_400);
        assert_eq!(d.active_secs, 300);
    }
}
