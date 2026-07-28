//! Work units: the several transcripts that make up one continuous stretch of
//! work. See `docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md`.

use crate::locate::{TranscriptSpan, transcript_span};
use std::path::{Path, PathBuf};

/// The idle gap that separates two stretches of work, in seconds.
///
/// DECLARED, NOT FITTED. Measured on this machine's 82-transcript corpus on
/// 2026-07-28, grouping is almost completely insensitive to this value:
///
/// | gap     | pairs joined | work units |
/// |---------|--------------|------------|
/// | 5 min   | 45           | 37         |
/// | 30 min  | 48           | 34         |
/// | 120 min | 51           | 31         |
///
/// Across a 24x range the unit count moves from 37 to 31, so there is almost
/// nothing to fit and the choice is a readability decision. Deliberately not
/// user-configurable: ADR A6's TOML override was removed once already for
/// adding surface without adding value.
pub const WORK_UNIT_IDLE_GAP_SECS: i64 = 30 * 60;

/// Most transcripts we will merge into one unit. The largest unit observed in
/// the corpus is 8, so this is generous headroom rather than a real limit.
/// Exceeding it is disclosed, never silent.
pub const MAX_WORK_UNIT_SESSIONS: usize = 16;

/// One transcript, with the span of time it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Absolute path to the main transcript.
    pub path: PathBuf,
    /// Its first and last timestamp.
    pub span: TranscriptSpan,
}

/// A set of transcripts that make up one continuous stretch of work.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkUnit {
    /// Members, oldest first.
    pub members: Vec<Member>,
    /// Gap in minutes between each member and the running span end before it.
    /// One shorter than `members`. A NEGATIVE value means that transcript
    /// overlapped the running span (a concurrent Claude Code instance) rather
    /// than following it.
    pub joined_gaps_min: Vec<f64>,
    /// Members discarded because the unit exceeded `MAX_WORK_UNIT_SESSIONS`.
    pub dropped: u64,
}

/// Seconds from `a` to `b`, where both are RFC 3339 UTC strings.
///
/// Parsed by hand rather than with a date crate, because the workspace has no
/// date dependency and is not getting one. The shape is fixed:
/// `YYYY-MM-DDTHH:MM:SS` with optional fractional seconds and a `Z`.
/// Returns `None` if either string does not have that shape.
fn secs_between(a: &str, b: &str) -> Option<i64> {
    Some(to_epoch_secs(b)? - to_epoch_secs(a)?)
}

/// Convert `YYYY-MM-DDTHH:MM:SS...` to seconds since 1970-01-01, UTC.
///
/// This hand-rolls the "days from civil" algorithm (Howard Hinnant's, the
/// standard branch-free one used by real date libraries) instead of pulling
/// in a date crate, because the workspace takes no new dependencies and the
/// only thing we need from a whole calendar library is "how many days have
/// elapsed since the epoch".
///
/// A plain-language walkthrough of the trick, for anyone new to this:
/// counting days in the Gregorian calendar is fiddly because February is a
/// different length in leap years, so the "day of year" for March onward
/// shifts depending on whether the current year is a leap year. The trick is
/// to relabel the calendar so the year starts on March 1st instead of
/// January 1st. Then the leap day (Feb 29) becomes the very LAST day of the
/// shifted year, and it never again changes the day-count of any month that
/// comes before it. Every month's offset within a shifted year is now fixed,
/// no matter whether that year is a leap year. That is what `y2` (the
/// shifted year) and `doy` (day of shifted year) do below.
///
/// An "era" is just a convenient chunk of 400 shifted years. 400 years is the
/// period after which the Gregorian leap-year pattern exactly repeats itself
/// (it has 97 leap years in every 400, always), so splitting the year count
/// into whole eras plus a remainder ("year of era", `yoe`, 0..=399) lets the
/// leap-year corrections (`/4`, `/100`) be done once per era with plain
/// integer division, instead of needing a lookup table or a loop.
fn to_epoch_secs(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { ts.get(from..to)?.parse::<i64>().ok() };
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);

    // Shift the year so that March is month 1; this makes the leap day the
    // last day of the "year" and removes every special case from the maths.
    // January and February of calendar year Y become months 11 and 12 of
    // shifted year Y-1, which is why `y2` subtracts one for those months.
    let y2 = if m <= 2 { y - 1 } else { y };
    // Round `y2` down to the nearest multiple of 400 to find which era it is
    // in. Plain integer division rounds negative numbers toward zero (not
    // downward), so a manual floor-adjustment (`y2 - 399`) is needed for
    // years before era 0; this codebase only ever sees years well after
    // 1970, but the formula is kept general and correct either way.
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400; // year of era, 0..=399
    // Day of the shifted year (0-based): month offsets after the March shift
    // follow a fixed 5-months-per-2-months pattern, which `(153 * m2 + 2) / 5`
    // reproduces without a per-month lookup table.
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    // Day of era: 365 days per year, plus one leap day every 4 years, minus
    // one every 100 (century years are not leap), the /100 correction having
    // already been folded into the era-level accounting above.
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of era
    // 719_468 is the day-count from 0000-03-01 (the epoch of this shifted
    // calendar) to 1970-01-01, so subtracting it re-bases the count onto the
    // Unix epoch.
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// Group transcripts into work units.
///
/// Pure: no filesystem access, and the input need not be sorted. Two
/// transcripts join when the later one overlaps the running span of the unit
/// so far, or begins within `WORK_UNIT_IDLE_GAP_SECS` of its end. If a
/// timestamp cannot be parsed, we start a new unit rather than guess: a gap
/// we cannot measure is not the same as a gap of zero.
pub fn group_spans(mut items: Vec<Member>) -> Vec<WorkUnit> {
    if items.is_empty() {
        return Vec::new();
    }
    // Sort by start time, then by path, so the grouping cannot depend on the
    // order the filesystem (or caller) happened to hand items in. `&str`
    // comparison is chronological order here because Claude Code timestamps
    // are fixed-width RFC 3339 UTC strings (see `TranscriptSpan`'s doc).
    items.sort_by(|a, b| (&a.span.first, &a.path).cmp(&(&b.span.first, &b.path)));

    let mut units: Vec<WorkUnit> = Vec::new();
    // The running end of the unit currently being built: the LATEST `last`
    // timestamp seen among its members so far, not merely the previous
    // member's. Tracked as its own variable (rather than re-derived from
    // `units.last()` each time) so extending it is a single, explicit step
    // instead of a conditional patched on afterward.
    let mut running_end = String::new();

    for item in items {
        // Decide whether `item` joins the open unit before mutating anything.
        // `None` covers both "no unit is open yet" and "the gap could not be
        // measured", and both cases start a new unit.
        let gap = if units.is_empty() {
            None
        } else {
            secs_between(&running_end, &item.span.first)
        };

        match gap {
            // Joins: a negative gap means `item` overlaps the running span
            // (a concurrent Claude Code instance); a small positive gap means
            // it is a continuation after an idle stretch.
            Some(g) if g <= WORK_UNIT_IDLE_GAP_SECS => {
                // Grab the timestamp we need before `item` is moved into the
                // unit below, so this branch does not need to clone the
                // whole `Member` (path included) just to read its `last`.
                let last = item.span.last.clone();
                let unit = units.last_mut().expect("units is non-empty: gap was Some");
                unit.joined_gaps_min.push(g as f64 / 60.0);
                unit.members.push(item);
                // The unit's end only ever grows: a short transcript that
                // finishes before the running end must not pull it backward.
                if last > running_end {
                    running_end = last;
                }
            }
            // Starts a new unit: either the gap was too long, or the
            // timestamp did not parse. The new unit's running end is this
            // member's own `last`, with no carry-over from the unit before.
            _ => {
                running_end = item.span.last.clone();
                units.push(WorkUnit {
                    members: vec![item],
                    joined_gaps_min: Vec::new(),
                    dropped: 0,
                });
            }
        }
    }

    // Apply the cap: keep the NEWEST members, disclose the rest. Trimming
    // from the front is right because members are oldest-first, and the
    // newest transcript is the one the user just finished and most wants
    // described.
    for unit in units.iter_mut() {
        if unit.members.len() > MAX_WORK_UNIT_SESSIONS {
            let excess = unit.members.len() - MAX_WORK_UNIT_SESSIONS;
            unit.members.drain(0..excess);
            // `joined_gaps_min` is one shorter than `members` (there is no
            // gap before the first member), so trimming the same count from
            // its front keeps the two aligned. `min` guards the case where
            // fewer gaps exist than `excess` (which should not happen given
            // the invariant, but costs nothing to guard).
            let gap_excess = excess.min(unit.joined_gaps_min.len());
            unit.joined_gaps_min.drain(0..gap_excess);
            unit.dropped = excess as u64;
        }
    }
    units
}

/// Find the work unit containing `main_path`, by scanning its project
/// directory for other transcripts and grouping them. Transcripts with no
/// resolvable time span are excluded, since a transcript that cannot be
/// placed in time cannot be grouped; `main_path` itself always comes back as
/// at least a single-member unit, even when its own span cannot be read.
pub fn discover_work_unit(main_path: &Path) -> WorkUnit {
    let fallback = || WorkUnit {
        members: vec![Member {
            path: main_path.to_path_buf(),
            span: TranscriptSpan {
                first: String::new(),
                last: String::new(),
            },
        }],
        joined_gaps_min: Vec::new(),
        dropped: 0,
    };
    let Some(dir) = main_path.parent() else {
        return fallback();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return fallback();
    };

    let mut items: Vec<Member> = Vec::new();
    for entry in entries.flatten() {
        let p = entry.path();
        // This function enumerates a whole directory, which is exactly the
        // kind of call site ADR A9 requires to defend itself: unlike
        // `transcript_span`, which trusts a path handed to it by a caller
        // that already validated it, nothing here has vetted what is sitting
        // in this directory. `p.is_symlink()` (checked without following the
        // link) rejects a planted `<uuid>.jsonl -> ~/.ssh/id_rsa` before its
        // target is ever opened. `is_file()` (which DOES follow symlinks)
        // runs after, so a symlink is rejected here regardless of what it
        // points to.
        if p.is_symlink() || !p.is_file() {
            continue;
        }
        if p.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        if let Some(span) = transcript_span(&p) {
            items.push(Member { path: p, span });
        }
    }

    for unit in group_spans(items) {
        if unit.members.iter().any(|mm| mm.path == main_path) {
            return unit;
        }
    }
    fallback()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a Member with a synthetic path and a span of two timestamps.
    fn m(name: &str, first: &str, last: &str) -> Member {
        Member {
            path: PathBuf::from(format!("/p/{name}.jsonl")),
            span: TranscriptSpan {
                first: first.to_string(),
                last: last.to_string(),
            },
        }
    }

    #[test]
    fn a_short_gap_joins_and_a_long_gap_splits() {
        // b starts 29 min after a ends: joins. c starts 31 min after b ends:
        // splits. These two cases straddle the declared 30 minute rule.
        let units = group_spans(vec![
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"),
            m("b", "2026-01-01T01:29:00Z", "2026-01-01T02:00:00Z"),
            m("c", "2026-01-01T02:31:00Z", "2026-01-01T03:00:00Z"),
        ]);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].members.len(), 2);
        assert_eq!(units[1].members.len(), 1);
    }

    #[test]
    fn overlapping_transcripts_join_and_report_a_negative_gap() {
        // Two concurrent Claude Code instances in one project. b starts
        // BEFORE a ends, so the reported gap is negative, which is how a
        // reader tells a concurrent instance from a continuation.
        let units = group_spans(vec![
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T02:00:00Z"),
            m("b", "2026-01-01T01:00:00Z", "2026-01-01T03:00:00Z"),
        ]);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].members.len(), 2);
        assert_eq!(units[0].joined_gaps_min, vec![-60.0]);
    }

    #[test]
    fn the_gap_is_measured_from_the_running_span_end_not_the_previous_member() {
        // a runs long and swallows b. c starts 10 min after A's end, which is
        // well after b's end. Measuring from b alone would wrongly split.
        let units = group_spans(vec![
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T05:00:00Z"),
            m("b", "2026-01-01T01:00:00Z", "2026-01-01T01:10:00Z"),
            m("c", "2026-01-01T05:10:00Z", "2026-01-01T06:00:00Z"),
        ]);
        assert_eq!(units.len(), 1, "all three are one stretch of work");
        assert_eq!(units[0].members.len(), 3);
    }

    #[test]
    fn input_order_does_not_change_the_grouping() {
        let forward = group_spans(vec![
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"),
            m("b", "2026-01-01T01:10:00Z", "2026-01-01T02:00:00Z"),
        ]);
        let reversed = group_spans(vec![
            m("b", "2026-01-01T01:10:00Z", "2026-01-01T02:00:00Z"),
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"),
        ]);
        let names = |u: &Vec<WorkUnit>| -> Vec<Vec<PathBuf>> {
            u.iter()
                .map(|x| x.members.iter().map(|mm| mm.path.clone()).collect())
                .collect()
        };
        assert_eq!(names(&forward), names(&reversed));
    }

    #[test]
    fn a_unit_over_the_cap_keeps_the_newest_and_discloses_the_drop() {
        // 20 transcripts, each starting one minute after the last ends, so
        // they are all one unit. The cap keeps 16 and discloses 4 dropped.
        let mut items = Vec::new();
        for i in 0..20 {
            items.push(m(
                &format!("s{i:02}"),
                &format!("2026-01-01T{:02}:00:00Z", i),
                &format!("2026-01-01T{:02}:30:00Z", i),
            ));
        }
        let units = group_spans(items);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].members.len(), MAX_WORK_UNIT_SESSIONS);
        assert_eq!(units[0].dropped, 4);
        // The NEWEST are kept: the last member is s19, not s15.
        let last = units[0].members.last().unwrap().path.clone();
        assert!(last.ends_with("s19.jsonl"), "kept the newest, got {last:?}");
    }

    #[test]
    fn empty_input_produces_no_units() {
        assert!(group_spans(vec![]).is_empty());
    }
}
