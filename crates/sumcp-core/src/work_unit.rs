//! Work units: the several transcripts that make up one continuous stretch of
//! work. See `docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md`.

use crate::locate::{TranscriptSpan, is_agent_jsonl, transcript_span};
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
    /// Same-directory transcripts whose time span could not be read at
    /// discovery (unreadable file, no parseable timestamp). Such a file
    /// cannot be placed in time, so its unit membership is UNKNOWN: this is
    /// a directory-level count attached to every unit discovered there, not
    /// a claim that these files belong to this unit. Spec §8 requires the
    /// exclusion to be counted rather than silent.
    pub unplaced: u64,
}

/// Gaps in minutes between each member and the running span end before it,
/// over exactly the given members (oldest first). One shorter than the input.
/// Same rule `group_spans` applies while grouping; this recomputes it for a
/// SUBSET (the members that actually loaded), so a payload's gap list can
/// stay one-shorter-than-sessions when a member was discovered but could not
/// be read.
pub fn gaps_between(members: &[Member]) -> Vec<f64> {
    let mut gaps = Vec::new();
    let Some(first) = members.first() else {
        return gaps;
    };
    let mut running_end = first.span.last.clone();
    for m in &members[1..] {
        // Members handed here were already grouped, so their timestamps
        // parsed once before; `unwrap_or(0.0)` is a can't-happen guard, not
        // a policy (NaN would serialize as JSON null and break the schema).
        gaps.push(
            secs_between(&running_end, &m.span.first)
                .map(|g| g as f64 / 60.0)
                .unwrap_or(0.0),
        );
        if m.span.last > running_end {
            running_end = m.span.last.clone();
        }
    }
    gaps
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
                    unplaced: 0,
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
        unplaced: 0,
    };
    let Some(dir) = main_path.parent() else {
        return fallback();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return fallback();
    };

    let mut items: Vec<Member> = Vec::new();
    let mut unplaced = 0u64;
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
        // A subagent transcript must never become a work-unit member in its
        // own right: `load_session` (called later, once per member) already
        // reads and merges it as part of its PARENT transcript, so counting
        // it again here would merge that same file a second time and double
        // every action inside it.
        //
        // Claude Code has stored subagent transcripts in two different
        // places across versions, and each is excluded here for a different
        // reason:
        //   - newer (2.1.x+) layout: namespaced one directory down, at
        //     `<project>/<uuid>/subagents/agent-<id>.jsonl`. Those are
        //     excluded automatically, just by where `std::fs::read_dir(dir)`
        //     above looks: it only lists `dir` itself, never descends into
        //     `dir/<uuid>/subagents`, so a file living one level down is
        //     never even offered to this loop.
        //   - legacy (pre-2.1.x) layout: a SIBLING of the main transcript,
        //     right here in `dir`, named `agent-<id>.jsonl`. Nothing about
        //     its location marks it as a subagent, so it sails straight past
        //     the directory-listing check above and must be excluded by name
        //     instead, with the same `is_agent_jsonl` predicate
        //     `discover_subagent_paths` uses to find it in the first place.
        if is_agent_jsonl(&p) {
            continue;
        }
        if let Some(span) = transcript_span(&p) {
            items.push(Member { path: p, span });
        } else {
            // A file that cannot be placed in time cannot be grouped, but
            // silently vanishing is worse than admitting the gap: it might
            // have belonged to the unit being reported. Counted here,
            // stamped onto the returned unit below, disclosed in payloads.
            unplaced += 1;
        }
    }

    for mut unit in group_spans(items) {
        if unit.members.iter().any(|mm| mm.path == main_path) {
            unit.unplaced = unplaced;
            return unit;
        }
    }
    let mut fb = fallback();
    fb.unplaced = unplaced;
    fb
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
        // `joined_gaps_min` is always one shorter than `members` (there is no
        // gap before the first member). The cap trims both from the front by
        // the same count, so the invariant must still hold after trimming.
        assert_eq!(
            units[0].joined_gaps_min.len(),
            units[0].members.len() - 1,
            "joined_gaps_min must stay one shorter than members after the cap trims"
        );
    }

    #[test]
    fn empty_input_produces_no_units() {
        assert!(group_spans(vec![]).is_empty());
    }

    #[test]
    fn a_legacy_sibling_subagent_file_is_excluded_from_membership() {
        // Legacy layout: `agent-<id>.jsonl` sits right next to the main
        // transcript in the SAME directory, not one directory down the way
        // the newer namespaced layout does. Before the fix, this function
        // had no way to tell that sibling apart from a real top-level
        // transcript, so it was picked up as a second member of its own work
        // unit here -- and then `load_session` would merge the very same
        // file again as a subagent of its parent, double-counting every
        // action inside it. See `legacy_sibling_subagent_edit_is_merged_exactly_once`
        // in `assemble.rs` for the end-to-end symptom.
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("5717aaaa-1111-2222-3333-444455556666.jsonl");
        std::fs::write(
            &main,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:10:00Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        // A legacy subagent sibling whose span sits inside the main
        // transcript's own window, so a bug here would join it into the same
        // unit rather than the timestamps simply missing each other.
        std::fs::write(
            td.path().join("agent-xyz.jsonl"),
            r#"{"type":"assistant","timestamp":"2026-01-01T00:05:00Z"}"#,
        )
        .unwrap();

        let unit = discover_work_unit(&main);
        assert_eq!(
            unit.members.len(),
            1,
            "the legacy subagent sibling must not become a member in its own \
             right: got {:?}",
            unit.members.iter().map(|m| &m.path).collect::<Vec<_>>()
        );
        assert_eq!(unit.members[0].path, main);
    }

    #[test]
    fn an_unplaceable_sibling_is_counted_not_silently_dropped() {
        // A sibling transcript whose span cannot be read (here: no
        // timestamps at all) cannot be grouped, but before the fix it
        // vanished with zero disclosure anywhere: the unit reported itself
        // complete while a file that might belong to it was ignored. Spec §8
        // requires the exclusion to be counted.
        let td = tempfile::tempdir().unwrap();
        let main = td.path().join("dddd0001-1111-2222-3333-444455556666.jsonl");
        std::fs::write(
            &main,
            concat!(
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z"}"#,
                "\n",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:10:00Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            td.path().join("dddd0002-1111-2222-3333-444455556666.jsonl"),
            r#"{"type":"assistant","note":"no timestamp anywhere"}"#,
        )
        .unwrap();

        let unit = discover_work_unit(&main);
        assert_eq!(unit.members.len(), 1, "the datable transcript alone");
        assert_eq!(
            unit.unplaced, 1,
            "the undatable sibling is counted, membership unknown"
        );
    }

    // These anchors are independently known values (computable by hand or by
    // any other correct calendar implementation), not values read off this
    // function. That is the whole point: if `to_epoch_secs` disagreed with
    // any of them, that would mean a real bug in the arithmetic, not a stale
    // test.
    //
    // Why these particular dates:
    //   - 1970-01-01 is the epoch itself: the simplest possible check, and it
    //     pins down the 719_468 rebasing constant.
    //   - 2000-01-01 and 2000-03-01 straddle March 1st in the year 2000.
    //     2000 is a century year (divisible by 100) but ALSO divisible by
    //     400, so unlike 1900 or 2100 it IS a leap year. Getting this pair
    //     right proves the era/century correction (the `/100` and the era
    //     split) is doing the right thing, not just "divisible by 4".
    //   - 2024-01-01 is a plain leap year, unremarkable on its own, but it
    //     sets up the next two anchors.
    //   - 2024-02-29 and 2024-03-01 are the day of, and the day after, a real
    //     leap day. The two together prove the leap day is counted exactly
    //     once: if it were dropped, the gap between these two anchors would
    //     be 0 seconds instead of 86_400; if it were double-counted, some
    //     other anchor pair would be off by a day instead.
    //   - 1969-12-31T23:59:59Z is one second before the epoch, exercising the
    //     negative-era path (`era = ... / 400` for `y2 < 0`), which uses a
    //     different floor-rounding rule than the positive case because plain
    //     integer division truncates toward zero instead of flooring.
    #[test]
    fn to_epoch_secs_anchors_against_independently_known_values() {
        assert_eq!(super::to_epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            super::to_epoch_secs("2000-01-01T00:00:00Z"),
            Some(946_684_800)
        );
        assert_eq!(
            super::to_epoch_secs("2000-03-01T00:00:00Z"),
            Some(951_868_800)
        );
        assert_eq!(
            super::to_epoch_secs("2024-01-01T00:00:00Z"),
            Some(1_704_067_200)
        );
        assert_eq!(
            super::to_epoch_secs("2024-02-29T00:00:00Z"),
            Some(1_709_164_800)
        );
        assert_eq!(
            super::to_epoch_secs("2024-03-01T00:00:00Z"),
            Some(1_709_251_200)
        );
        assert_eq!(super::to_epoch_secs("1969-12-31T23:59:59Z"), Some(-1));
    }

    #[test]
    fn month_boundaries_have_the_right_day_counts() {
        // Consecutive month starts a month apart should differ by exactly
        // that month's length in seconds. A single anchor could hide an
        // off-by-one in the `doy` formula that only shows up for specific
        // month lengths; comparing adjacent month-starts catches it directly
        // because the difference IS the month length.
        let mar1 = super::to_epoch_secs("2026-03-01T00:00:00Z").unwrap();
        let apr1 = super::to_epoch_secs("2026-04-01T00:00:00Z").unwrap();
        let may1 = super::to_epoch_secs("2026-05-01T00:00:00Z").unwrap();
        assert_eq!(apr1 - mar1, 31 * 86_400, "March has 31 days");
        assert_eq!(may1 - apr1, 30 * 86_400, "April has 30 days");
    }

    #[test]
    fn a_gap_across_a_year_boundary_still_joins() {
        // The first transcript ends just before midnight on New Year's Eve,
        // the second starts 15 minutes into the new year. This exercises the
        // epoch conversion as `group_spans` actually uses it (via
        // `secs_between`), not just in isolation: a bug in the year-boundary
        // arithmetic would show up here as a wrongly split unit.
        let units = group_spans(vec![
            m("a", "2025-12-31T23:00:00Z", "2025-12-31T23:50:00Z"),
            m("b", "2026-01-01T00:05:00Z", "2026-01-01T01:00:00Z"),
        ]);
        assert_eq!(
            units.len(),
            1,
            "a 15 minute gap across the year boundary must join"
        );
        assert_eq!(units[0].members.len(), 2);
    }

    #[test]
    fn a_gap_of_exactly_the_idle_threshold_joins() {
        // The join rule is "less than or EQUAL TO" WORK_UNIT_IDLE_GAP_SECS.
        // The other tests use 29 and 31 minutes, which straddle 30 minutes
        // without ever testing the boundary value itself.
        let units = group_spans(vec![
            m("a", "2026-01-01T00:00:00Z", "2026-01-01T01:00:00Z"),
            m("b", "2026-01-01T01:30:00Z", "2026-01-01T02:00:00Z"),
        ]);
        assert_eq!(
            secs_between("2026-01-01T01:00:00Z", "2026-01-01T01:30:00Z"),
            Some(WORK_UNIT_IDLE_GAP_SECS)
        );
        assert_eq!(units.len(), 1, "a gap exactly at the threshold must join");
        assert_eq!(units[0].members.len(), 2);
    }
}
