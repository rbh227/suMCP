# HTML Report Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild `sumcp --html` as a flat-utilitarian, review-first report per `docs/superpowers/specs/2026-07-22-html-report-redesign-design.md`, and reframe the README hero around review targeting.

**Architecture:** New pure derivations land in core (`Session.cwd` in ingest, timestamp/active-duration helpers in `report.rs`, a new `review.rs` for the needs-review qualification and plain-language sentences). `html.rs` is rewritten section by section on top of them; payload functions (`payloads::*`) stay the single source for stories/evidence/blind spots. No MCP payload changes.

**Tech Stack:** Rust (edition 2024), serde/serde_json only in core (dependency budget is frozen), plain HTML/CSS/vanilla JS inline in one generated document.

**Branch:** create `feat/report-redesign` off `main` before Task 1; merge back after Task 9.

## Global Constraints

- Dependency budget frozen: `sumcp-core` uses serde/serde_json ONLY (SPEC §5). No chrono, no regex.
- Every dynamic string in HTML goes through `esc()`; excerpts additionally through `crate::redact::redact` (ADR A9(4)).
- Generated HTML is fully self-contained: no external URL, `<script src>`, `<link>`, or `<img>` (existing test enforces).
- `render_html` stays deterministic: byte-identical output for identical input (existing test enforces).
- Six-tool MCP payload surface untouched.
- Parsing paths never panic on transcript data.
- Run `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets` must stay clean (one pre-existing warning in `assemble.rs` is not yours).
- Spec deviation, agreed at planning: the 5-minute active-gap cap is a documented `pub const` in `report.rs`, not a config field (adding it to `Weights` would pollute the ranking-transparency payload echo). The report tooltip states the cap.

---

### Task 1: `Session.cwd` capture

**Files:**
- Modify: `crates/sumcp-core/src/model.rs` (Session struct, ~line 269)
- Modify: `crates/sumcp-core/src/ingest.rs` (capture + construction, ~lines 30-60 and 310)
- Modify: `crates/sumcp-core/src/merge.rs` (construction ~line 57, test constructors ~lines 78 and 237)

**Interfaces:**
- Consumes: nothing new.
- Produces: `Session.cwd: Option<String>` — the first `cwd` string seen in the main transcript. Task 4's header uses it.

- [ ] **Step 1: Write the failing test** (append inside `mod tests` in `ingest.rs`)

```rust
#[test]
fn cwd_is_captured_from_first_line_carrying_one() {
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","cwd":"/work/proj","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","cwd":"/other","message":{"content":[{"type":"tool_use","id":"2","name":"Read","input":{"file_path":"/b.ts"}}]}}"#,
    );
    let s = ingest_str(raw, Lane::Main);
    assert_eq!(s.cwd.as_deref(), Some("/work/proj"), "first cwd wins");
    let none = ingest_str(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[]}}"#,
        Lane::Main,
    );
    assert_eq!(none.cwd, None, "no cwd line -> None");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sumcp-core cwd_is_captured -- --nocapture`
Expected: COMPILE ERROR (`no field cwd on Session`) — that is the correct failure for a missing field.

- [ ] **Step 3: Implement**

In `model.rs`, add to `Session` (after `user_texts`):

```rust
    /// The working directory recorded in the transcript (first `cwd` field
    /// seen). Used by the HTML report header; `None` for synthetic sessions.
    #[serde(default)]
    pub cwd: Option<String>,
```

In `ingest.rs`: declare `let mut cwd: Option<String> = None;` beside the other accumulators; inside the per-line loop (right after the `type_counts` update) add:

```rust
        if cwd.is_none()
            && let Some(c) = v.get("cwd").and_then(Value::as_str)
        {
            cwd = Some(c.to_string());
        }
```

Add `cwd,` to the `Session { ... }` construction at the end of `ingest_str`.

In `merge.rs`: in `merge_sessions`, the main transcript's value wins — capture `let cwd = main.cwd.clone();` before `main` is consumed (adjust to the function's existing style) and add `cwd,` to its `Session { ... }` construction. Add `cwd: None,` to the two test-helper `Session { ... }` literals.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core`
Expected: PASS (all, including the new test).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A crates/sumcp-core
git commit -m "core: capture Session.cwd from the transcript (report header)"
```

---

### Task 2: Timestamp parsing and active duration

**Files:**
- Modify: `crates/sumcp-core/src/report.rs` (append after `Overview` impl)
- Test: same file, `mod tests`

**Interfaces:**
- Consumes: `Session.actions[i].effective_ts: String` (ISO 8601, usually `...Z`).
- Produces (Task 4 header consumes all three):
  - `pub const ACTIVE_GAP_CAP_SECS: i64 = 300;`
  - `pub fn ts_secs(ts: &str) -> Option<i64>` — Unix seconds, offset-aware, `None` on malformed input.
  - `pub struct ActiveSpan { pub active_secs: i64, pub span_secs: i64 }`
  - `pub fn active_span(s: &Session, cap_secs: i64) -> Option<ActiveSpan>` — `None` when no action has a parseable timestamp.

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `report.rs`)

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p sumcp-core ts_secs -- --nocapture`
Expected: COMPILE ERROR (`ts_secs` not found).

- [ ] **Step 3: Implement** (append to `report.rs` after the `Overview` impl)

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core report::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/report.rs
git commit -m "core: ts_secs + active_span (capped active duration for the report header)"
```

---

### Task 3: `review.rs` — needs-review qualification and plain language

**Files:**
- Create: `crates/sumcp-core/src/review.rs`
- Modify: `crates/sumcp-core/src/lib.rs` (add `pub mod review;` beside the other modules)

**Interfaces:**
- Consumes: `score::FileScore { file, score, breakdown: BTreeMap<String,u64>, findings: Vec<Finding> }`, `model::{Finding, FindingKind}`.
- Produces (Tasks 5, 7, 8 consume):
  - `pub fn needs_review<'a>(ranked: &'a [FileScore], all: &[Finding]) -> Vec<&'a FileScore>` — evidence floor, capped at 3.
  - `pub fn reason_sentence(fs: &FileScore, all: &[Finding]) -> String` — e.g. `"rewritten 8x, re-read 4x, 1 failure loop"`.
  - `pub fn category_phrase(category: &str, n: u64) -> String` — the fixed vocabulary, shared with the struggle table.
  - `pub const SEVERITY_ORDER: [&str; 6]`.

- [ ] **Step 1: Write the failing tests** (in the new file's `mod tests`; the whole file is written in Step 3, so write the file WITH tests first and stub bodies `todo!()` if you prefer strict red — simplest honest red: create the file containing ONLY the tests plus `use` lines, watch the compile fail)

Test content (final form):

```rust
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

    fn fs(file: &str, findings: Vec<Finding>, breakdown: &[(&str, u64)]) -> FileScore {
        FileScore {
            file: file.into(),
            score: 1.0,
            breakdown: breakdown
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect(),
            findings,
        }
    }

    #[test]
    fn two_findings_meet_the_floor_one_does_not() {
        let yes = fs(
            "/a.rs",
            vec![
                finding(FindingKind::Churn, "/a.rs"),
                finding(FindingKind::ReRead, "/a.rs"),
            ],
            &[("churn", 4), ("re_read", 2)],
        );
        let no = fs(
            "/b.rs",
            vec![finding(FindingKind::Churn, "/b.rs")],
            &[("churn", 2)],
        );
        let ranked = vec![yes, no];
        let picked = needs_review(&ranked, &[]);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].file, "/a.rs");
    }

    #[test]
    fn one_high_signal_finding_qualifies_alone() {
        let loop_file = fs(
            "/c.rs",
            vec![finding(FindingKind::FailureLoop, "/c.rs")],
            &[("failure_loops", 3)],
        );
        let ranked = vec![loop_file];
        assert_eq!(needs_review(&ranked, &[]).len(), 1);
    }

    #[test]
    fn nonranking_flip_qualifies_via_all_findings() {
        // Flip findings never enter FileScore.findings (they don't rank), so
        // qualification must see them through `all`.
        let churn_only = fs(
            "/d.rs",
            vec![finding(FindingKind::Churn, "/d.rs")],
            &[("churn", 2)],
        );
        let all = vec![finding(FindingKind::Flip, "/d.rs")];
        let ranked = vec![churn_only];
        assert_eq!(needs_review(&ranked, &all).len(), 1);
    }

    #[test]
    fn cap_is_three() {
        let mk = |i: usize| {
            fs(
                &format!("/f{i}.rs"),
                vec![
                    finding(FindingKind::Churn, &format!("/f{i}.rs")),
                    finding(FindingKind::ReRead, &format!("/f{i}.rs")),
                ],
                &[("churn", 3), ("re_read", 2)],
            )
        };
        let ranked: Vec<FileScore> = (0..5).map(mk).collect();
        assert_eq!(needs_review(&ranked, &[]).len(), 3);
    }

    #[test]
    fn reason_sentence_uses_fixed_vocabulary_in_severity_order() {
        let f = fs(
            "/a.rs",
            vec![],
            &[("churn", 8), ("re_read", 4), ("failure_loops", 1)],
        );
        assert_eq!(
            reason_sentence(&f, &[]),
            "1 failure loop, rewritten 8x, re-read 4x"
        );
    }

    #[test]
    fn reason_sentence_appends_flip_and_user_corrected() {
        let f = fs("/a.rs", vec![], &[("churn", 2)]);
        let all = vec![
            finding(FindingKind::Flip, "/a.rs"),
            finding(FindingKind::UserCorrected, "/a.rs"),
        ];
        assert_eq!(
            reason_sentence(&f, &all),
            "rewritten 2x, flipped after pushback, user-corrected"
        );
    }

    #[test]
    fn category_phrase_pluralizes() {
        assert_eq!(category_phrase("failure_loops", 1), "1 failure loop");
        assert_eq!(category_phrase("failure_loops", 2), "2 failure loops");
        assert_eq!(category_phrase("fumbles", 1), "1 blind-write attempt");
        assert_eq!(category_phrase("churn", 3), "rewritten 3x");
        // unknown categories fall back to raw "name n" rather than panicking
        assert_eq!(category_phrase("mystery", 2), "mystery 2");
    }
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core review::`
Expected: COMPILE ERROR (functions not defined / module missing).

- [ ] **Step 3: Implement** (top of `crates/sumcp-core/src/review.rs`, above the tests)

```rust
//! Needs-review qualification and plain-language rendering (report redesign,
//! spec 2026-07-22). The evidence floor is countable and explainable in one
//! sentence — deliberately NOT a tuned score threshold (SPEC §7: never a
//! single opaque number). The vocabulary is fixed and strictly descriptive:
//! the tool never editorializes; counts always attach.

use crate::model::{Finding, FindingKind};
use crate::score::FileScore;

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
        "failure_loops" => format!("{n} failure loop{s}"),
        "fumbles" => format!("{n} blind-write attempt{s}"),
        "action_loops" => format!("{n} repeated-call loop{s}"),
        other => format!("{other} {n}"),
    }
}

/// Does `all` contain a non-ranking high-signal finding (flip or
/// user-correction) for this file? Those never enter `FileScore.findings`
/// (they don't rank), so qualification has to look at the full finding list.
fn has_flip_or_correction(all: &[Finding], file: &str) -> bool {
    all.iter().any(|f| {
        f.file.as_deref() == Some(file)
            && matches!(f.kind, FindingKind::Flip | FindingKind::UserCorrected)
    })
}

/// The evidence floor (grill decision, 2026-07-22): a file needs review when
/// it has 2+ findings, or a single high-signal one (failure loop, blind-write
/// attempt, flip, user-correction). Cap 3. Order follows `ranked`.
pub fn needs_review<'a>(ranked: &'a [FileScore], all: &[Finding]) -> Vec<&'a FileScore> {
    ranked
        .iter()
        .filter(|fs| {
            let ranked_high = fs.findings.iter().any(|f| {
                matches!(
                    f.kind,
                    FindingKind::FailureLoop | FindingKind::BlindWriteAttempt
                )
            });
            fs.findings.len() >= 2 || ranked_high || has_flip_or_correction(all, &fs.file)
        })
        .take(3)
        .collect()
}

/// One strictly descriptive sentence for a file: category phrases in severity
/// order, then flip/user-correction markers when present.
pub fn reason_sentence(fs: &FileScore, all: &[Finding]) -> String {
    let mut parts: Vec<String> = SEVERITY_ORDER
        .iter()
        .filter_map(|cat| fs.breakdown.get(*cat).map(|n| category_phrase(cat, *n)))
        .collect();
    if all
        .iter()
        .any(|f| f.file.as_deref() == Some(fs.file.as_str()) && f.kind == FindingKind::Flip)
    {
        parts.push("flipped after pushback".into());
    }
    if all.iter().any(|f| {
        f.file.as_deref() == Some(fs.file.as_str()) && f.kind == FindingKind::UserCorrected
    }) {
        parts.push("user-corrected".into());
    }
    parts.join(", ")
}
```

Add `pub mod review;` to `crates/sumcp-core/src/lib.rs`.

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p sumcp-core review::`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/review.rs crates/sumcp-core/src/lib.rs
git commit -m "core: review.rs — evidence-floor qualification + fixed plain-language vocabulary"
```

---

### Task 4: html.rs shell — CSS, header band, facts strip, status bar

**Files:**
- Modify: `crates/sumcp-core/src/html.rs` (replace `base_css`, `render_html` scaffolding, `overview_section`, `context_health_footer`; update stale tests)

**Interfaces:**
- Consumes: `report::{active_span, ACTIVE_GAP_CAP_SECS}`, `Session.cwd`, `Overview::from_session`.
- Produces (later tasks slot sections into this shell): `render_html` emits, in order: `<header class="hdr">`, `<div class="facts">`, then per-section `<section class="sec" id="...">` blocks from Tasks 5-8, then `<footer class="status">`. Helpers `fmt_thousands(n: u64) -> String` and `fmt_duration(secs: i64) -> String` are module-private and reused by every later task.

- [ ] **Step 1: Write/adjust the failing tests**

In `html.rs` `mod tests`, REPLACE `overview_precedes_struggles`, `overview_section_shows_totals`, and the context-health assertion inside `renders_blindspots_filestories_health_and_evidence` with:

```rust
#[test]
fn header_and_facts_precede_struggles() {
    let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","cwd":"/work/proj","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
    let html = render(raw);
    let hdr = html.find("class=\"hdr\"").expect("header band");
    let facts = html.find("class=\"facts\"").expect("facts strip");
    let strug = html.find("Struggle").expect("struggle section");
    assert!(hdr < facts && facts < strug, "order: header, facts, struggles");
    assert!(html.contains("/work/proj"), "project dir shown");
    assert!(html.contains("active "), "active duration shown");
}

#[test]
fn facts_strip_shows_totals_and_status_bar_replaces_context_health() {
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
    );
    let html = render(raw);
    assert!(html.contains("class=\"facts\""), "facts strip present");
    assert!(html.contains("2</b> actions"), "action count rendered");
    assert!(!html.contains("Context health"), "old footer removed");
    assert!(html.contains("class=\"status\""), "status bar present");
    assert!(html.contains("no LLM"), "trust line present");
    assert!(html.contains("parsed"), "parse trust count present");
}

#[test]
fn numbers_are_thousands_formatted() {
    assert_eq!(fmt_thousands(0), "0");
    assert_eq!(fmt_thousands(999), "999");
    assert_eq!(fmt_thousands(254_703), "254,703");
    assert_eq!(fmt_thousands(1_000_000), "1,000,000");
}

#[test]
fn durations_format_compactly() {
    assert_eq!(fmt_duration(59), "0m");
    assert_eq!(fmt_duration(60 * 47), "47m");
    assert_eq!(fmt_duration(3600 + 120), "1h 02m");
}
```

In `renders_blindspots_filestories_health_and_evidence`, change the fixture so the file qualifies for a story under the new rules (6 edits give churn only — one finding — which no longer earns a story). Prepend three reads of `/a.ts` so churn + re-read = 2 findings:

```rust
let mut lines = Vec::new();
for i in 0..3 {
    lines.push(format!(
        r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"r{i}","name":"Read","input":{{"file_path":"/a.ts"}}}}]}}}}"#
    ));
}
for i in 0..6 {
    lines.push(format!(
        r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"xxxxxxxx"}}}}]}}}}"#
    ));
}
```

and replace its `Context health` assertion with `assert!(html.contains("class=\"status\""));`.

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core html::`
Expected: FAIL/compile error on `fmt_thousands` etc.

- [ ] **Step 3: Implement**

Replace `base_css()` with the flat-utilitarian stylesheet:

```rust
/// Inline stylesheet: flat utilitarian (spec 2026-07-22). Win95's discipline
/// (one column, hard grid, grouped sections, status bar) rendered flat: white
/// page, navy accent, 1px hairlines, sharp corners, system fonts. No external
/// asset of any kind (zero-network invariant).
fn base_css() -> &'static str {
    ":root{--ink:#16161d;--navy:#000080;--red:#c41000;--mut:#6b6b76;\
      --line:#d8d8de;--soft:#f4f4fb}\
     *{box-sizing:border-box}\
     body{margin:0;background:#fff;color:var(--ink);\
       font:14px/1.45 system-ui,-apple-system,'Segoe UI',sans-serif}\
     .page{max-width:920px;margin:0 auto;padding:0 16px 32px}\
     .mono,code{font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
       font-size:13px}\
     .num{font-variant-numeric:tabular-nums}\
     .hdr{background:var(--navy);color:#fff;margin-top:16px;padding:10px 14px;\
       display:flex;justify-content:space-between;align-items:baseline;\
       gap:12px;flex-wrap:wrap}\
     .hdr .brand{font-weight:700;letter-spacing:.04em}\
     .hdr-meta{font-size:12px;color:#dfdfff;display:flex;gap:10px;\
       flex-wrap:wrap;align-items:baseline}\
     .chip{border:1px solid #dfdfff;padding:0 6px;font-size:11px;\
       text-transform:uppercase;letter-spacing:.05em}\
     .facts{display:flex;gap:20px;flex-wrap:wrap;border:1px solid var(--line);\
       border-top:none;padding:8px 14px;font-size:13px;color:var(--mut)}\
     .facts b{color:var(--ink);font-variant-numeric:tabular-nums;\
       font-weight:600}\
     .sec{margin-top:28px}\
     .sec>h2{font-size:12px;font-weight:700;letter-spacing:.08em;\
       text-transform:uppercase;color:var(--navy);margin:0 0 10px;\
       padding-bottom:4px;border-bottom:1px solid var(--ink)}\
     .calm{color:var(--mut)}\
     .nr{border:1px solid var(--line);border-left:3px solid var(--navy);\
       padding:8px 12px;margin:8px 0;display:flex;gap:12px;flex-wrap:wrap;\
       align-items:baseline;justify-content:space-between}\
     .nr .why{color:var(--mut)}\
     .nr a{color:var(--navy)}\
     .timeline{position:relative;border:1px solid var(--line);\
       padding:6px 8px 4px}\
     .track{position:absolute;left:64px;right:10px;top:0;bottom:0}\
     .lane{position:relative;height:22px;border-bottom:1px dotted var(--line)}\
     .lane:last-child{border-bottom:none}\
     .lane-lbl{position:absolute;left:4px;top:4px;color:var(--mut);\
       font-size:11px;text-transform:uppercase;letter-spacing:.05em}\
     .tick{position:absolute;top:4px;width:2px;height:14px;\
       background:var(--navy)}\
     .tick-err{background:var(--red);width:3px}\
     .band{position:absolute;top:5px;height:12px;\
       background:rgba(0,0,128,.12);border:1px solid var(--navy);\
       cursor:pointer}\
     .rules{pointer-events:none}\
     .urule{position:absolute;top:0;bottom:0;width:1px;\
       background:var(--line);pointer-events:auto}\
     .gapmark{position:absolute;top:0;bottom:0;width:0;\
       border-left:2px dashed #b6b6c2}\
     .legend{display:flex;gap:18px;flex-wrap:wrap;font-size:12px;\
       color:var(--mut);margin-top:8px;align-items:center}\
     .legend .sw{display:inline-block;vertical-align:-2px;margin-right:5px}\
     .sw-tick{width:2px;height:12px;background:var(--navy)}\
     .sw-err{width:3px;height:12px;background:var(--red)}\
     .sw-band{width:16px;height:10px;background:rgba(0,0,128,.12);\
       border:1px solid var(--navy)}\
     .sw-turn{width:1px;height:12px;background:#9a9aa4}\
     .sw-gap{width:0;height:12px;border-left:2px dashed #b6b6c2}\
     .tbl{width:100%;border-collapse:collapse}\
     .tbl th{text-align:left;font-size:11px;text-transform:uppercase;\
       letter-spacing:.06em;color:var(--mut);border-bottom:1px solid var(--ink);\
       padding:4px 8px}\
     .tbl td{padding:5px 8px;border-bottom:1px solid var(--line);\
       vertical-align:top}\
     .tbl .r{text-align:right;font-variant-numeric:tabular-nums}\
     .tbl tr.top td{background:var(--soft)}\
     .foot{font-size:12px;color:var(--mut);margin-top:6px}\
     .story-box{border:1px solid var(--line);margin:14px 0;padding:10px 14px}\
     .story-box h3{margin:0 0 2px;font-size:13px;font-weight:600}\
     .story-box .why{color:var(--mut);font-size:13px;margin:0 0 8px}\
     .story{margin:6px 0;padding-left:1.6em;font-size:13px}\
     .story li{padding:1px 0}\
     .story .fail{color:var(--red)}\
     .story .run{color:var(--mut)}\
     .exc{font-family:ui-monospace,'SF Mono',Menlo,Consolas,monospace;\
       font-size:12px;white-space:pre-wrap;word-break:break-all}\
     .tag{font-size:11px;text-transform:uppercase;letter-spacing:.05em;\
       border:1px solid var(--mut);color:var(--mut);padding:0 4px}\
     details.ev{margin:6px 0}\
     details.ev summary{cursor:pointer;color:var(--navy);font-size:13px}\
     .ev-tbl td{padding:3px 8px;border-bottom:1px solid var(--line);\
       font-size:12px;vertical-align:top}\
     .status{margin-top:32px;border-top:1px solid var(--ink);padding-top:8px;\
       font-size:12px;color:var(--mut);display:flex;gap:6px;flex-wrap:wrap}\
     .status .sep{color:var(--line)}"
}
```

Add helpers:

```rust
/// 254703 -> "254,703". Display only; payloads keep raw numbers.
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Whole-minute duration: "47m", "1h 02m". Sub-minute clamps to "0m".
fn fmt_duration(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m >= 60 {
        format!("{}h {:02}m", m / 60, m % 60)
    } else {
        format!("{m}m")
    }
}
```

Replace the `render_html` scaffolding (drop `.titlebar`/`.desktop`/`group_box` shell; keep the JS hook):

```rust
pub fn render_html(
    s: &Session,
    ranked: &[FileScore],
    weights: &Weights,
    meta: &SessionMeta,
) -> String {
    let all = crate::score::all_findings(s);
    let review = crate::review::needs_review(ranked, &all);
    let o = crate::report::Overview::from_session(s);
    let mut h = String::new();
    let _ = write!(
        h,
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>suMCP report — {id}</title><style>{css}</style></head><body>\
         <div class=\"page\">",
        id = esc(&meta.id),
        css = base_css(),
    );
    h.push_str(&header_band(s, meta));
    h.push_str(&facts_strip(&o, s));
    h.push_str(&needs_review_section(&review, &all, s));      // Task 5
    h.push_str(&timeline_section(s, ranked));                 // Task 6
    h.push_str(&struggles_section(ranked, weights, &review)); // Task 7
    h.push_str(&file_stories_section(s, &review, &all, meta));// Task 8
    h.push_str(&blind_spots_section(s, meta));                // Task 8
    h.push_str(&status_bar(s, &o));
    let _ = write!(h, "</div><script>{}</script></body></html>", inline_js());
    h
}
```

(Until Tasks 5-8 land, keep the existing `timeline_section`, `struggles_section` (adapt its signature by ignoring the extra args with `let _ =`), `file_stories_section` (temporarily pass `ranked` through by mapping `review` back to a slice — simplest: keep old body but iterate `review` instead of `ranked.iter().take(3)`), and `blind_spots_section` bodies so the crate compiles at every step.)

New sections:

```rust
/// Flat navy identity band: project, date, durations, session, mode chip.
fn header_band(s: &Session, meta: &SessionMeta) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cwd) = &s.cwd {
        parts.push(format!("<span class=\"mono\">{}</span>", esc(cwd)));
    }
    if let Some(a) = s.actions.first() {
        parts.push(esc(a.effective_ts.get(0..10).unwrap_or("")));
    }
    if let Some(d) = crate::report::active_span(s, crate::report::ACTIVE_GAP_CAP_SECS) {
        parts.push(format!(
            "<span class=\"num\" title=\"active time sums the gaps between \
             actions, each capped at 5 minutes\">active {} (span {})</span>",
            fmt_duration(d.active_secs),
            fmt_duration(d.span_secs),
        ));
    }
    let short: String = meta.id.chars().take(8).collect();
    parts.push(format!("session {}", esc(&short)));
    if s.auto_accept {
        parts.push("<span class=\"chip\">auto-accept</span>".into());
    }
    format!(
        "<header class=\"hdr\"><span class=\"brand\">suMCP</span>\
         <span class=\"hdr-meta\">{}</span></header>",
        parts.join(" · ")
    )
}

/// One aligned row of deterministic totals (replaces the Overview box).
fn facts_strip(o: &crate::report::Overview, s: &Session) -> String {
    let mut facts = vec![
        format!("<span><b>{}</b> actions</span>", fmt_thousands(o.actions as u64)),
        format!("<span><b>{}</b> files</span>", fmt_thousands(o.files_touched as u64)),
        format!("<span><b>{}</b> edits</span>", fmt_thousands(o.edits as u64)),
        format!("<span><b>{}</b> writes</span>", fmt_thousands(o.writes as u64)),
        format!("<span><b>{}</b> reads</span>", fmt_thousands(o.reads as u64)),
        format!("<span><b>{}</b> bash</span>", fmt_thousands(o.bash as u64)),
    ];
    if !s.spawns.is_empty() {
        facts.push(format!("<span><b>{}</b> subagents</span>", s.spawns.len()));
    }
    format!("<div class=\"facts\">{}</div>", facts.join(""))
}

/// Bottom trust line (replaces the Context health section).
fn status_bar(s: &Session, o: &crate::report::Overview) -> String {
    let ratio = o
        .cache_hit_ratio
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let parsed: u64 = s.type_counts.values().sum();
    let items = [
        format!("cache hit {ratio}"),
        format!("output {} tok", fmt_thousands(o.output_tokens)),
        format!(
            "parsed {} lines ({} unparsable)",
            fmt_thousands(parsed),
            fmt_thousands(o.parse_errors)
        ),
        "deterministic · no LLM".to_string(),
        format!("suMCP v{}", env!("CARGO_PKG_VERSION")),
    ];
    format!(
        "<footer class=\"status\">{}</footer>",
        items.join("<span class=\"sep\">|</span>")
    )
}
```

Delete `overview_section`, `context_health_footer`, and `group_box` once nothing references them (Tasks 5-8 remove the remaining callers; if `group_box` is still referenced at this task's end, keep it and delete in Task 8).

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p sumcp-core html::`
Expected: PASS (all html tests, including untouched structural ones).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/html.rs
git commit -m "html: flat-utilitarian shell — header band, facts strip, status bar"
```

---

### Task 5: Needs-review section

**Files:**
- Modify: `crates/sumcp-core/src/html.rs`

**Interfaces:**
- Consumes: `review::{needs_review, reason_sentence}` (Task 3), `payloads::blind_spots` (calm check).
- Produces: `fn needs_review_section(review: &[&FileScore], all: &[Finding], s: &Session) -> String`; story anchors `#story-1..3` that Task 8's boxes must carry as `id`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn needs_review_lists_qualifying_files_with_reasons_and_links() {
    // churn + re_read on /a.ts -> qualifies; reason in fixed vocabulary.
    let mut lines = Vec::new();
    for i in 0..3 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"r{i}","name":"Read","input":{{"file_path":"/a.ts"}}}}]}}}}"#
        ));
    }
    for i in 0..6 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("Needs review"), "section heading");
    assert!(html.contains("rewritten 6x"), "vocabulary reason");
    assert!(html.contains("href=\"#story-1\""), "jump link to story");
}

#[test]
fn calm_state_when_nothing_qualifies() {
    // One read, no findings at all.
    let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
    let html = render(raw);
    assert!(
        html.contains("No struggle signals"),
        "calm line shown: {}",
        &html[..html.len().min(2000)]
    );
    assert!(!html.contains("class=\"nr\""), "no review rows on calm sessions");
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core html::needs_review html::calm_state`
Expected: FAIL ("Needs review" absent).

- [ ] **Step 3: Implement**

```rust
/// The lead section: which files need eyes, and why — or an explicit calm
/// state. Reasons are strictly descriptive (grill decision 2026-07-22).
fn needs_review_section(
    review: &[&FileScore],
    all: &[crate::model::Finding],
    s: &Session,
) -> String {
    use crate::model::FindingKind;
    if review.is_empty() {
        let has_blind = all.iter().any(|f| {
            matches!(
                f.kind,
                FindingKind::BlindWriteAttempt
                    | FindingKind::ReviewBurden
                    | FindingKind::LargeWriteInstantAccept
            )
        });
        let msg = if has_blind {
            "No files met the review bar. Blind spots below still apply."
        } else {
            "No struggle signals. No blind spots."
        };
        return format!(
            "<section class=\"sec\"><h2>Needs review</h2>\
             <p class=\"calm\">{msg}</p></section>"
        );
    }
    let _ = s; // session reserved for future per-row context
    let mut rows = String::new();
    for (i, fs) in review.iter().enumerate() {
        let _ = write!(
            rows,
            "<div class=\"nr\"><span class=\"mono\">{file}</span>\
             <span class=\"why\">{why}</span>\
             <a href=\"#story-{n}\">story</a></div>",
            file = esc(&fs.file),
            why = esc(&crate::review::reason_sentence(fs, all)),
            n = i + 1,
        );
    }
    format!(
        "<section class=\"sec\"><h2>Needs review</h2>{rows}</section>"
    )
}
```

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p sumcp-core html::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/html.rs
git commit -m "html: Needs review lead section with evidence-floor rows and calm state"
```

---

### Task 6: Timeline — findings lane, legend, gap glyphs, prompt tooltips

**Files:**
- Modify: `crates/sumcp-core/src/html.rs` (`timeline_section`)

**Interfaces:**
- Consumes: `report::{ts_secs, ACTIVE_GAP_CAP_SECS}`, `redact::redact`, `Session.user_texts`.
- Produces: same `fn timeline_section(s: &Session, ranked: &[FileScore]) -> String` signature; keeps `.band`/`data-idxs` contract the inline JS and Task 8's `details.ev` rely on.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn timeline_has_legend_findings_lane_and_declared_axis() {
    let mut lines = vec![
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/a.ts"}}]}}"#.to_string(),
    ];
    for i in 0..5 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("class=\"legend\""), "legend row");
    assert!(html.contains("action sequence, not time"), "axis declared");
    assert!(html.contains("lane-findings"), "findings strip is a labeled lane");
}

#[test]
fn timeline_marks_long_gaps() {
    // Two actions 2 hours apart -> one gap glyph.
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T10:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-01-01T12:00:00Z","message":{"content":[{"type":"tool_use","id":"2","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
    );
    let html = render(raw);
    assert!(html.contains("gapmark"), "gap glyph rendered");
}

#[test]
fn user_turn_tooltips_carry_redacted_prompt_excerpts() {
    let raw = concat!(
        r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"text","text":"fix auth, my key is sk-abcdefghijklmnopqrstuv"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
    );
    let html = render(raw);
    assert!(html.contains("fix auth"), "prompt excerpt in tooltip");
    assert!(!html.contains("sk-abcdefghijklmnopqrstuv"), "secret redacted");
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core html::timeline`
Expected: the three new tests FAIL; the two existing timeline tests still pass.

- [ ] **Step 3: Implement**

Rework `timeline_section`. Keep the existing ordinal `x()` map, tick building, and band building EXACTLY as they are, then change the assembly:

```rust
    // Gap glyphs: between consecutive actions more than the active-gap cap
    // apart. Placed at the midpoint of the two ordinals.
    let mut gaps = String::new();
    for w in s.actions.windows(2) {
        if let (Some(a), Some(b)) = (
            crate::report::ts_secs(&w[0].effective_ts),
            crate::report::ts_secs(&w[1].effective_ts),
        ) && b - a > crate::report::ACTIVE_GAP_CAP_SECS
        {
            let mid = (x(w[0].idx.0 as usize) + x(w[1].idx.0 as usize)) / 2.0;
            let _ = write!(
                gaps,
                "<div class=\"gapmark\" style=\"left:{mid:.2}%\" \
                 title=\"gap over 5 minutes\"></div>"
            );
        }
    }

    // User-turn rules with a redacted prompt excerpt as tooltip (grill
    // decision 2026-07-22: sharing the file is deliberate; excerpts pass the
    // same redaction as evidence).
    let mut rules = String::new();
    for ut in &s.user_texts {
        let ord = s
            .actions
            .iter()
            .position(|a| a.line_no >= ut.line_no)
            .unwrap_or(n - 1);
        let excerpt: String = ut.text.chars().take(80).collect();
        let excerpt = crate::redact::redact(&excerpt);
        let _ = write!(
            rules,
            "<div class=\"urule\" style=\"left:{:.2}%\" title=\"{}\"></div>",
            x(ord),
            esc(excerpt.split_whitespace().collect::<Vec<_>>().join(" ").as_str()),
        );
    }

    let lane = |name: &str, label: &str, content: &str| {
        format!(
            "<div class=\"lane lane-{name}\"><span class=\"lane-lbl\">{label}</span>\
             <div class=\"track\">{content}</div></div>"
        )
    };
    let legend = "<div class=\"legend\">\
        <span><i class=\"sw sw-tick\"></i>action</span>\
        <span><i class=\"sw sw-err\"></i>error</span>\
        <span><i class=\"sw sw-band\"></i>finding span (click for evidence)</span>\
        <span><i class=\"sw sw-turn\"></i>your turn (hover for prompt)</span>\
        <span><i class=\"sw sw-gap\"></i>gap &gt; 5 min</span>\
        <span>x = action sequence, not time</span></div>";
    let caption = if other > 0 {
        format!("<p class=\"foot\">{other} actions in other tools not laned.</p>")
    } else {
        String::new()
    };
    let body = format!(
        "<div class=\"timeline\">\
         <div class=\"track rules\">{rules}{gaps}</div>\
         {findings}{read}{edit}{bash}</div>{legend}{caption}",
        findings = lane("findings", "findings", &bands),
        read = lane("read", "read", &ticks["read"]),
        edit = lane("edit", "edit", &ticks["edit"]),
        bash = lane("bash", "bash", &ticks["bash"]),
    );
    format!("<section class=\"sec\"><h2>Timeline</h2>{body}</section>")
```

(The `.band` CSS from Task 4 positions bands inside their lane's track; the old separate `.bands` strip and its CSS die here. `.rules` spans the whole timeline box so user-turn rules and gap glyphs cut vertically across all lanes.)

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p sumcp-core html::`
Expected: PASS, including `timeline_renders_lanes_ticks_and_bands`, `timeline_flags_error_ticks`, `timeline_single_action_no_divide_by_zero`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/html.rs
git commit -m "html: timeline legend, labeled findings lane, gap glyphs, redacted prompt tooltips"
```

---

### Task 7: Struggle table — plain language, cap, weights footnote

**Files:**
- Modify: `crates/sumcp-core/src/html.rs` (`struggles_section`)

**Interfaces:**
- Consumes: `review::category_phrase`, `review::SEVERITY_ORDER`, `Weights` fields (`churn`, `rework`, `failure_loop`, `re_read`, `fumble`, `action_loop`, `low_confidence_factor`, `source`).
- Produces: `fn struggles_section(ranked: &[FileScore], weights: &Weights, review: &[&FileScore]) -> String`.

- [ ] **Step 1: Write the failing tests**

Update the existing `struggles_section_lists_ranked_files_with_breakdown` assertion from `html.contains("churn")` to the vocabulary, and add the footnote/cap tests:

```rust
#[test]
fn struggle_breakdown_is_plain_language_with_weights_footnote() {
    let mut lines = Vec::new();
    for i in 0..6 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("rewritten 6x"), "plain-language breakdown");
    assert!(!html.contains("re_read"), "no internal jargon in the table");
    assert!(
        html.contains("score = weight x count"),
        "formula footnote present"
    );
    assert!(html.contains("rework 3"), "actual weights echoed");
}

#[test]
fn struggle_table_caps_at_ten_rows() {
    // 12 files, each churned twice -> 12 ranked, 10 shown, overflow note.
    let mut lines = Vec::new();
    for f in 0..12 {
        for i in 0..2 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:{f:02}:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"f{f}e{i}","name":"Edit","input":{{"file_path":"/f{f}.ts","new_string":"x"}}}}]}}}}"#
            ));
        }
    }
    let html = render(&lines.join("\n"));
    // Each data row has exactly two .r cells (rank + score); top-3 rows carry
    // class="top" so counting "<tr><td" would undercount.
    assert_eq!(
        html.matches("<td class=\"r\">").count(),
        20,
        "ten data rows expected"
    );
    assert_eq!(html.matches("class=\"top\"").count(), 3, "top-3 emphasized");
    assert!(html.contains("2 more file"), "overflow note");
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core html::struggle`
Expected: new tests FAIL.

- [ ] **Step 3: Implement**

```rust
/// Ranked files: cap 10, plain-language breakdown, top-3 emphasized and
/// linked to their stories, formula + exact weights footnoted (the
/// transparency promise in SPEC decision 6).
fn struggles_section(
    ranked: &[FileScore],
    weights: &Weights,
    review: &[&FileScore],
) -> String {
    if ranked.is_empty() {
        return format!(
            "<section class=\"sec\"><h2>Struggle areas</h2>\
             <p class=\"calm\">No struggle signals fired.</p></section>"
        );
    }
    let story_anchor = |file: &str| -> Option<usize> {
        review.iter().position(|fs| fs.file == file).map(|i| i + 1)
    };
    let mut rows = String::new();
    for (i, f) in ranked.iter().take(10).enumerate() {
        let phrases: Vec<String> = crate::review::SEVERITY_ORDER
            .iter()
            .filter_map(|cat| {
                f.breakdown
                    .get(*cat)
                    .map(|n| crate::review::category_phrase(cat, *n))
            })
            .collect();
        let file_cell = match story_anchor(&f.file) {
            Some(n) => format!(
                "<a class=\"mono\" href=\"#story-{n}\">{}</a>",
                esc(&f.file)
            ),
            None => format!("<span class=\"mono\">{}</span>", esc(&f.file)),
        };
        let _ = write!(
            rows,
            "<tr{top}><td class=\"r\">{rank}</td><td>{file_cell}</td>\
             <td class=\"r\">{score:.1}</td><td>{phrases}</td></tr>",
            top = if i < 3 { " class=\"top\"" } else { "" },
            rank = i + 1,
            score = f.score,
            phrases = esc(&phrases.join(", ")),
        );
    }
    let overflow = if ranked.len() > 10 {
        format!(
            "<p class=\"foot\">{} more file{} with minor signals (see \
             struggle_areas via the MCP tools).</p>",
            ranked.len() - 10,
            if ranked.len() - 10 == 1 { "" } else { "s" }
        )
    } else {
        String::new()
    };
    let footnote = format!(
        "<p class=\"foot\">score = weight x count, low-confidence x{lcf} · \
         weights: rewrites {c} · rework {rw} · failure loops {fl} · \
         re-reads {rr} · blind-writes {fu} · loops {al} ({src})</p>",
        lcf = weights.low_confidence_factor,
        c = weights.churn,
        rw = weights.rework,
        fl = weights.failure_loop,
        rr = weights.re_read,
        fu = weights.fumble,
        al = weights.action_loop,
        src = esc(&weights.source),
    );
    format!(
        "<section class=\"sec\"><h2>Struggle areas</h2>\
         <table class=\"tbl\"><thead><tr><th>#</th><th>file</th>\
         <th>score</th><th>signals</th></tr></thead>\
         <tbody>{rows}</tbody></table>{overflow}{footnote}</section>"
    )
}
```

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p sumcp-core html::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/html.rs
git commit -m "html: struggle table — plain language, 10-row cap, weights footnote, story links"
```

---

### Task 8: Story boxes and blind spots

**Files:**
- Modify: `crates/sumcp-core/src/html.rs` (`file_stories_section`, `file_story_section`, `blind_spots_section`; delete `group_box` when done)

**Interfaces:**
- Consumes: `payloads::{file_story, blind_spots, evidence}`, `review::reason_sentence`, `redact::redact`, `Session.actions[i].{error, command}`.
- Produces: `fn file_stories_section(s: &Session, review: &[&FileScore], all: &[Finding], meta: &SessionMeta) -> String` — boxes carry `id="story-1..N"`. `fn blind_spots_section(s: &Session, meta: &SessionMeta) -> String` — calm line or finding rows with `heuristic` tags.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn story_boxes_compress_runs_and_show_failures_inline() {
    // 3 reads then 6 identical edits -> qualifies (2 findings); the edits
    // render as one run line. A failing bash attributed to the file shows red.
    let mut lines = Vec::new();
    for i in 0..3 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"r{i}","name":"Read","input":{{"file_path":"/a.ts"}}}}]}}}}"#
        ));
    }
    for i in 0..6 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("id=\"story-1\""), "story anchor for review file");
    assert!(html.contains("class=\"run\""), "run compression rendered");
    assert!(html.contains("x6"), "run count shown");
    assert!(
        html.contains("rewritten 6x"),
        "story box opens with its why"
    );
}

#[test]
fn blind_spots_calm_line_when_clean_and_suppression_sentence() {
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","mode":"acceptEdits","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
    );
    let html = render(raw);
    assert!(
        html.contains("No blind-write attempts"),
        "calm line: {}",
        &html[html.find("Blind spots").unwrap_or(0)..html.len().min(6000)]
    );
    assert!(
        html.contains("auto-accept"),
        "suppression explained in words"
    );
    assert!(!html.contains(">suppressed<"), "bare jargon state removed");
}
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p sumcp-core html::story html::blind`
Expected: FAIL.

- [ ] **Step 3: Implement**

Run compression + story rendering:

```rust
/// One rendered story row: either a single event or a compressed run of 3+
/// consecutive same-kind, non-failing events.
enum StoryRow {
    One {
        idx: u64,
        action: String,
        outcome: String,
    },
    Run {
        action: String,
        first: u64,
        last: u64,
        n: usize,
    },
}

/// Collapse consecutive same-action, same-outcome events (never failures)
/// into runs of 3 or more. Pure; unit-testable via the rendered HTML.
fn compress_runs(events: &[serde_json::Value]) -> Vec<StoryRow> {
    let get = |v: &serde_json::Value| {
        (
            v["idx"].as_u64().unwrap_or(0),
            v["action"].as_str().unwrap_or("").to_string(),
            v["outcome"].as_str().unwrap_or("n/a").to_string(),
        )
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let (idx, action, outcome) = get(&events[i]);
        let mut j = i + 1;
        while j < events.len() && outcome != "fail" {
            let (_, a2, o2) = get(&events[j]);
            if a2 != action || o2 != outcome {
                break;
            }
            j += 1;
        }
        if j - i >= 3 {
            let (last, _, _) = get(&events[j - 1]);
            out.push(StoryRow::Run {
                action,
                first: idx,
                last,
                n: j - i,
            });
        } else {
            for k in i..j {
                let (idx, action, outcome) = get(&events[k]);
                out.push(StoryRow::One {
                    idx,
                    action,
                    outcome,
                });
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Render story rows; failing events show their redacted error excerpt.
fn story_rows_html(s: &Session, rows: &[StoryRow]) -> String {
    let mut out = String::new();
    for row in rows {
        match row {
            StoryRow::Run {
                action,
                first,
                last,
                n,
            } => {
                let _ = write!(
                    out,
                    "<li class=\"run\">{}s #{}–#{} x{}</li>",
                    esc(action),
                    first,
                    last,
                    n
                );
            }
            StoryRow::One {
                idx,
                action,
                outcome,
            } => {
                if outcome == "fail" {
                    let err = s
                        .actions
                        .get(*idx as usize)
                        .and_then(|a| a.error.as_deref())
                        .unwrap_or("");
                    let excerpt: String = err.chars().take(120).collect();
                    let _ = write!(
                        out,
                        "<li class=\"fail\">#{idx} · {} · fail \
                         <span class=\"exc\">{}</span></li>",
                        esc(action),
                        esc(&crate::redact::redact(&excerpt)),
                    );
                } else {
                    let _ = write!(
                        out,
                        "<li>#{idx} · {} · {}</li>",
                        esc(action),
                        esc(outcome)
                    );
                }
            }
        }
    }
    out
}
```

Rewrite `file_story_section` to take the box index + why sentence and emit a `.story-box` with `id="story-{n}"`; rewrite `file_stories_section` to iterate `review` (not top-3), and for each `FailureLoop` finding in `fs.findings` add a line with the repeated command:

```rust
fn file_stories_section(
    s: &Session,
    review: &[&FileScore],
    all: &[crate::model::Finding],
    meta: &SessionMeta,
) -> String {
    use crate::model::FindingKind;
    let mut out = String::new();
    for (i, fs) in review.iter().enumerate() {
        let p = crate::payloads::file_story(s, &fs.file, meta);
        let empty = Vec::new();
        let head = p["events"].as_array().unwrap_or(&empty);
        let tail = p["tail"].as_array().unwrap_or(&empty);
        let mut body = String::from("<ol class=\"story\">");
        body.push_str(&story_rows_html(s, &compress_runs(head)));
        if let Some(elided) = p.get("elided").filter(|v| !v.is_null()) {
            let _ = write!(
                body,
                "<li class=\"run\">… {} events elided …</li>",
                elided["count"].as_u64().unwrap_or(0)
            );
        }
        body.push_str(&story_rows_html(s, &compress_runs(tail)));
        body.push_str("</ol>");
        // The repeated failing command behind any failure loop, verbatim.
        for f in fs.findings.iter().filter(|f| f.kind == FindingKind::FailureLoop) {
            if let Some(cmd) = f
                .idxs
                .first()
                .and_then(|i| s.actions.get(i.0 as usize))
                .and_then(|a| a.command.as_deref())
            {
                let capped: String = cmd.chars().take(120).collect();
                let _ = write!(
                    body,
                    "<p class=\"foot\">repeated failing command: \
                     <span class=\"exc\">{}</span></p>",
                    esc(&crate::redact::redact(&capped))
                );
            }
        }
        let idxs: Vec<crate::model::Idx> = fs
            .findings
            .iter()
            .flat_map(|f| f.idxs.iter().copied())
            .collect();
        body.push_str(&evidence_details(s, &idxs, meta));
        let _ = write!(
            out,
            "<div class=\"story-box\" id=\"story-{n}\">\
             <h3 class=\"mono\">{file}</h3>\
             <p class=\"why\">score {score:.1} · {why}</p>{body}</div>",
            n = i + 1,
            file = esc(&fs.file),
            score = fs.score,
            why = esc(&crate::review::reason_sentence(fs, all)),
        );
    }
    if out.is_empty() {
        return String::new(); // calm sessions: no story section at all
    }
    format!("<section class=\"sec\"><h2>File stories</h2>{out}</section>")
}
```

Rewrite `blind_spots_section`:

```rust
/// Blind spots: calm one-liner when clean; otherwise each finding with its
/// note and a visible heuristic tag. Suppression states become sentences.
fn blind_spots_section(s: &Session, meta: &SessionMeta) -> String {
    let p = crate::payloads::blind_spots(s, meta);
    let empty = Vec::new();
    let families = [
        ("blind_write_attempts", "blind-write attempt"),
        ("review_burden", "review burden"),
        ("approval_outliers", "instant accept"),
    ];
    let mut rows = String::new();
    let mut total = 0usize;
    for (key, label) in families {
        let arr = p[key].as_array().unwrap_or(&empty);
        for f in arr.iter().take(5) {
            total += 1;
            let file = f["file"].as_str().unwrap_or("");
            let note = f["note"].as_str().unwrap_or("");
            let tag = if f["exact"].as_bool() == Some(false) {
                " <span class=\"tag\" title=\"timing-based inference, not \
                 proof\">heuristic</span>"
            } else {
                ""
            };
            let _ = write!(
                rows,
                "<li><b>{label}</b>{}{}{tag}</li>",
                if file.is_empty() {
                    String::new()
                } else {
                    format!(" · <span class=\"mono\">{}</span>", esc(file))
                },
                if note.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", esc(note))
                },
            );
        }
        if arr.len() > 5 {
            let _ = write!(rows, "<li class=\"run\">… {} more</li>", arr.len() - 5);
        }
    }
    let suppression = if p["suppression"]["approval_latency"].as_str() == Some("suppressed") {
        "<p class=\"foot\">Approval timing not measured: the session ran \
         under auto-accept, so edit-to-result deltas say nothing about \
         human attention. Review-burden is never suppressed.</p>"
    } else {
        ""
    };
    let body = if total == 0 {
        "<p class=\"calm\">No blind-write attempts, review-burden findings, \
         or instant-accept outliers.</p>"
            .to_string()
    } else {
        format!("<ul class=\"story\">{rows}</ul>")
    };
    format!(
        "<section class=\"sec\"><h2>Blind spots</h2>{body}{suppression}</section>"
    )
}
```

Delete `group_box` and the now-unused old `file_story_section` if still present.

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p sumcp-core`
Expected: PASS (whole crate).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/sumcp-core/src/html.rs
git commit -m "html: story boxes (why-first, run compression, inline failures) + worded blind spots"
```

---

### Task 9: README reframe, screenshot, final verification

**Files:**
- Modify: `README.md`
- Modify: `scripts/render_demo_report.sh` (retune capture height)
- Regenerate: `docs/assets/report-screenshot.png`

**Interfaces:**
- Consumes: the finished report from Tasks 4-8.
- Produces: the public face; no code interfaces.

- [ ] **Step 1: Rewrite the README "Why" section**

Replace the `## Why I built this` body with:

```markdown
I ship code an agent wrote faster than I can fully review it. The question
at the end of a session is not "what did we do?" but "which of this do I
actually need to look at before I trust it?"

Ask the agent and it answers from a lossy, self-flattering memory of its own
context, or it re-reads an enormous transcript. The transcript is the real
evidence: every edit, every failed command, every time I pushed back, ordered
and timestamped. suMCP reads that record deterministically, in Rust, with no
LLM and no network, and turns it into review targeting: the files the session
actually struggled with, why, and the exact actions that prove it. The tool
does not judge; it shows its work, so your limited review time goes where the
risk is.
```

- [ ] **Step 2: Demote the token chart**

Move the entire `## The numbers` section (heading, diagram, paragraph, footnote) to AFTER `## How it works`, and change its first sentence to begin: `A supporting point: the evidence arrives cheap.` Everything else in the section stays verbatim.

- [ ] **Step 3: Regenerate the screenshot**

Run: `scripts/render_demo_report.sh`
Then open the PNG. The capture should end cleanly after the Struggle areas table (hero shows: header band, facts, Needs review, Timeline, Struggle areas). Adjust `--window-size=960,<h>` in the script and re-run until the crop is clean; update the script's comment to describe the new crop target. Commit the retuned height.

- [ ] **Step 4: Full verification**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo fmt --all -- --check`
Expected: all tests pass; no NEW clippy warnings (one pre-existing in `assemble.rs`); fmt clean.

Render a real session end-to-end and eyeball it:

```bash
cargo run -p sumcp-cli --release -- --file fixtures/demo/demo-session.jsonl --html > /tmp/report.html && open /tmp/report.html
```

Check against the spec: alignment on one grid, legend present, needs-review reads in plain language, status bar replaces context health, nothing overflows horizontally.

- [ ] **Step 5: Commit and merge**

```bash
git add README.md scripts/render_demo_report.sh docs/assets/report-screenshot.png
git commit -m "readme: review-first reframe + new report hero"
git checkout main && git merge --no-ff feat/report-redesign -m "Merge feat/report-redesign: flat-utilitarian review-first HTML report + README reframe"
```
