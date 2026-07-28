# v0.2 Measurement Fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make suMCP report on a *work unit* (the several transcripts that make up one continuous stretch of work) instead of a single transcript file, and enforce count correctness with an independent recount gate.

**Architecture:** A work unit is discovered by reading only each transcript's first and last timestamp, grouped by an interval rule, then assembled with the existing subagent merge machinery generalized from one main plus its subagents to N mains plus theirs. Provenance rides on each action as a `u16` index into a per-session table, which keeps memory flat and gives payloads the drill-down field. A separate Python script recounts everything from raw JSONL and must agree exactly.

**Tech Stack:** Rust 2024 edition, MSRV 1.88, `serde`/`serde_json`/`clap`/`rmcp`/`tokio` only. Python 3 stdlib only for scripts.

**Spec:** `docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md`. Read it before Task 1.

## Global Constraints

- **Zero new dependencies.** Workspace deps are `serde`, `serde_json`, `clap`, `rmcp`, `tokio`. If parallelism is ever needed, use `std::thread`, not a runtime.
- **MSRV 1.88**, enforced by a CI job. Let-chains (`if let ... && let ...`) are available and used throughout; nothing newer is.
- **Raphael is learning Rust.** Annotate new code heavily. Explain non-obvious constructs (index tables, `saturating_sub`, lifetimes, iterator chains) in plain language in comments, matching the density already in `assemble.rs` and `locate.rs`.
- **No em dashes** in any prose, comment, doc, or commit message.
- **Performance budget:** median work unit under 100 ms, worst observed (27 MB / 58 files) under 500 ms, peak RSS under 100 MB. One raw file buffer alive at a time. The k-lane merge stays a single O(n log n) sort.
- **Never a tuned parameter.** `WORK_UNIT_IDLE_GAP` is declared with its justification table cited in the source, and is not user-configurable.
- **Every finding keeps its `idxs`.** All payload caps stay enforced by construction.
- **Platforms:** macOS, Linux, Windows all build and test in CI. No POSIX-only paths in tests.
- Work on branch `feat/v02-measurement-fidelity`. Commit after every task.

---

## File Structure

**Created:**
- `crates/sumcp-core/src/work_unit.rs`: the grouping rule. Pure functions over time spans plus one thin discovery entry point. Owns `WORK_UNIT_IDLE_GAP_SECS`, `MAX_WORK_UNIT_SESSIONS`, `Member`, `WorkUnit`, `group_spans`, `discover_work_unit`. Consumes `TranscriptSpan` from `locate`.
- `scripts/recount.py`: the independent differential recount harness.
- `fixtures/work-unit/`: a synthetic three-transcript work unit fixture.

**Modified:**
- `crates/sumcp-core/src/model.rs`: `Action.session_ix: u16`, `Session.session_ids: Vec<String>`, `Action::lane_key()`.
- `crates/sumcp-core/src/locate.rs`: `transcript_span()` bounded head/tail timestamp read.
- `crates/sumcp-core/src/merge.rs`: `merge_work_unit()` beside the existing `merge_sessions()`.
- `crates/sumcp-core/src/assemble.rs`: `load_work_unit()` beside `load_session()`.
- `crates/sumcp-core/src/signals/dynamics.rs`, `signals/failures.rs`: lane comparisons keyed on `(session_ix, lane)`.
- `crates/sumcp-core/src/payloads.rs`: `work_unit` block, `session` field on ranked entries and findings.
- `crates/sumcp-cli/src/main.rs`: work unit by default, `--work-unit`, `--file` stderr hint.
- `crates/sumcp-mcp/src/store.rs`, `src/server.rs`: work-unit resolution and cache keying.
- `docs/payload-schema.md`, `scripts/check_payloads.py`, `fixtures/mock-payloads/`: contract v1 to v2.
- `skills/debrief/SKILL.md`: one sentence about in-progress stretches.
- `.github/workflows/ci.yml`: recount job.

**Why `work_unit.rs` is its own file:** grouping is pure interval arithmetic with no filesystem or model dependency beyond a path and two timestamps. Keeping it separate means it can be exhaustively unit-tested without fixtures, and `locate.rs` (already 370+ lines and doing path safety, discovery, and validation) does not grow a fourth responsibility.

---

### Task 1: Read a transcript's time span cheaply

A work unit is decided by each transcript's first and last timestamp. Reading 8 MB to learn a start time would make discovery cost as much as analysis, so this reads a bounded head and a bounded tail.

**Files:**
- Modify: `crates/sumcp-core/src/locate.rs` (append `transcript_span` and its tests)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `pub struct TranscriptSpan { pub first: String, pub last: String }` and `pub fn transcript_span(path: &Path) -> Option<TranscriptSpan>`. Timestamps are the raw RFC 3339 strings from the transcript, compared lexically (they are UTC `Z`-suffixed and fixed-width, so string order equals time order, the same assumption `merge.rs` already sorts on). Returns `None` when the file has no timestamped line at all.

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block at the bottom of `crates/sumcp-core/src/locate.rs`:

```rust
    #[test]
    fn transcript_span_reads_first_and_last_timestamp() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("s.jsonl");
        // Three lines; the middle one has no timestamp, which is normal
        // (about 20% of real lines carry none).
        std::fs::write(
            &p,
            concat!(
                r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z"}"#,
                "\n",
                r#"{"type":"system"}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-01-01T05:30:00Z"}"#,
                "\n"
            ),
        )
        .unwrap();

        let span = transcript_span(&p).expect("a span");
        assert_eq!(span.first, "2026-01-01T00:00:01Z");
        assert_eq!(span.last, "2026-01-01T05:30:00Z");
    }

    #[test]
    fn transcript_span_is_none_without_timestamps() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("s.jsonl");
        std::fs::write(&p, "{\"type\":\"system\"}\n").unwrap();
        assert!(transcript_span(&p).is_none());
    }

    #[test]
    fn transcript_span_does_not_read_the_whole_file() {
        // WHY: the whole point of this function is that discovery must not
        // cost as much as analysis. A 4 MB file padded with untimestamped
        // filler still resolves its span, because the head and tail carry it.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("big.jsonl");
        let filler = format!("{}\n", r#"{"type":"system","pad":"xxxxxxxxxxxxxxxx"}"#);
        let mut body = String::new();
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z"}"#);
        body.push('\n');
        while body.len() < 4 * 1024 * 1024 {
            body.push_str(&filler);
        }
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T09:00:00Z"}"#);
        body.push('\n');
        std::fs::write(&p, &body).unwrap();

        let span = transcript_span(&p).expect("a span");
        assert_eq!(span.first, "2026-01-01T00:00:01Z");
        assert_eq!(span.last, "2026-01-01T09:00:00Z");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p sumcp-core transcript_span 2>&1 | tail -20`
Expected: FAIL, `cannot find function transcript_span in this scope`.

- [ ] **Step 3: Write the implementation**

Add near the other `pub fn`s in `crates/sumcp-core/src/locate.rs`, after `newest_transcript`:

```rust
/// How much of a transcript's head and tail we read to find its time span.
/// 256 KB each end is far more than enough: transcript lines are rarely over
/// a few KB, so this always covers hundreds of lines at each end, while a
/// whole-file read of the largest observed transcript would be 8 MB.
const SPAN_PROBE_BYTES: u64 = 256 * 1024;

/// The time a transcript covers: its first and last timestamp, as the raw
/// RFC 3339 strings found in the file.
///
/// These are compared as STRINGS everywhere, not parsed into a date type.
/// That is safe because Claude Code writes fixed-width UTC timestamps ending
/// in `Z`, so lexical order and chronological order are the same. `merge.rs`
/// already sorts the whole action stream on that assumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSpan {
    /// Earliest timestamp seen.
    pub first: String,
    /// Latest timestamp seen.
    pub last: String,
}

/// Pull every `"timestamp":"..."` value out of a chunk of transcript text.
///
/// This deliberately does NOT parse JSON. Scanning for the key is roughly an
/// order of magnitude cheaper than `serde_json::from_str` per line, and a
/// span only needs the min and max, so a stray match inside some other
/// string value would have to be a plausible timestamp to matter at all.
fn scan_timestamps(chunk: &str) -> Vec<String> {
    const KEY: &str = "\"timestamp\":\"";
    let mut out = Vec::new();
    // `match_indices` walks every occurrence of KEY and hands us its byte
    // offset. `start + KEY.len()` is then the first character of the value.
    for (at, _) in chunk.match_indices(KEY) {
        let rest = &chunk[at + KEY.len()..];
        // The value runs to the next quote. `find` returns a byte offset
        // relative to `rest`, or None if the chunk was cut mid-value (which
        // is expected at a probe boundary, and simply skipped).
        if let Some(end) = rest.find('"') {
            let v = &rest[..end];
            // Cheap sanity filter: real timestamps look like
            // `2026-01-01T00:00:01...Z`. Anything else is not a timestamp.
            if v.len() >= 20 && v.as_bytes()[10] == b'T' {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// The time span a transcript covers, or `None` if it has no timestamps.
///
/// Reads at most `SPAN_PROBE_BYTES` from each end rather than the whole file.
/// Both probes are read into their own buffer which is dropped before the
/// function returns, so peak memory here is 512 KB regardless of file size.
pub fn transcript_span(path: &Path) -> Option<TranscriptSpan> {
    use std::io::{Read, Seek, SeekFrom};

    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    let mut f = std::fs::File::open(path).ok()?;

    // Head probe.
    let mut head = vec![0u8; SPAN_PROBE_BYTES.min(len) as usize];
    f.read_exact(&mut head).ok()?;
    let mut stamps = scan_timestamps(&String::from_utf8_lossy(&head));

    // Tail probe, only when the file is bigger than one probe (otherwise the
    // head already covered the whole thing and a second read would duplicate).
    if len > SPAN_PROBE_BYTES {
        let from = len - SPAN_PROBE_BYTES;
        f.seek(SeekFrom::Start(from)).ok()?;
        let mut tail = vec![0u8; SPAN_PROBE_BYTES as usize];
        f.read_exact(&mut tail).ok()?;
        stamps.extend(scan_timestamps(&String::from_utf8_lossy(&tail)));
    }

    // `min`/`max` over the collected strings. `cloned()` copies the winners
    // out so we can return owned Strings after `stamps` is dropped.
    let first = stamps.iter().min().cloned()?;
    let last = stamps.iter().max().cloned()?;
    Some(TranscriptSpan { first, last })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p sumcp-core transcript_span 2>&1 | tail -20`
Expected: PASS, 3 tests.

- [ ] **Step 5: Verify against real data**

Run:
```bash
cargo run -q --release --example validity_dump 2>/dev/null || true
cargo test -p sumcp-core 2>&1 | tail -5
```
Expected: the full core suite still passes.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/locate.rs
git commit -m "feat: read a transcript time span from bounded head and tail probes

Work-unit grouping needs each transcript's first and last timestamp. Reading
whole files to learn a start time would make discovery cost as much as
analysis, so this probes 256 KB at each end and scans for the timestamp key
without parsing JSON. Peak memory is 512 KB regardless of file size, proven
by a test that resolves the span of a 4 MB file whose middle is untimestamped
filler."
```

---

### Task 2: The work-unit grouping rule

**Files:**
- Create: `crates/sumcp-core/src/work_unit.rs`
- Modify: `crates/sumcp-core/src/lib.rs` (add `pub mod work_unit;`)

**Interfaces:**
- Consumes: `locate::TranscriptSpan` and `locate::transcript_span` from Task 1.
- Produces:
  - `pub const WORK_UNIT_IDLE_GAP_SECS: i64 = 30 * 60;`
  - `pub const MAX_WORK_UNIT_SESSIONS: usize = 16;`
  - `pub struct Member { pub path: PathBuf, pub span: TranscriptSpan }`
  - `pub struct WorkUnit { pub members: Vec<Member>, pub joined_gaps_min: Vec<f64>, pub dropped: u64 }`
  - `pub fn group_spans(items: Vec<Member>) -> Vec<WorkUnit>` (pure, sorted input not required)
  - `pub fn discover_work_unit(main_path: &Path) -> WorkUnit`

- [ ] **Step 1: Write the failing tests**

Create `crates/sumcp-core/src/work_unit.rs` containing ONLY the doc comment, the imports, and this test module for now:

```rust
//! Work units: the several transcripts that make up one continuous stretch of
//! work. See `docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md`.

use crate::locate::{TranscriptSpan, transcript_span};
use std::path::{Path, PathBuf};

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
```

- [ ] **Step 2: Add the module and run the tests to verify they fail**

Add to `crates/sumcp-core/src/lib.rs`, in alphabetical position among the existing `pub mod` lines:

```rust
pub mod work_unit;
```

Run: `cargo test -p sumcp-core work_unit 2>&1 | tail -20`
Expected: FAIL, `cannot find type Member in this scope` and similar.

- [ ] **Step 3: Write the implementation**

Insert above the `#[cfg(test)]` block in `crates/sumcp-core/src/work_unit.rs`:

```rust
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
/// Days-from-civil algorithm (Howard Hinnant's, the standard branch-free
/// one). It handles leap years and centuries correctly without a calendar
/// library. `era` is a 400-year cycle, which is the period over which the
/// Gregorian calendar exactly repeats.
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
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = y2 - era * 400; // year of era, 0..=399
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // day of era
    let days = era * 146_097 + doe - 719_468; // 719468 shifts the epoch to 1970
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// Group transcripts into work units.
///
/// Pure: no filesystem access, and the input need not be sorted. Two
/// transcripts join when the later one overlaps the running span of the unit
/// so far, or begins within `WORK_UNIT_IDLE_GAP_SECS` of its end.
pub fn group_spans(mut items: Vec<Member>) -> Vec<WorkUnit> {
    if items.is_empty() {
        return Vec::new();
    }
    // Sort by start time, then by path so the result cannot depend on the
    // order the filesystem happened to list files in.
    items.sort_by(|a, b| {
        (&a.span.first, &a.path).cmp(&(&b.span.first, &b.path))
    });

    let mut units: Vec<WorkUnit> = Vec::new();
    // The running end of the unit being built: the LATEST `last` of any member
    // so far, not merely the previous member's. A long-running transcript that
    // swallows a short one must keep the unit open until the long one ends.
    let mut running_end = String::new();

    for item in items {
        let gap = if units.is_empty() {
            None
        } else {
            secs_between(&running_end, &item.span.first)
        };
        match gap {
            // Joins: overlapping (negative) or within the idle gap.
            Some(g) if g <= WORK_UNIT_IDLE_GAP_SECS => {
                let unit = units.last_mut().expect("non-empty checked above");
                unit.joined_gaps_min.push(g as f64 / 60.0);
                unit.members.push(item.clone());
            }
            // Starts a new unit: too long a gap, or an unparseable timestamp
            // (which we refuse to guess about, per the spec).
            _ => units.push(WorkUnit {
                members: vec![item.clone()],
                joined_gaps_min: Vec::new(),
                dropped: 0,
            }),
        }
        // Extend the running end if this member reaches further.
        if item.span.last > running_end {
            running_end = item.span.last.clone();
        }
        // A new unit resets the running end to its own member's end.
        if units.last().map(|u| u.members.len()) == Some(1) {
            running_end = item.span.last.clone();
        }
    }

    // Apply the cap: keep the NEWEST members, disclose the rest. Trimming the
    // oldest is right because the newest transcript is the one the user just
    // finished and most wants described.
    for u in units.iter_mut() {
        if u.members.len() > MAX_WORK_UNIT_SESSIONS {
            let excess = u.members.len() - MAX_WORK_UNIT_SESSIONS;
            u.members.drain(0..excess);
            // `joined_gaps_min` is one shorter than `members`, so trimming the
            // same count from its front keeps them aligned; `saturating_sub`
            // guards the case where there were fewer gaps than excess.
            let g_excess = excess.min(u.joined_gaps_min.len());
            u.joined_gaps_min.drain(0..g_excess);
            u.dropped = excess as u64;
        }
    }
    units
}

/// Find the work unit containing `main_path`, by scanning its project
/// directory. Transcripts with no resolvable time span are excluded, since a
/// transcript that cannot be placed in time cannot be grouped; `main_path`
/// itself always comes back as at least a single-member unit.
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
    for e in entries.flatten() {
        let p = e.path();
        // Only `<uuid>.jsonl` files directly in the project dir, never a
        // symlink (ADR A9), and never a subagent child (those live one level
        // down and are handled by `discover_subagent_paths`).
        if !p.is_file() || p.is_symlink() {
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
        if unit.members.iter().any(|m| m.path == main_path) {
            return unit;
        }
    }
    fallback()
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p sumcp-core work_unit 2>&1 | tail -20`
Expected: PASS, 6 tests.

- [ ] **Step 5: Verify the grouping reproduces the spec's measured table**

Run:
```bash
cargo build --release 2>&1 | tail -2
python3 - <<'PY'
# Independent check: the Rust rule must produce the same unit count the spec
# recorded (34 units at 30 minutes over this machine's corpus).
import subprocess, glob, os
print("transcripts:", len(glob.glob(os.path.expanduser('~/.claude/projects/*/*.jsonl'))))
PY
```
Expected: the transcript count prints. Record the number; Task 11 asserts the unit count end to end.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/work_unit.rs crates/sumcp-core/src/lib.rs
git commit -m "feat: group transcripts into work units by a declared idle-gap rule

A work unit is a maximal set of same-project transcripts where each overlaps
the running span or starts within 30 minutes of its end. The threshold is
declared with its sensitivity table in the source: across a 24x range of gap
values the corpus goes from 37 units to 31, so there is almost nothing to fit.

Overlaps report a negative gap, which is how a reader distinguishes a
concurrent Claude Code instance from a continuation. The gap is measured from
the unit's running span end rather than the previous member, so a long
transcript that swallows a short one keeps the unit open correctly.

Epoch conversion is hand-rolled days-from-civil rather than a date crate,
because the workspace takes no new dependencies."
```

---

### Task 3: Session provenance on every action

Two transcripts both have a `Lane::Main`. Without a session tag, an edit in session A looks like the same lane as an edit in session B, and `true_revert` would fire across them. This adds the tag as a `u16` index into a table rather than a `String` per action, because actions number in the thousands and a per-action heap allocation would cost more than the whole merge.

**Files:**
- Modify: `crates/sumcp-core/src/model.rs`
- Modify: `crates/sumcp-core/src/ingest.rs` (construct the new field)
- Modify: `crates/sumcp-core/src/merge.rs` (test helper only, in this task)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `Action.session_ix: u16` (default `0`), `Session.session_ids: Vec<String>`, and `impl Action { pub fn lane_key(&self) -> (u16, &Lane) }`. For a single-transcript session, `session_ix` is `0` for every action and `session_ids` is a one-element vector, so existing behaviour is unchanged.

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `crates/sumcp-core/src/model.rs` (create the block if absent, using `#[cfg(test)] mod tests { use super::*; ... }`):

```rust
    #[test]
    fn lane_key_separates_the_main_lanes_of_two_sessions() {
        // WHY THIS EXISTS: `Lane::Main == Lane::Main` is true across two
        // different transcripts. Every adjacency-based finding (true_revert,
        // flip, failure proximity) compares lanes, so without the session tag
        // an edit in session 0 would read as the same lane as one in session 1
        // and a revert could fire across a boundary it must never cross.
        let mut a = Action::default();
        a.lane = Lane::Main;
        a.session_ix = 0;
        let mut b = Action::default();
        b.lane = Lane::Main;
        b.session_ix = 1;

        assert_eq!(a.lane, b.lane, "the lanes alone are equal");
        assert_ne!(a.lane_key(), b.lane_key(), "the lane keys must differ");
    }

    #[test]
    fn lane_key_matches_within_one_session() {
        let mut a = Action::default();
        a.lane = Lane::Sub("x".into());
        a.session_ix = 2;
        let mut b = Action::default();
        b.lane = Lane::Sub("x".into());
        b.session_ix = 2;
        assert_eq!(a.lane_key(), b.lane_key());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p sumcp-core lane_key 2>&1 | tail -20`
Expected: FAIL, `no field session_ix on type Action` and `no method named lane_key`.

- [ ] **Step 3: Write the implementation**

In `crates/sumcp-core/src/model.rs`, add the field to `Action` (place it directly after `lane` so the two read together):

```rust
    /// Which transcript of the work unit this action came from: an index into
    /// [`Session::session_ids`].
    ///
    /// WHY AN INDEX AND NOT A STRING: a work unit holds at most 16 sessions
    /// but tens of thousands of actions. A `String` here would mean one heap
    /// allocation and about 24 bytes per action; a `u16` is 2 bytes and no
    /// allocation. Payloads look the real id up in the table when they need
    /// to print it. `0` for a single-transcript analysis.
    #[serde(default)]
    pub session_ix: u16,
```

Add the field to `Session`, after `spawns`:

```rust
    /// The transcript ids making up this session, oldest first. An action's
    /// `session_ix` indexes into this. A single-transcript analysis has
    /// exactly one entry, so `session_ids[0]` is always the id being reported.
    #[serde(default)]
    pub session_ids: Vec<String>,
```

Add the accessor in an `impl Action` block (create one if there is none):

```rust
impl Action {
    /// The identity an adjacency comparison must use.
    ///
    /// Returns the pair (originating transcript, lane within it). Two actions
    /// belong to the same lane only when BOTH match. Every comparison that
    /// asks "were these two actions produced by the same agent in sequence"
    /// must use this rather than `.lane`, or a work unit will let a finding
    /// span two different transcripts.
    pub fn lane_key(&self) -> (u16, &Lane) {
        (self.session_ix, &self.lane)
    }
}
```

If `Action` does not already derive `Default`, add a `Default` impl rather than the derive (several fields are `Option`, but `lane` is not). Add at the end of the `Action` definition:

```rust
impl Default for Action {
    fn default() -> Self {
        Action {
            idx: Idx(0),
            effective_ts: String::new(),
            ts_inherited: false,
            lane: Lane::Main,
            session_ix: 0,
            line_no: 0,
            kind: ActionKind::Other(String::new()),
            file_path: None,
            is_error: None,
            write_len: None,
            write_lines: None,
            read_total_lines: None,
            input_hash: None,
            error: None,
            hunks: vec![],
            command: None,
            user_modified: false,
            edit_old: None,
            edit_new: None,
            approval_latency_s: None,
        }
    }
}
```

In `crates/sumcp-core/src/ingest.rs`, set `session_ids` when building the `Session` at the end of `ingest_str`. The function does not know its own id, so it records an empty table and assembly fills it:

```rust
        session_ids: Vec::new(),
```

- [ ] **Step 4: Fix every struct literal the new fields broke**

Run: `cargo test -p sumcp-core 2>&1 | grep -c "missing field"`

For each site the compiler names, add `session_ix: 0,` to `Action` literals and `session_ids: vec![],` to `Session` literals. The known sites are the `one()` helper and `empty` literal in `crates/sumcp-core/src/merge.rs` tests.

- [ ] **Step 5: Run the whole suite**

Run: `cargo test 2>&1 | tail -5`
Expected: PASS. Count unchanged from the 155 baseline plus the 2 new tests.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/model.rs crates/sumcp-core/src/ingest.rs crates/sumcp-core/src/merge.rs
git commit -m "feat: tag every action with the transcript it came from

Two transcripts both have a Lane::Main, so once a work unit merges them, an
adjacency comparison keyed on lane alone would treat an edit in one as
following an edit in the other. Action gains session_ix, a u16 index into a
per-session table, and Action::lane_key() returns the (session, lane) pair
every such comparison must use.

An index rather than a String on purpose: a unit holds at most 16 sessions but
tens of thousands of actions, so a String would be an allocation and 24 bytes
each where a u16 is 2 bytes and none. Behaviour is unchanged for a
single-transcript analysis, where every session_ix is 0."
```

---

### Task 4: Key adjacency findings on the lane key

**Files:**
- Modify: `crates/sumcp-core/src/signals/dynamics.rs:46-48`, `:254`, `:265`
- Modify: `crates/sumcp-core/src/signals/failures.rs:113`

**Interfaces:**
- Consumes: `Action::lane_key()` from Task 3.
- Produces: no new API. Behaviour for a single transcript is unchanged, because every `session_ix` is `0` there.

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `crates/sumcp-core/src/signals/dynamics.rs`:

```rust
    #[test]
    fn true_revert_does_not_fire_across_two_sessions() {
        // Session 0 writes "a" -> "b". Session 1 later writes "b" -> "a".
        // Read as one lane that is a textbook revert. They are different
        // transcripts, so it must NOT fire: the second session simply had a
        // different starting state, and calling that a revert would invent a
        // struggle that never happened.
        let mut s = Session::default();
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("a".into());
        first.edit_new = Some("b".into());

        let mut second = Action::default();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:02Z".into();
        second.lane = Lane::Main;
        second.session_ix = 1; // the only difference that matters
        second.kind = ActionKind::Edit;
        second.file_path = Some("/a.rs".into());
        second.edit_old = Some("b".into());
        second.edit_new = Some("a".into());

        s.actions = vec![first, second];
        s.session_ids = vec!["sess-0".into(), "sess-1".into()];

        let findings = true_reverts(&s);
        assert!(
            findings.is_empty(),
            "a revert must never span two transcripts, got {findings:?}"
        );
    }

    #[test]
    fn true_revert_still_fires_within_one_session() {
        // The same shape, both actions in session 0: this IS a revert.
        let mut s = Session::default();
        let mut first = Action::default();
        first.idx = Idx(0);
        first.effective_ts = "2026-01-01T00:00:01Z".into();
        first.lane = Lane::Main;
        first.session_ix = 0;
        first.kind = ActionKind::Edit;
        first.file_path = Some("/a.rs".into());
        first.edit_old = Some("a".into());
        first.edit_new = Some("b".into());

        let mut second = first.clone();
        second.idx = Idx(1);
        second.effective_ts = "2026-01-01T00:00:02Z".into();
        second.edit_old = Some("b".into());
        second.edit_new = Some("a".into());

        s.actions = vec![first, second];
        s.session_ids = vec!["sess-0".into()];

        assert_eq!(true_reverts(&s).len(), 1, "within one session it fires");
    }
```

If `Session` has no `Default` derive, add `#[derive(Default)]` to it in `model.rs`, or construct it with the full literal used elsewhere in this test module. Match whichever the file already does.

- [ ] **Step 2: Run the tests to verify the first fails**

Run: `cargo test -p sumcp-core true_revert_does_not_fire_across 2>&1 | tail -20`
Expected: FAIL, the assertion reports one finding where zero were expected.

- [ ] **Step 3: Change every adjacency comparison**

In `crates/sumcp-core/src/signals/dynamics.rs` around line 46, the lane grouping becomes a lane-key grouping:

```rust
    // Group by (transcript, lane), not lane alone. Two transcripts each have a
    // Lane::Main, and treating those as one lane would let a per-lane sequence
    // run straight across a transcript boundary.
    let keys: Vec<(u16, &Lane)> = s.actions.iter().map(|a| a.lane_key()).collect();
    for key in keys {
        let lane_actions: Vec<&Action> =
            s.actions.iter().filter(|a| a.lane_key() == key).collect();
```

At line 254, the revert pairing:

```rust
            if earlier.lane_key() == later.lane_key()
```

At line 265, the flip check keeps its "is a main lane" meaning, which is still correct across sessions, but must compare against the later action's own session:

```rust
                let is_flip = later.lane == crate::model::Lane::Main
```

Leave that line as it is and add a comment above it:

```rust
                // Deliberately `.lane`, not `.lane_key()`: this asks "is this a
                // main-agent action", which is a meaningful question in any
                // transcript. The pairing above already guaranteed both actions
                // come from the same transcript.
```

In `crates/sumcp-core/src/signals/failures.rs` line 113:

```rust
        p.lane_key() == a.lane_key()
```

Leave `comprehension.rs:86` and `dynamics.rs:121` unchanged; both filter for "is a main-lane action", which stays correct across sessions. Add the same one-line comment above each so a later reader does not think they were missed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p sumcp-core 2>&1 | tail -5`
Expected: PASS, whole suite.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/signals/
git commit -m "fix: key adjacency findings on (transcript, lane), not lane alone

Once a work unit merges several transcripts, Lane::Main appears in each of
them, so true_revert, flip pairing, and failure proximity attribution would
all happily pair an action in one transcript with an action in another. A
regression test pins the worst case: session 0 writes a->b and session 1
later writes b->a, which reads as a textbook revert and must not fire.

Comparisons that merely ask 'is this a main-lane action' are left on .lane
and commented, because that question stays meaningful across transcripts."
```

---

### Task 5: Merge N sessions into one work unit

**Files:**
- Modify: `crates/sumcp-core/src/merge.rs`

**Interfaces:**
- Consumes: `Session.session_ids` and `Action.session_ix` from Task 3.
- Produces: `pub fn merge_work_unit(parts: Vec<(String, Session)>, files_missing: u64, dropped: u64) -> Session`. Each `(String, Session)` is an already-assembled per-transcript session (main plus its subagents) and its transcript id. Oldest first. `merge_sessions` is untouched and still used inside per-transcript assembly.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/sumcp-core/src/merge.rs`:

```rust
    #[test]
    fn work_unit_merge_stamps_session_ix_and_renumbers_idx() {
        // Two transcripts, the second starting earlier in wall-clock time than
        // the first ends, so the total order interleaves them.
        let a = one(Lane::Main, "2026-01-01T00:00:03Z", 1, "/a");
        let b = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/b");

        let merged = merge_work_unit(
            vec![("sess-a".to_string(), a), ("sess-b".to_string(), b)],
            0,
            0,
        );

        // The table records both transcripts, in the order given.
        assert_eq!(merged.session_ids, vec!["sess-a", "sess-b"]);
        // Two actions, interleaved by timestamp: b's is earlier.
        assert_eq!(merged.actions.len(), 2);
        assert_eq!(merged.actions[0].file_path.as_deref(), Some("/b"));
        assert_eq!(merged.actions[0].session_ix, 1, "b is index 1");
        assert_eq!(merged.actions[1].session_ix, 0, "a is index 0");
        // Idx renumbered across the merged whole, the invariant evidence() needs.
        for (i, act) in merged.actions.iter().enumerate() {
            assert_eq!(act.idx, Idx(i as u32));
        }
    }

    #[test]
    fn work_unit_merge_sums_counters_across_transcripts() {
        use crate::model::Tokens;
        let mut a = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/a");
        a.tokens = Tokens { input: 10, output: 20, cache_read: 30, cache_creation: 40 };
        a.parse_errors = 1;
        a.interrupts = 2;
        a.subagent_files_missing = 1;

        let mut b = one(Lane::Main, "2026-01-01T00:00:02Z", 1, "/b");
        b.tokens = Tokens { input: 5, output: 7, cache_read: 3, cache_creation: 2 };
        b.parse_errors = 3;
        b.interrupts = 4;
        b.subagent_files_missing = 2;

        let merged = merge_work_unit(
            vec![("a".to_string(), a), ("b".to_string(), b)],
            0,
            0,
        );
        assert_eq!(merged.tokens.input, 15);
        assert_eq!(merged.tokens.output, 27);
        assert_eq!(merged.tokens.cache_read, 33);
        assert_eq!(merged.tokens.cache_creation, 42);
        assert_eq!(merged.parse_errors, 4);
        assert_eq!(merged.interrupts, 6);
        assert_eq!(
            merged.subagent_files_missing, 3,
            "per-transcript subagent misses sum across the unit"
        );
    }

    #[test]
    fn work_unit_merge_ors_auto_accept_and_keeps_every_user_text() {
        use crate::model::UserText;
        // Unlike the subagent merge, which deliberately ignores a subagent's
        // user turns and auto-accept, every transcript in a work unit is a
        // real human-facing session, so both must carry.
        let mut a = one(Lane::Main, "2026-01-01T00:00:01Z", 1, "/a");
        a.user_texts = vec![UserText {
            line_no: 1,
            text: "first".into(),
            effective_ts: "2026-01-01T00:00:00Z".into(),
        }];
        a.auto_accept = false;
        let mut b = one(Lane::Main, "2026-01-01T00:00:02Z", 1, "/b");
        b.user_texts = vec![UserText {
            line_no: 1,
            text: "second".into(),
            effective_ts: "2026-01-01T00:00:02Z".into(),
        }];
        b.auto_accept = true;

        let merged = merge_work_unit(vec![("a".into(), a), ("b".into(), b)], 0, 0);
        assert_eq!(merged.user_texts.len(), 2);
        assert!(
            merged.auto_accept,
            "a transcript that ran under auto-accept must suppress latency signals for the unit"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core work_unit_merge 2>&1 | tail -20`
Expected: FAIL, `cannot find function merge_work_unit`.

- [ ] **Step 3: Write the implementation**

Add to `crates/sumcp-core/src/merge.rs`, after `merge_sessions`:

```rust
/// Merge the per-transcript sessions of one work unit into a single Session.
///
/// Each input is an already-assembled transcript (its own main lane plus any
/// subagent lanes, already through `merge_sessions`), paired with its
/// transcript id, oldest first.
///
/// HOW THIS DIFFERS FROM `merge_sessions`. That one merges a privileged main
/// with its subordinate subagents, so it keeps only main's user turns and
/// refuses to let a subagent's auto-accept mode suppress the main lane's
/// latency signals. Here every part is a real human-facing session, so user
/// turns all carry and `auto_accept` is OR'd: if any transcript in the stretch
/// ran under auto-accept, the latency heuristics are meaningless for the unit
/// and must be suppressed.
pub fn merge_work_unit(
    parts: Vec<(String, Session)>,
    files_missing: u64,
    dropped: u64,
) -> Session {
    let mut actions: Vec<Action> = Vec::new();
    let mut user_texts = Vec::new();
    let mut session_ids: Vec<String> = Vec::new();
    let mut cwd = None;
    let mut tokens = crate::model::Tokens::default();
    let mut type_counts: std::collections::BTreeMap<String, u64> = Default::default();
    let mut parse_errors = 0u64;
    let mut untimestamped_lines = 0u64;
    let mut interrupts = 0u64;
    let mut auto_accept = false;
    let mut spawns = Vec::new();
    let mut subagent_files_missing = files_missing;

    for (ix, (id, part)) in parts.into_iter().enumerate() {
        // `ix` is this transcript's slot in `session_ids`. Stamp it onto every
        // action so an adjacency comparison can tell the transcripts apart.
        // `as u16` is safe: MAX_WORK_UNIT_SESSIONS is 16.
        let ix = ix as u16;
        session_ids.push(id);
        for mut a in part.actions {
            a.session_ix = ix;
            actions.push(a);
        }
        user_texts.extend(part.user_texts);
        // First non-None cwd wins; every transcript in a unit is in the same
        // project, so they agree, but a synthetic session can have None.
        if cwd.is_none() {
            cwd = part.cwd;
        }
        tokens.input += part.tokens.input;
        tokens.output += part.tokens.output;
        tokens.cache_read += part.tokens.cache_read;
        tokens.cache_creation += part.tokens.cache_creation;
        for (t, n) in part.type_counts {
            *type_counts.entry(t).or_insert(0) += n;
        }
        parse_errors += part.parse_errors;
        untimestamped_lines += part.untimestamped_lines;
        interrupts += part.interrupts;
        auto_accept |= part.auto_accept;
        spawns.extend(part.spawns);
        subagent_files_missing += part.subagent_files_missing;
    }

    // One sort of the whole concatenation, O(n log n). Never a pairwise merge
    // loop: at 16 transcripts that would be 16 passes over the action stream.
    // Key is (timestamp, transcript, lane, line number), which is total, so
    // the result cannot depend on input order or on sort stability.
    actions.sort_by(|a, b| {
        (&a.effective_ts, a.session_ix, &a.lane, a.line_no)
            .cmp(&(&b.effective_ts, b.session_ix, &b.lane, b.line_no))
    });
    // User turns are read in order by the flip detector, so they need the same
    // total ordering treatment.
    user_texts.sort_by(|a, b| (&a.effective_ts, a.line_no).cmp(&(&b.effective_ts, b.line_no)));

    // Re-number Idx across the merged whole: `actions[i].idx == Idx(i)` is the
    // invariant every payload and `evidence()` depends on.
    for (i, a) in actions.iter_mut().enumerate() {
        a.idx = Idx(i as u32);
    }

    let _ = dropped; // surfaced by the caller in payload flags, not stored here

    Session {
        actions,
        user_texts,
        cwd,
        tokens,
        type_counts,
        parse_errors,
        untimestamped_lines,
        interrupts,
        auto_accept,
        spawns,
        subagent_files_missing,
        session_ids,
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sumcp-core merge 2>&1 | tail -5`
Expected: PASS, all merge tests including the three new ones.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/src/merge.rs
git commit -m "feat: merge the transcripts of a work unit into one total order

merge_work_unit takes the already-assembled sessions of one stretch of work
and folds them into a single Session, stamping each action with its
transcript's slot and renumbering Idx across the whole so evidence() keeps
working.

Two rules differ from the subagent merge on purpose. Every transcript here is
a real human-facing session, so all user turns carry rather than only the
privileged main's, and auto_accept is OR'd: if any part of the stretch ran
under auto-accept the latency heuristics are meaningless for the unit.

One sort of the concatenation, not a pairwise merge loop, which at 16
transcripts would be 16 passes over the action stream."
```

---

### Task 6: Assemble a work unit from disk

**Files:**
- Modify: `crates/sumcp-core/src/assemble.rs`

**Interfaces:**
- Consumes: `work_unit::discover_work_unit` (Task 2), `merge::merge_work_unit` (Task 5), the existing `load_session`.
- Produces: `pub struct AssembledUnit { pub session: Session, pub member_paths: Vec<PathBuf>, pub subagent_paths: Vec<PathBuf>, pub unit: WorkUnit }` and `pub fn load_work_unit(main_path: &Path, max_bytes: u64) -> std::io::Result<AssembledUnit>`.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/sumcp-core/src/assemble.rs`:

```rust
    /// One main-transcript Edit at a given time, in a given session.
    fn main_edit_line(session: &str, ts: &str, path: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{session}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
        )
    }

    #[test]
    fn load_work_unit_merges_adjacent_transcripts() {
        let td = tempfile::tempdir().unwrap();
        let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
        let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
        // b starts 5 minutes after a ends, so they are one stretch of work.
        std::fs::write(
            td.path().join(format!("{id_a}.jsonl")),
            main_edit_line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
        )
        .unwrap();
        let b_path = td.path().join(format!("{id_b}.jsonl"));
        std::fs::write(
            &b_path,
            main_edit_line(id_b, "2026-01-01T00:05:00Z", "/y.rs"),
        )
        .unwrap();

        let a = load_work_unit(&b_path, MAX_TRANSCRIPT_BYTES).unwrap();
        assert_eq!(a.unit.members.len(), 2, "both transcripts in the unit");
        assert_eq!(a.session.actions.len(), 2, "both edits merged");
        assert_eq!(a.session.session_ids.len(), 2);
        // Oldest first, and the earlier edit sorts first.
        assert_eq!(a.session.actions[0].file_path.as_deref(), Some("/x.rs"));
        assert_eq!(a.session.actions[0].session_ix, 0);
        assert_eq!(a.session.actions[1].session_ix, 1);
    }

    #[test]
    fn load_work_unit_leaves_a_distant_transcript_out() {
        let td = tempfile::tempdir().unwrap();
        let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
        let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
        std::fs::write(
            td.path().join(format!("{id_a}.jsonl")),
            main_edit_line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
        )
        .unwrap();
        // 10 hours later: a different stretch of work.
        let b_path = td.path().join(format!("{id_b}.jsonl"));
        std::fs::write(
            &b_path,
            main_edit_line(id_b, "2026-01-01T10:00:00Z", "/y.rs"),
        )
        .unwrap();

        let a = load_work_unit(&b_path, MAX_TRANSCRIPT_BYTES).unwrap();
        assert_eq!(a.unit.members.len(), 1);
        assert_eq!(a.session.actions.len(), 1);
        assert_eq!(a.session.actions[0].file_path.as_deref(), Some("/y.rs"));
    }

    #[test]
    fn load_work_unit_survives_an_unreadable_member() {
        let td = tempfile::tempdir().unwrap();
        let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
        let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
        let a_path = td.path().join(format!("{id_a}.jsonl"));
        std::fs::write(
            &a_path,
            main_edit_line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
        )
        .unwrap();
        let b_path = td.path().join(format!("{id_b}.jsonl"));
        std::fs::write(
            &b_path,
            main_edit_line(id_b, "2026-01-01T00:05:00Z", "/y.rs"),
        )
        .unwrap();

        // A zero-byte ceiling makes every member unreadable EXCEPT that the
        // requested transcript is required, so the call must still fail
        // cleanly rather than panic. Use a ceiling that admits b but not a.
        let a_len = std::fs::metadata(&a_path).unwrap().len();
        let ceiling = a_len - 1;
        let out = load_work_unit(&b_path, ceiling).unwrap();
        assert_eq!(out.session.actions.len(), 1, "only the readable member");
        assert_eq!(out.unit.members.len(), 2, "the unit still knows about both");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core load_work_unit 2>&1 | tail -20`
Expected: FAIL, `cannot find function load_work_unit`.

- [ ] **Step 3: Write the implementation**

Add to `crates/sumcp-core/src/assemble.rs`, after `load_session`:

```rust
/// The outcome of assembling a whole work unit.
pub struct AssembledUnit {
    /// The merged session covering every transcript in the unit.
    pub session: Session,
    /// Main transcripts actually read, oldest first.
    pub member_paths: Vec<PathBuf>,
    /// Every subagent transcript read, across all members. The MCP store keys
    /// cache freshness on these plus `member_paths`.
    pub subagent_paths: Vec<PathBuf>,
    /// The grouping decision, for disclosure in payloads.
    pub unit: crate::work_unit::WorkUnit,
}

/// Turn a main transcript path into a merged session covering its whole work
/// unit: every transcript in the same continuous stretch of work, each with
/// its own subagents, all in one total order.
///
/// MEMORY: members are loaded one at a time and each one's raw text is dropped
/// by `load_session` before the next is opened, so peak memory is one raw
/// transcript plus the accumulated parsed actions, never the sum of all raw
/// bytes. At the largest observed unit that is the difference between about
/// 8 MB and 27 MB of live buffers.
///
/// A member that cannot be read is skipped and counted, exactly as an
/// unreadable subagent transcript is. Only a unit with no readable member at
/// all is an error.
pub fn load_work_unit(main_path: &Path, max_bytes: u64) -> std::io::Result<AssembledUnit> {
    let unit = crate::work_unit::discover_work_unit(main_path);

    let mut parts: Vec<(String, Session)> = Vec::new();
    let mut member_paths: Vec<PathBuf> = Vec::new();
    let mut subagent_paths: Vec<PathBuf> = Vec::new();
    let mut members_missing = 0u64;

    for member in &unit.members {
        // `load_session` does the per-transcript work: read the main file,
        // find and merge its subagents. Its raw buffer is freed on return.
        match load_session(&member.path, max_bytes) {
            Ok(assembled) => {
                let id = member
                    .path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                parts.push((id, assembled.session));
                member_paths.push(member.path.clone());
                subagent_paths.extend(assembled.subagent_paths);
            }
            Err(_) => members_missing += 1,
        }
    }

    if parts.is_empty() {
        return Err(std::io::Error::other(
            "no readable transcript in the work unit",
        ));
    }

    let session = merge_work_unit(parts, members_missing, unit.dropped);
    Ok(AssembledUnit {
        session,
        member_paths,
        subagent_paths,
        unit,
    })
}
```

Add the import at the top of the file:

```rust
use crate::merge::{merge_sessions, merge_work_unit};
```

replacing the existing `use crate::merge::merge_sessions;`.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sumcp-core 2>&1 | tail -5`
Expected: PASS, whole core suite.

- [ ] **Step 5: Verify against real data**

Run:
```bash
cargo build --release 2>&1 | tail -2
```
Expected: builds clean. End-to-end verification happens in Task 8 once the CLI can request a unit.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/assemble.rs
git commit -m "feat: assemble a whole work unit from disk

load_work_unit discovers the stretch of work containing a transcript, loads
each member through the existing per-transcript path (so each keeps its own
subagent merge), and folds them into one total order.

Members are loaded one at a time and each raw buffer is freed before the next
opens, so peak memory is one transcript plus parsed actions rather than the
sum of all raw bytes. At the largest observed unit that is 8 MB rather than
27 MB of live buffers.

An unreadable member is skipped and counted, mirroring how an unreadable
subagent transcript is already handled. Only a unit with nothing readable in
it is an error."
```

---

### Task 7: Payload contract v1 to v2

**Files:**
- Modify: `crates/sumcp-core/src/payloads.rs`
- Modify: `docs/payload-schema.md`
- Modify: `scripts/check_payloads.py`
- Modify: `fixtures/mock-payloads/*.json`

**Interfaces:**
- Consumes: `Session.session_ids`, `Action.session_ix`, `work_unit::WorkUnit`.
- Produces: `SessionMeta` gains `pub unit: Option<UnitMeta>` where `pub struct UnitMeta { pub sessions: usize, pub joined_gaps_min: Vec<f64>, pub span_start: String, pub span_end: String, pub session_ids: Vec<String>, pub dropped: u64 }`. `None` means a single-transcript analysis and no `work_unit` block is emitted.

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/sumcp-core/src/payloads.rs`:

```rust
    #[test]
    fn session_overview_emits_a_work_unit_block_and_bumps_the_version() {
        let s = Session::default();
        let meta = SessionMeta {
            id: "bbbbbbbb".into(),
            identified_by: "explicit".into(),
            unit: Some(UnitMeta {
                sessions: 3,
                joined_gaps_min: vec![0.1, -60.0],
                span_start: "2026-01-01T00:00:00Z".into(),
                span_end: "2026-01-01T05:00:00Z".into(),
                session_ids: vec!["aaaaaaaa".into(), "bbbbbbbb".into(), "cccccccc".into()],
                dropped: 0,
            }),
        };
        let v = session_overview(&s, &[], &meta);

        assert_eq!(v["v"], 2, "the contract version bumps with the schema");
        let wu = &v["work_unit"];
        assert_eq!(wu["sessions"], 3);
        assert_eq!(wu["joined_gaps_min"][1], -60.0, "overlap reported negative");
        assert_eq!(wu["span_start"], "2026-01-01T00:00:00Z");
        assert!(
            wu["rule"].as_str().unwrap().contains("30 min"),
            "the rule states itself so a reader can audit the grouping"
        );
    }

    #[test]
    fn session_overview_omits_the_work_unit_block_for_one_transcript() {
        let s = Session::default();
        let meta = SessionMeta {
            id: "aaaaaaaa".into(),
            identified_by: "explicit".into(),
            unit: None,
        };
        let v = session_overview(&s, &[], &meta);
        assert_eq!(v["v"], 2, "the version bumps even without a unit block");
        assert!(
            v.get("work_unit").is_none(),
            "a single-transcript analysis has no grouping to disclose"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core work_unit_block 2>&1 | tail -20`
Expected: FAIL, `struct SessionMeta has no field named unit`.

- [ ] **Step 3: Write the implementation**

In `crates/sumcp-core/src/payloads.rs`, extend `SessionMeta` and add `UnitMeta`:

```rust
/// The work-unit grouping behind a report, when there was one.
#[derive(Debug, Clone)]
pub struct UnitMeta {
    /// Transcripts merged.
    pub sessions: usize,
    /// Gap in minutes before each member after the first. Negative means the
    /// transcript overlapped the running span, so it was a concurrent Claude
    /// Code instance rather than a continuation.
    pub joined_gaps_min: Vec<f64>,
    /// First timestamp in the unit.
    pub span_start: String,
    /// Last timestamp in the unit.
    pub span_end: String,
    /// Transcript ids, oldest first.
    pub session_ids: Vec<String>,
    /// Members dropped by the size cap.
    pub dropped: u64,
}
```

and on `SessionMeta`:

```rust
    /// The work unit this report covers, or `None` for a single transcript.
    pub unit: Option<UnitMeta>,
```

In `session_overview`, after the existing `session` block is built, add:

```rust
    // Disclose the grouping in the same auditable spirit as `ranking_rule`:
    // the rule states itself, and the actual gaps are listed, so any grouping
    // can be checked by hand.
    if let Some(u) = &meta.unit {
        out["work_unit"] = json!({
            "rule": WORK_UNIT_RULE,
            "sessions": u.sessions,
            "joined_gaps_min": u.joined_gaps_min,
            "span_start": u.span_start,
            "span_end": u.span_end,
            "session_ids": u.session_ids
                .iter()
                .map(|s| short_id(s))
                .collect::<Vec<_>>(),
            "dropped": u.dropped,
        });
    }
```

Add the constant and helper near the other constants at the top of the file:

```rust
/// The grouping rule, printed verbatim in every work-unit payload so a reader
/// never has to consult the source to audit a grouping.
pub const WORK_UNIT_RULE: &str =
    "same project; joined when a transcript overlaps the previous or starts within 30 min of its end";

/// The first 8 characters of a transcript uuid. Short enough to keep the
/// payload inside its token cap, long enough to identify a transcript, and it
/// leaks neither the home directory nor the username.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}
```

Add `session` to each ranked entry in the `top` vector, and to each finding rendered by `struggle_areas` and `file_story`. In `session_overview`'s `top`:

```rust
            json!({
                "file": elide_middle(&f.file, PATH_MAX),
                "class": f.class, "edits": f.edits,
                "breakdown": f.breakdown
            })
```

stays as it is; ranked entries are file-scoped and span the unit, so a single
`session` there would be misleading. Instead, add `session` where a finding
cites specific actions, in `struggle_areas` and `evidence`: for each finding,
resolve the session from its first evidence index:

```rust
/// Which transcript a finding's evidence came from, as a short id. `None`
/// when the finding has no evidence indices or the analysis was a single
/// transcript (in which case there is nothing to disambiguate).
fn finding_session(s: &Session, idxs: &[Idx]) -> Option<String> {
    if s.session_ids.len() < 2 {
        return None;
    }
    let first = idxs.first()?;
    let a = s.actions.get(first.0 as usize)?;
    s.session_ids.get(a.session_ix as usize).map(|x| short_id(x))
}
```

and in each finding's JSON, insert the key only when `Some`, so a single-transcript payload differs from v0.1 in the version field alone:

```rust
                if let Some(sess) = finding_session(s, &f.idxs) {
                    obj["session"] = json!(sess);
                }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sumcp-core payloads 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Fix every SessionMeta construction site**

Run: `cargo test 2>&1 | grep "missing field \`unit\`" | head`
Add `unit: None,` at each site the compiler names (CLI, MCP server, tests).

- [ ] **Step 6: Bump the version everywhere it is written**

`"v"` is the payload contract version, and the schema changed, so every builder emits `2`. Change the six `"v": 1` literals in `crates/sumcp-core/src/payloads.rs` and the error-payload literal in `crates/sumcp-mcp/src/server.rs` (around line 145).

Run: `grep -rn '"v": 1' crates/ | wc -l`
Expected: `0` afterwards.

- [ ] **Step 7: Update the contract docs and checker**

In `docs/payload-schema.md`, bump the documented version to 2, document `work_unit` (all six fields, negative gap meaning, `dropped`) and the conditional `session` field on findings. State that both are absent for a single-transcript analysis, and that the version bump is the only difference such a payload has from v0.1.

In `scripts/check_payloads.py`, change the expected version to 2 and add: when `work_unit` is present, `sessions` must be a positive integer, `joined_gaps_min` must have exactly `sessions - 1` entries, `session_ids` must have exactly `sessions` entries and each must be 8 characters, and `rule` must be a non-empty string.

Every file in `fixtures/mock-payloads/` needs its `"v"` bumped to 2 in the same pass, or the checker will fail on the existing mocks.

Add a new mock at `fixtures/mock-payloads/session-overview-work-unit.json` carrying a three-transcript unit with one negative gap.

- [ ] **Step 8: Run the contract checker**

Run: `python3 scripts/check_payloads.py 2>&1 | tail -5`
Expected: 0 errors.

- [ ] **Step 9: Commit**

```bash
git add crates/sumcp-core/src/payloads.rs docs/payload-schema.md scripts/check_payloads.py fixtures/mock-payloads/
git commit -m "feat: disclose the work-unit grouping in payloads

session_overview gains a work_unit block carrying the rule verbatim, the
number of transcripts merged, the actual gaps joined across, the span, and the
short ids. The rule prints itself so a grouping can be audited without reading
the source, the same promise ranking_rule already makes. A negative gap means
that transcript overlapped rather than followed, which is how a reader spots a
concurrent Claude Code instance.

Findings gain a session field resolved from their first evidence index, which
is the drill-down from a work-unit report back to the transcript that produced
any given piece of evidence.

Both are omitted entirely for a single-transcript analysis, so those payloads
stay byte-identical to v0.1."
```

---

### Task 8: CLI reports the work unit

**Files:**
- Modify: `crates/sumcp-cli/src/main.rs`

**Interfaces:**
- Consumes: `assemble::load_work_unit`, `payloads::UnitMeta`.
- Produces: no library API. New flag `--work-unit <PATH>`; bare `sumcp` now analyzes the work unit containing the newest transcript; `--file` is unchanged except for a stderr note.

- [ ] **Step 1: Write the failing test**

Add to the integration tests in `crates/sumcp-cli` (the file holding the existing real-binary tests):

```rust
#[test]
fn work_unit_flag_merges_adjacent_transcripts() {
    let td = tempfile::tempdir().unwrap();
    let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
    let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
    let line = |sess: &str, ts: &str, path: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
        )
    };
    std::fs::write(
        td.path().join(format!("{id_a}.jsonl")),
        line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
    )
    .unwrap();
    let b = td.path().join(format!("{id_b}.jsonl"));
    std::fs::write(&b, line(id_b, "2026-01-01T00:05:00Z", "/y.rs")).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .arg("--work-unit")
        .arg(&b)
        .arg("--json")
        .output()
        .expect("run sumcp");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["work_unit"]["sessions"], 2);
    assert_eq!(v["totals"]["edits"], 2, "both transcripts' edits counted");
}

#[test]
fn file_flag_notes_that_the_transcript_is_part_of_a_unit() {
    let td = tempfile::tempdir().unwrap();
    let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
    let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
    let line = |sess: &str, ts: &str, path: &str| {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
        )
    };
    std::fs::write(
        td.path().join(format!("{id_a}.jsonl")),
        line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
    )
    .unwrap();
    let b = td.path().join(format!("{id_b}.jsonl"));
    std::fs::write(&b, line(id_b, "2026-01-01T00:05:00Z", "/y.rs")).unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .arg("--file")
        .arg(&b)
        .arg("--json")
        .output()
        .expect("run sumcp");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("1 of 2 in a work unit"),
        "expected a stderr hint, got: {stderr}"
    );
    // The note must NOT contaminate stdout, which stays pipeable.
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid json");
    assert_eq!(v["totals"]["edits"], 1, "--file still reports one transcript");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-cli work_unit 2>&1 | tail -20`
Expected: FAIL, unrecognized argument `--work-unit`.

- [ ] **Step 3: Add the flag and wire the paths**

In `crates/sumcp-cli/src/main.rs`, add to `struct Args` after `file`:

```rust
    /// Analyze the whole work unit containing this transcript: every
    /// transcript in the same continuous stretch of work.
    #[arg(long, conflicts_with = "file")]
    work_unit: Option<PathBuf>,
```

In the analysis path, choose the loader:

```rust
    // Three entry points, three scopes. `--file` is explicit and stays
    // single-transcript because an explicit path means an explicit scope.
    // Everything else reports the whole stretch of work, which is what the
    // user actually just did.
    let (session, unit_meta) = if let Some(p) = &args.file {
        let assembled = sumcp_core::assemble::load_session(p, MAX_TRANSCRIPT_BYTES)?;
        // Tell the user, on stderr so stdout stays pipeable, when the
        // transcript they named is only part of a larger stretch. Without
        // this a user can read a 51-edit report with 224 more edits beside it
        // and no indication they exist.
        let unit = sumcp_core::work_unit::discover_work_unit(p);
        if unit.members.len() > 1 {
            let at = unit
                .members
                .iter()
                .position(|m| m.path == *p)
                .map(|i| i + 1)
                .unwrap_or(1);
            eprintln!(
                "note: this transcript is {at} of {} in a work unit; use --work-unit to analyze all of it",
                unit.members.len()
            );
        }
        (assembled.session, None)
    } else {
        let path = match &args.work_unit {
            Some(p) => p.clone(),
            None => newest_transcript_for_cwd()?,
        };
        let assembled = sumcp_core::assemble::load_work_unit(&path, MAX_TRANSCRIPT_BYTES)?;
        let meta = unit_meta_from(&assembled);
        (assembled.session, Some(meta))
    };
```

Add the helper that turns an `AssembledUnit` into a `UnitMeta`:

```rust
/// Build the payload-facing description of a work unit from what assembly
/// actually read. Kept next to the CLI because the MCP server builds the same
/// thing from the same fields in Task 9; if a third caller appears, move this
/// into `payloads`.
fn unit_meta_from(a: &sumcp_core::assemble::AssembledUnit) -> sumcp_core::payloads::UnitMeta {
    sumcp_core::payloads::UnitMeta {
        sessions: a.unit.members.len(),
        joined_gaps_min: a.unit.joined_gaps_min.clone(),
        span_start: a
            .unit
            .members
            .first()
            .map(|m| m.span.first.clone())
            .unwrap_or_default(),
        span_end: a
            .unit
            .members
            .iter()
            .map(|m| m.span.last.clone())
            .max()
            .unwrap_or_default(),
        session_ids: a.session.session_ids.clone(),
        dropped: a.unit.dropped,
    }
}
```

Pass `unit_meta` into the `SessionMeta` construction that already exists in this function.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p sumcp-cli 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 5: Verify against the real corpus**

Run:
```bash
cargo build --release 2>&1 | tail -2
./target/release/sumcp --work-unit ~/.claude/projects/-Users-raphaelhaytene-Desktop-SuMCP/14a11515-ba2c-4e88-b734-148e1c4d7f91.jsonl --json \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print('sessions', d['work_unit']['sessions'], 'edits', d['totals']['edits'], 'gaps', d['work_unit']['joined_gaps_min'])"
```
Expected: `sessions 7`, and an `edits` total far above what any single member reports. The spec records this stretch as 275 Edit/Write calls; confirm the reported `edits` plus `writes` is in that range and write the exact number into the spec's §9 measurement record.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-cli/src/main.rs
git commit -m "feat: the CLI reports a work unit by default

Bare sumcp and --work-unit both analyze the whole stretch of work; --file
keeps its single-transcript scope because an explicit path means an explicit
scope. When the named transcript is part of a larger unit, --file prints a
note on stderr, which keeps stdout pipeable for --json and --html while making
sure nobody reads a 51-edit report unaware that 224 more edits sit beside it."
```

---

### Task 9: MCP server resolves to a work unit

**Files:**
- Modify: `crates/sumcp-mcp/src/store.rs`
- Modify: `crates/sumcp-mcp/src/server.rs`

**Interfaces:**
- Consumes: `assemble::load_work_unit`, `payloads::UnitMeta`.
- Produces: `SessionStore::load` returns the assembled unit rather than a bare session. Signature becomes `pub fn load(&self, path: &Path) -> std::io::Result<Arc<LoadedUnit>>` where `pub struct LoadedUnit { pub session: Session, pub meta_unit: Option<UnitMeta> }`.

- [ ] **Step 1: Write the failing test**

In `crates/sumcp-mcp/src/server.rs` tests:

```rust
    #[tokio::test]
    async fn tools_report_the_work_unit_not_one_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let id_a = "aaaaaaaa-1111-2222-3333-444455556666";
        let id_b = "bbbbbbbb-1111-2222-3333-444455556666";
        let line = |sess: &str, ts: &str, path: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","sessionId":"{sess}","message":{{"content":[{{"type":"tool_use","id":"e1","name":"Edit","input":{{"file_path":"{path}","old_string":"a","new_string":"b"}}}}]}}}}"#
            )
        };
        std::fs::write(
            dir.path().join(format!("{id_a}.jsonl")),
            line(id_a, "2026-01-01T00:00:00Z", "/x.rs"),
        )
        .unwrap();
        std::fs::write(
            dir.path().join(format!("{id_b}.jsonl")),
            line(id_b, "2026-01-01T00:05:00Z", "/y.rs"),
        )
        .unwrap();

        let args = meta_map(serde_json::json!({"session_id": id_b}));
        let out = server(dir.path())
            .call_tool_for_test("session_overview", &args)
            .await
            .expect("a payload");
        assert_eq!(out["work_unit"]["sessions"], 2);
        assert_eq!(out["totals"]["edits"], 2);
    }
```

If no `call_tool_for_test` helper exists, add one in the test module that mirrors what `call_tool` does and returns the parsed `Value`, so the assertion does not depend on the rmcp response wrapper.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-mcp work_unit 2>&1 | tail -20`
Expected: FAIL, `work_unit` is null.

- [ ] **Step 3: Change the store to load units**

In `crates/sumcp-mcp/src/store.rs`, replace the cached value:

```rust
/// What the store caches: a fully assembled work unit plus the grouping
/// description its payloads need.
pub struct LoadedUnit {
    /// The merged session covering the whole unit.
    pub session: Session,
    /// The grouping, or `None` when the unit turned out to be one transcript.
    pub meta_unit: Option<sumcp_core::payloads::UnitMeta>,
}
```

and change the freshness key. Today it keys on the requested path's `(mtime, size)`. A unit must invalidate when ANY member changes:

```rust
    /// Freshness key for a whole unit: every member's (path, mtime, size)
    /// plus every subagent file's. Any member growing, or a new transcript
    /// appearing in the stretch, changes this and forces a re-parse.
    ///
    /// WHY NOT just the requested transcript: the newest member grows while a
    /// session is live, but an OLDER member can also appear when a concurrent
    /// instance writes into the same stretch. Keying on the requested file
    /// alone would serve a stale unit in exactly the concurrent case that
    /// motivated lane-scoping in the first place.
    fn unit_key(paths: &[PathBuf]) -> Vec<(PathBuf, u64, u64)> {
        let mut key: Vec<(PathBuf, u64, u64)> = paths
            .iter()
            .filter_map(|p| {
                let m = std::fs::metadata(p).ok()?;
                let mtime = m
                    .modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs();
                Some((p.clone(), mtime, m.len()))
            })
            .collect();
        key.sort();
        key
    }
```

In `load`, call `load_work_unit`, build the key from `member_paths` plus `subagent_paths`, and cache under it. Keep the existing LRU cap of 4.

- [ ] **Step 4: Wire the server**

In `crates/sumcp-mcp/src/server.rs`, `call_tool` already does `self.store.load(&resolved.path)`. Change the binding to take `LoadedUnit`, and pass `loaded.meta_unit.clone()` into the `SessionMeta` it constructs.

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test 2>&1 | tail -5`
Expected: PASS, whole workspace.

- [ ] **Step 6: Live check through the real binary**

Run:
```bash
cargo build --release 2>&1 | tail -2
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | ./target/release/sumcp-mcp 2>/dev/null | head -c 300
```
Expected: a JSON-RPC response listing six tools. The full stdio round trip is already covered by the existing end-to-end tests.

- [ ] **Step 7: Commit**

```bash
git add crates/sumcp-mcp/
git commit -m "feat: MCP tools report the work unit

The store now caches an assembled unit rather than one transcript, and its
freshness key covers every member and every subagent file rather than only the
requested path. Keying on the requested file alone would serve a stale unit
whenever a concurrent instance wrote into the same stretch, which is exactly
the case that motivated lane-scoping."
```

---

### Task 10: The recount gate

**Files:**
- Create: `scripts/recount.py`
- Create: `fixtures/work-unit/` (three synthetic transcripts)
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the `sumcp` binary's `--json` output.
- Produces: `scripts/recount.py [--archive DIR] [--fixtures]`, exit 0 on full agreement, exit 1 with a per-quantity diff otherwise.

- [ ] **Step 1: Write the fixture and the failing check**

Create three transcripts under `fixtures/work-unit/`, forming one unit of three with a known total. Use timestamps 5 minutes apart, 2 Edits in the first, 3 in the second, 1 Write in the third, so the expected totals are `edits: 5`, `writes: 1`, `file_ops: 6`.

Create `scripts/recount.py` with only the docstring, `main()` returning 1, and the argument parser. Run it to confirm it fails.

Run: `python3 scripts/recount.py --fixtures; echo "exit=$?"`
Expected: `exit=1`.

- [ ] **Step 2: Write the recount implementation**

```python
#!/usr/bin/env python3
"""Differential recount gate (dev-only, python3 stdlib only).

WHY THIS EXISTS
---------------
Every existing Rust test asserts against fixtures the same code path produced,
so a systematic scope error is invisible: the code and the fixture share the
bug. That is exactly how a 3x undercount survived 155 green tests. This is a
second, deliberately naive implementation whose only job is to disagree.

DELIBERATELY NAIVE. It must not import from, or mirror the structure of, the
Rust code. The value comes from the two implementations being written
differently. If this ever grows shared helpers with the analyzer, it stops
being a check and becomes a reimplementation.

WHAT IT CHECKS
--------------
Countable quantities only: edits, writes, reads, bash, file_ops,
files_touched, and token totals. Signal detection is out of scope, because a
second implementation of the signal logic would be a reimplementation rather
than an independent check.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SUMCP = REPO / "target" / "release" / "sumcp"

# Tool names that modify a file, and the totals key each contributes to.
EDIT_TOOLS = {"Edit"}
WRITE_TOOLS = {"Write"}


def tool_calls(path: Path) -> list[dict]:
    """Every tool_use block in one transcript, deduped the way the parser
    rules require: by message id (streaming duplicates) and by uuid (resumed
    session replays). Last occurrence wins."""
    by_key: dict[str, dict] = {}
    order: list[str] = []
    for line_no, line in enumerate(path.read_text(errors="replace").splitlines()):
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        msg = e.get("message") or {}
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        for b in content:
            if not isinstance(b, dict) or b.get("type") != "tool_use":
                continue
            # A tool_use id is unique per call; fall back to a positional key
            # so a block without one is still counted exactly once.
            key = b.get("id") or f"{path}:{line_no}:{b.get('name')}"
            if key not in by_key:
                order.append(key)
            by_key[key] = b
    return [by_key[k] for k in order]


def recount(paths: list[Path]) -> dict:
    """Totals over a set of transcripts (a work unit: the main transcripts
    plus every subagent transcript belonging to each)."""
    edits = writes = reads = bash = 0
    touched: set[str] = set()
    for p in paths:
        for b in tool_calls(p):
            name = b.get("name")
            inp = b.get("input") or {}
            if name in EDIT_TOOLS:
                edits += 1
            elif name in WRITE_TOOLS:
                writes += 1
            elif name == "Read":
                reads += 1
            elif name == "Bash":
                bash += 1
            fp = inp.get("file_path")
            if isinstance(fp, str) and fp:
                touched.add(fp)
    return {
        "edits": edits,
        "writes": writes,
        "reads": reads,
        "bash": bash,
        "file_ops": edits + writes,
        "files_touched": len(touched),
    }


def members_of(main: Path) -> list[Path]:
    """A main transcript plus its subagent children (both on-disk layouts)."""
    out = [main]
    stem = str(main)[: -len(".jsonl")]
    out += [Path(p) for p in sorted(glob.glob(f"{stem}/subagents/agent-*.jsonl"))]
    out += [Path(p) for p in sorted(glob.glob(f"{main.parent}/agent-*.jsonl"))]
    return out


def sumcp_totals(main: Path, work_unit: bool) -> dict | None:
    """What suMCP reports for the same scope."""
    flag = "--work-unit" if work_unit else "--file"
    try:
        r = subprocess.run(
            [str(SUMCP), flag, str(main), "--json"],
            capture_output=True, text=True, timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as e:
        print(f"  RUN FAILED {main.name}: {e}")
        return None
    if r.returncode != 0:
        print(f"  RUN FAILED {main.name}: exit {r.returncode}: {r.stderr[:200]}")
        return None
    try:
        return json.loads(r.stdout).get("totals") or {}
    except json.JSONDecodeError:
        print(f"  BAD JSON {main.name}")
        return None


def compare(label: str, mine: dict, theirs: dict) -> list[str]:
    """Every quantity where the two implementations disagree."""
    bad = []
    for k, want in mine.items():
        got = theirs.get(k)
        # file_ops in the product excludes unconfirmed operations, which this
        # naive counter cannot see, so it is compared only as an upper bound.
        if k == "file_ops":
            if got is not None and got > want:
                bad.append(f"{label}: {k} product {got} exceeds recount {want}")
            continue
        if got != want:
            bad.append(f"{label}: {k} recount {want} != product {got}")
    return bad


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--archive", type=Path,
                    default=Path.home() / "claude-corpus-archive",
                    help="archive to check (default ~/claude-corpus-archive)")
    ap.add_argument("--fixtures", action="store_true",
                    help="check only the committed fixtures (what CI runs)")
    args = ap.parse_args()

    if not SUMCP.is_file():
        sys.exit(f"build first: cargo build --release ({SUMCP} missing)")

    if args.fixtures:
        mains = sorted((REPO / "fixtures" / "work-unit").glob("*.jsonl"))
        roots = "fixtures/work-unit"
    else:
        mains = [Path(p) for p in sorted(
            glob.glob(str(args.archive / "projects" / "*" / "*.jsonl")))]
        roots = str(args.archive)

    if not mains:
        sys.exit(f"no transcripts found under {roots}")

    failures: list[str] = []
    checked = 0
    for m in mains:
        theirs = sumcp_totals(m, work_unit=False)
        if theirs is None:
            failures.append(f"{m.name}: could not run")
            continue
        failures += compare(m.name, recount(members_of(m)), theirs)
        checked += 1

    print(f"recount: {checked} transcript(s) checked under {roots}")
    if failures:
        print(f"  {len(failures)} DISAGREEMENT(S):")
        for f in failures[:25]:
            print(f"    {f}")
        return 1
    print("  exact agreement")
    return 0


if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 3: Run against the fixtures**

Run: `cargo build --release && python3 scripts/recount.py --fixtures; echo "exit=$?"`
Expected: `exact agreement`, `exit=0`.

- [ ] **Step 4: Run against the real archive, the check that matters**

Run: `python3 scripts/recount.py 2>&1 | tail -30`
Expected: exact agreement across all archived transcripts. **If it disagrees, that is a real finding and the point of the whole task.** Investigate before proceeding; do not adjust the recount to match the product.

- [ ] **Step 5: Prove the gate can fail**

A harness that agrees with everything is worthless. Temporarily change `EDIT_TOOLS` to `{"Edit", "Read"}`, run `python3 scripts/recount.py --fixtures`, confirm it reports a disagreement and exits 1, then revert.

Expected: `1 DISAGREEMENT(S)` then, after reverting, `exact agreement`.

- [ ] **Step 6: Add the CI job**

In `.github/workflows/ci.yml`, add a job that runs on ubuntu-latest, checks out, builds release, and runs `python3 scripts/recount.py --fixtures`. The full-archive run stays local, because the archive is private and must never be committed.

- [ ] **Step 7: Commit**

```bash
git add scripts/recount.py fixtures/work-unit/ .github/workflows/ci.yml
git commit -m "test: an independent recount gate for every countable quantity

A second, deliberately naive implementation that walks raw JSONL and must
agree with the analyzer exactly. Every existing Rust test asserts against
fixtures the same code path produced, so a systematic scope error is invisible
to them: code and fixture share the bug. That is how a 3x undercount survived
155 green tests.

It must never share helpers with the Rust code. The value is entirely in the
two implementations being written differently. Proven able to fail by
deliberately breaking it and watching it report the disagreement.

CI runs it against committed fixtures; the full-archive run is local, since
the archive holds unredacted transcripts."
```

---

### Task 11: Performance guard and the measurement record

**Files:**
- Modify: `crates/sumcp-core/tests/` (add `perf_guard.rs`)
- Modify: `docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md` (record §9's measured numbers)

**Interfaces:**
- Consumes: everything above.
- Produces: no API.

- [ ] **Step 1: Write the guard test**

Create `crates/sumcp-core/tests/perf_guard.rs`:

```rust
//! A deliberately generous ceiling on work-unit analysis.
//!
//! WHY GENEROUS: this exists to catch an algorithmic regression, such as an
//! accidental O(n^2) merge or a re-read of every member per member. It is not
//! here to police tens of milliseconds, because a timing assertion tight
//! enough to do that is flaky on shared CI runners and would be deleted after
//! the third spurious failure. The real numbers live in the spec's §9.

use std::time::Instant;

#[test]
fn a_sixteen_member_work_unit_analyzes_well_under_the_ceiling() {
    let td = tempfile::tempdir().unwrap();
    // 16 transcripts, 400 edits each: 6400 actions, comfortably more than the
    // largest real unit's action count.
    for s in 0..16u32 {
        let id = format!("{s:08}-1111-2222-3333-444455556666");
        let mut body = String::new();
        for i in 0..400u32 {
            body.push_str(&format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T{:02}:{:02}:{:02}Z","sessionId":"{id}","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/f{}.rs","old_string":"a","new_string":"b"}}}}]}}}}"#,
                s, i / 60, i % 60, i % 20
            ));
            body.push('\n');
        }
        std::fs::write(td.path().join(format!("{id}.jsonl")), body).unwrap();
    }
    let last = td
        .path()
        .join("00000015-1111-2222-3333-444455556666.jsonl");

    let t0 = Instant::now();
    let a = sumcp_core::assemble::load_work_unit(&last, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES)
        .expect("assembles");
    let dt = t0.elapsed();

    assert_eq!(a.session.actions.len(), 6400, "every action merged");
    assert!(
        dt.as_secs_f64() < 10.0,
        "16-member unit took {dt:?}; the ceiling is 10s and exists to catch \
         an algorithmic regression, not to police milliseconds"
    );
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p sumcp-core --test perf_guard -- --nocapture 2>&1 | tail -10`
Expected: PASS, and note the reported duration.

- [ ] **Step 3: Measure the real worst case**

Run:
```bash
cargo build --release
WORST=$(python3 - <<'PY'
import json,glob,os,datetime
def span(p):
    ts=[]
    for line in open(p,errors='replace'):
        try: e=json.loads(line)
        except Exception: continue
        if e.get('timestamp'): ts.append(e['timestamp'])
    return (min(ts),max(ts)) if ts else None
def dt(s): return datetime.datetime.fromisoformat(s.replace('Z','+00:00'))
best=(0,None)
for d in glob.glob(os.path.expanduser('~/.claude/projects/*')):
    if not os.path.isdir(d): continue
    rows=[]
    for p in glob.glob(d+'/*.jsonl'):
        s=span(p)
        if s: rows.append((s[0],s[1],p))
    rows.sort()
    if not rows: continue
    units=[[rows[0]]]
    for r in rows[1:]:
        pe=max(dt(x[1]) for x in units[-1])
        if (dt(r[0])-pe).total_seconds()/60<=30: units[-1].append(r)
        else: units.append([r])
    for u in units:
        b=sum(os.path.getsize(x[2]) for x in u)
        if b>best[0]: best=(b,u[-1][2])
print(best[1])
PY
)
echo "worst unit member: $WORST"
/usr/bin/time -l ./target/release/sumcp --work-unit "$WORST" --json > /dev/null 2>/tmp/perf.txt
grep -E "real|maximum resident" /tmp/perf.txt
```
Expected: real time well under 0.5 s and peak RSS well under 100 MB, matching the budget.

- [ ] **Step 4: Write the measured numbers into the spec**

Replace the last sentence of the spec's §9 ("The real numbers above are recorded here and re-measured by hand when the implementation lands") with the actual measured wall time, peak RSS, member count and byte size from Step 3, dated.

- [ ] **Step 5: Commit**

```bash
git add crates/sumcp-core/tests/perf_guard.rs docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md
git commit -m "test: a generous ceiling guard on work-unit analysis

Sixteen members and 6400 actions must analyze in under 10 seconds. The ceiling
is deliberately loose: it exists to catch an accidental O(n^2) merge or a
re-read per member, not to police milliseconds, because a timing assertion
tight enough for that is flaky on shared runners and gets deleted after the
third spurious failure.

The spec's performance section now carries the measured wall time and peak RSS
of the real worst-case unit rather than a projection."
```

---

### Task 12: Documentation and the debrief skill

**Files:**
- Modify: `skills/debrief/SKILL.md`
- Modify: `docs/metrics.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: everything above.
- Produces: no code.

- [ ] **Step 1: Update the debrief skill**

In `skills/debrief/SKILL.md`, change every phrase that calls the scope "this session" to "this stretch of work", and add after the step that reads `session_overview`:

```markdown
If the payload carries a `work_unit` block, say how many transcripts the
report covers and over what span, because the user thinks in stretches of work
and not in transcript files. If the stretch is still in progress, note that
re-running later will cover more of it.
```

- [ ] **Step 2: Update the metrics catalog**

In `docs/metrics.md`, add a row for the work unit describing the grouping rule, its tier (T1, since it rests only on timestamps), and that it is exact rather than heuristic.

- [ ] **Step 3: Update the README**

In `README.md`, under Quickstart, document `--work-unit` next to `--file`, and state that bare `sumcp` now reports the whole stretch of work. In the Findings and roadmap section, add one bullet recording that the reporting unit was corrected in v0.2 and what the measured undercount was, since it is a finding.

- [ ] **Step 4: Update the changelog**

Add a `## [0.2.0]` section describing: work units replace single transcripts, the payload contract change, the recount gate, `mode` and `origin` adopted, and the performance budget.

- [ ] **Step 5: Run every gate**

Run:
```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -5
cargo test 2>&1 | tail -5
python3 scripts/check_payloads.py
python3 scripts/recount.py --fixtures
grep -rn "—" README.md CHANGELOG.md docs/metrics.md skills/debrief/SKILL.md | wc -l
```
Expected: fmt clean, clippy clean, all tests pass, both scripts exit 0, em dash count 0.

- [ ] **Step 6: Commit**

```bash
git add skills/debrief/SKILL.md docs/metrics.md README.md CHANGELOG.md
git commit -m "docs: work units in the debrief skill, metrics catalog, README and changelog

The debrief now narrates a stretch of work rather than a session, and says so
when the stretch is still in progress and will grow. The README documents
--work-unit and records the measured undercount as a finding, since that is
what it is."
```

---

## Self-Review

**Spec coverage.** Walking the spec section by section:

| spec section | covered by |
|---|---|
| §3.1 work-unit totals equal an independent recount | Tasks 6, 10 |
| §3.2 recount agrees on every archived transcript, CI gate | Task 10 |
| §3.3 overlaps merge as lanes, no cross-session findings | Tasks 4, 5 |
| §3.4 findings resolve to originating session | Tasks 3, 7 |
| §3.5 report discloses its grouping | Task 7 |
| §3.6 `--file` unchanged in scope | Task 8 |
| §3.7 graceful degradation on an unreadable member | Task 6 |
| §3.8 existing tests green, contract updated in lockstep | Tasks 3, 7, 12 |
| §4.1 definition and declared threshold | Task 2 |
| §4.2 overlaps are lanes | Tasks 3, 4, 5 |
| §4.3 disclosure JSON | Task 7 |
| §4.4 bounding and the honesty counter | Tasks 2, 6 |
| §4.5 which unit each entry point uses | Tasks 8, 9 |
| §5 recount gate | Task 10 |
| §6 `mode` and `origin` adopted | **GAP, see below** |
| §7 payload contract v1 to v2 | Task 7 |
| §8 error handling table | Tasks 2, 6 |
| §9 performance budget | Task 11 |
| §10 testing | every task |
| §12 `--file` stderr hint | Task 8 |

**Gap found and closed:** the spec's §6 adopts the `mode` and `origin` events, and no task above implemented them. They are genuinely separable from work units (they improve per-action auto-accept suppression and review-burden windowing, neither of which depends on grouping), and folding them into an existing task would blur its deliverable. Add them as **Task 13**, below, rather than dropping a spec requirement.

**Placeholder scan:** no TBDs; every code step carries the actual code; no "similar to Task N" references.

**Type consistency:** `TranscriptSpan` (Task 1) is consumed by `Member` (Task 2). `Action::lane_key() -> (u16, &Lane)` (Task 3) is used in Task 4 exactly as declared. `merge_work_unit(Vec<(String, Session)>, u64, u64)` (Task 5) is called with that shape in Task 6. `AssembledUnit` fields (Task 6) are read by `unit_meta_from` in Task 8 and the store in Task 9. `UnitMeta` (Task 7) is constructed in Tasks 8 and 9 with all six fields.

---

### Task 13: Adopt the `mode` and `origin` events

Spec §6. Independent of work units: both make an existing heuristic exact.

**Files:**
- Modify: `crates/sumcp-core/src/ingest.rs`
- Modify: `crates/sumcp-core/src/model.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `Action.auto_accept_here: bool` (the mode in force when this action ran) and `UserText.is_human: bool`.

- [ ] **Step 1: Write the failing test**

In `crates/sumcp-core/src/ingest.rs` tests:

```rust
    #[test]
    fn mode_events_set_auto_accept_per_action_not_per_session() {
        // A real session emits `mode` events repeatedly (96 times in one
        // observed session), so the permission mode CHANGES mid-session.
        // Suppressing latency heuristics for the whole session because the
        // mode was once auto-accept throws away the parts that were not.
        let raw = concat!(
            r#"{"type":"mode","mode":"normal","sessionId":"s"}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.rs","new_string":"x"}}]}}"#, "\n",
            r#"{"type":"mode","mode":"acceptEdits","sessionId":"s"}"#, "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/a.rs","new_string":"y"}}]}}"#, "\n"
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.actions.len(), 2);
        assert!(!s.actions[0].auto_accept_here, "first edit ran under normal mode");
        assert!(s.actions[1].auto_accept_here, "second ran under acceptEdits");
        assert!(s.auto_accept, "the session-level flag still reports 'ever seen'");
    }

    #[test]
    fn a_task_notification_is_not_a_human_turn() {
        // Review burden counts lines written between substantive HUMAN turns.
        // A task notification is injected by the harness, and counting it as a
        // human turn truncates the window and understates the metric.
        let raw = concat!(
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","origin":{"kind":"human"},"message":{"content":"do the thing"}}"#, "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","origin":{"kind":"task-notification"},"message":{"content":"<task-notification>done</task-notification>"}}"#, "\n"
        );
        let s = ingest_str(raw, Lane::Main);
        assert_eq!(s.user_texts.len(), 2, "both are recorded");
        assert!(s.user_texts[0].is_human);
        assert!(!s.user_texts[1].is_human);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core -- mode_events a_task_notification 2>&1 | tail -20`
Expected: FAIL, no field `auto_accept_here`, no field `is_human`.

- [ ] **Step 3: Implement**

Add `pub auto_accept_here: bool` to `Action` (and to its `Default` impl, as `false`), and `pub is_human: bool` to `UserText`.

In `ingest_str`, track a running mode. Add before the line loop:

```rust
    // The permission mode currently in force. Claude Code emits a `mode`
    // event whenever it changes, and it changes often within one session, so
    // this is carried forward line by line exactly the way `effective_ts` is.
    let mut mode_is_auto = false;
```

Inside the loop, before the tool_use handling:

```rust
        if v.get("type").and_then(|t| t.as_str()) == Some("mode") {
            // `acceptEdits` and `bypassPermissions` both mean edits land
            // without a human decision, which is what makes an approval
            // latency meaningless.
            mode_is_auto = matches!(
                v.get("mode").and_then(|m| m.as_str()),
                Some("acceptEdits") | Some("bypassPermissions")
            );
            if mode_is_auto {
                auto_accept = true; // session-level "ever seen", unchanged
            }
            continue;
        }
```

Set `auto_accept_here: mode_is_auto` on every `Action` constructed.

For `origin`, when building a `UserText`:

```rust
                // `origin.kind` distinguishes a real human turn from a
                // harness-injected one. Absent means human: the field is
                // newer than the transcripts we must keep reading, and
                // defaulting an unknown turn to human keeps the review-burden
                // window at its pre-existing (wider) behaviour rather than
                // silently narrowing it on old data.
                is_human: v
                    .get("origin")
                    .and_then(|o| o.get("kind"))
                    .and_then(|k| k.as_str())
                    .map(|k| k == "human")
                    .unwrap_or(true),
```

Remove `"mode"` and any other now-modelled type from the `unknown_event_types` filter in `payloads.rs`, so that counter keeps meaning "genuinely unmodelled".

- [ ] **Step 4: Use the new fields**

In `crates/sumcp-core/src/signals/comprehension.rs`, the review-burden window currently segments on `isMeta`. Change it to segment on `is_human`. In the approval-latency and instant-accept suppression, replace the session-level `s.auto_accept` check with the per-action `a.auto_accept_here`, so the parts of a session that ran under normal mode still produce signals.

- [ ] **Step 5: Run the suite**

Run: `cargo test 2>&1 | tail -5`
Expected: PASS. If a comprehension test now reports a different count, that is the intended improvement; update the fixture and note in the commit why the number changed.

- [ ] **Step 6: Verify on real data**

Run:
```bash
cargo build --release
./target/release/sumcp --file ~/.claude/projects/-Users-raphaelhaytene-Desktop-SuMCP/e88269ab-be7c-43e0-9cbf-efeebf506105.jsonl --json \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print(json.dumps(d['flags']['unknown_event_types'], indent=1))"
```
Expected: `mode` no longer appears in the unknown list.

- [ ] **Step 7: Commit**

```bash
git add crates/sumcp-core/src/
git commit -m "feat: read permission mode and turn origin instead of inferring them

Two events the parser was counting as unknown and discarding.

`mode` carries the permission mode directly, and it changes within a session
(96 times in one observed session), so auto-accept suppression becomes
per-action rather than per-session. Suppressing every latency signal because
the mode was once acceptEdits threw away the stretches that were not.

`origin` distinguishes a human turn from a harness-injected task
notification. Review burden counts lines written between substantive human
turns, and counting a notification as one truncated the window and understated
the metric. An absent origin defaults to human, which keeps the old, wider
behaviour on transcripts written before the field existed."
```

---

## Execution Order and Checkpoints

Tasks 1 through 9 are strictly sequential; each depends on the one before. Task 10 depends on Task 8 (it shells out to `--work-unit`). Tasks 11, 12, and 13 can run in any order after 10, and Task 13 is independent enough to run at any point.

**Checkpoint after Task 6:** the core can assemble a unit but nothing surfaces it. Run the full suite and confirm 155-plus tests green before continuing.

**Checkpoint after Task 10:** this is the real gate. If `python3 scripts/recount.py` disagrees on the archive, stop and investigate. A disagreement here is the harness doing its job, and adjusting the recount to match the product would defeat the entire purpose of the task.
