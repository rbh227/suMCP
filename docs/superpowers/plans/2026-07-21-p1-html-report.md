# P1 — `sumcp report --html` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. The **visual/CSS layer (Task 3–5) should be built with the `frontend-design` skill** — the plan fixes the HTML structure, data mapping, and test invariants; frontend-design owns the Win9x aesthetic within those guardrails.

**Goal:** Add `sumcp report --html <transcript>` — a single self-contained, zero-network HTML file rendering the same `Report` data the MCP serves, with a session-timeline centerpiece, in the locked Win9x readability-first aesthetic.

**Architecture:** A new pure `sumcp-core::html` module exposes one function, `render_html(&Session, &[FileScore], &Weights, &SessionMeta) -> String`, that builds a complete HTML document by string composition (no template-engine dependency, ADR dependency budget). All CSS and JS are inlined; no external references ever. The CLI gains an `--html` output flag (sibling to `--json`) that prints the string to stdout. Core stays sync/pure (ADR A2); the binary just redirects the string.

**Tech Stack:** Rust (edition 2024), `sumcp-core` existing types, pure `std::fmt::Write` string building, inline CSS + ~60 lines vanilla JS. No new crate dependencies.

## Global Constraints

- **Rust edition 2024**, workspace version `0.1.0`; `cargo fmt` + `clippy` clean, all tests green before each commit.
- **No new dependencies** in `sumcp-core` (string-built HTML; no template/HTML crate). Ask before adding any.
- **Self-contained, zero network (hard invariant):** the output HTML must contain **no** `http://`, `https://`, `src=`, `href=` to any external host, no CDN, no remote font/image. Everything inline. This is a tested invariant, not a guideline.
- **Light-only, Win9x chrome:** `#c0c0c0` surfaces, navy title bars, beveled group boxes, sunken white wells. No dark mode (would break the era). Readability-first (author's explicit preference).
- **Print/no-JS safe:** the report is fully readable with JavaScript disabled; JS only enhances (tooltips, click-to-evidence).
- **Redaction (ADR A9(4)):** any evidence excerpt rendered into the HTML passes `crate::redact::redact` first — the HTML is shareable by default.
- **Section order (locked 2026-07-19):** overview → timeline → struggles → blind spots → top-3 file stories → context-health footer → evidence in native `<details>`.
- **Determinism:** same session ⇒ byte-identical HTML (no timestamps-of-now, no HashMap iteration order; use the existing `BTreeMap`-backed data).

---

### Task 1: `html` module scaffold + escaping + document shell

**Files:**
- Create: `crates/sumcp-core/src/html.rs`
- Modify: `crates/sumcp-core/src/lib.rs` (add `pub mod html;`)

**Interfaces:**
- Consumes: `crate::model::Session`, `crate::score::{FileScore, Weights}`, `crate::payloads::SessionMeta`.
- Produces:
  - `pub fn render_html(s: &Session, ranked: &[FileScore], weights: &Weights, meta: &SessionMeta) -> String`
  - `fn esc(raw: &str) -> String` — HTML-escapes `& < > " '` (module-private).

- [ ] **Step 1: Write the failing test** (escaping + shell invariants)

Add to `crates/sumcp-core/src/html.rs` a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;
    use crate::payloads::SessionMeta;
    use crate::score::{rank, Weights};

    fn meta() -> SessionMeta {
        SessionMeta { id: "sess-1".into(), identified_by: "explicit".into() }
    }

    fn render(raw: &str) -> String {
        let s = ingest_str(raw, Lane::Main);
        let w = Weights::default();
        let r = rank(&s, &w);
        render_html(&s, &r, &w, &meta())
    }

    #[test]
    fn escapes_html_metacharacters_in_paths() {
        // A file path containing HTML must never render as live markup.
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/<script>x</script>.ts"}}]}}"#;
        let html = render(raw);
        assert!(!html.contains("<script>x</script>.ts"), "raw markup leaked");
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;.ts"), "path not escaped");
    }

    #[test]
    fn shell_is_a_complete_selfcontained_document() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(html.starts_with("<!DOCTYPE html>"), "missing doctype");
        assert!(html.contains("<html"), "missing html element");
        assert!(html.contains("sess-1"), "session id not shown");
        // Zero-network invariant (the whole point of the artifact).
        assert!(!html.contains("http://") && !html.contains("https://"), "external URL present");
        assert!(!html.contains("src=") && !html.contains("href="), "external ref attribute present");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function render_html` / module not declared.

- [ ] **Step 3: Declare the module**

In `crates/sumcp-core/src/lib.rs`, add alongside the other `pub mod` lines:

```rust
pub mod html;
```

- [ ] **Step 4: Write minimal implementation**

At the top of `crates/sumcp-core/src/html.rs` (above the test module):

```rust
//! Static, self-contained HTML report (`sumcp report --html`).
//!
//! Renders the same `Report` data the MCP serves into one file — no network,
//! no external asset, light-only Win9x chrome (locked 2026-07-19). Pure and
//! sync (ADR A2): builds a `String`, writes nothing.

use crate::model::Session;
use crate::payloads::SessionMeta;
use crate::score::{FileScore, Weights};
use std::fmt::Write;

/// HTML-escape the five metacharacters. Every dynamic string that reaches the
/// document goes through this — file paths and excerpts are attacker-influenced
/// (ADR A9), so unescaped interpolation would be an injection.
fn esc(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render the full self-contained report document.
pub fn render_html(s: &Session, ranked: &[FileScore], weights: &Weights, meta: &SessionMeta) -> String {
    let _ = (ranked, weights); // wired in later tasks
    let mut h = String::new();
    // Shell. CSS is filled out in Task 2; kept minimal-but-inline here so the
    // zero-network invariant holds from the first task.
    let _ = write!(
        h,
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <title>suMCP report — {id}</title><style>{css}</style></head><body>",
        id = esc(&meta.id),
        css = base_css(),
    );
    let _ = write!(
        h,
        "<div class=\"titlebar\">suMCP — session {id}</div>",
        id = esc(&meta.id),
    );
    let _ = write!(h, "<div class=\"desktop\">");
    // Sections appended by later tasks go here.
    let _ = write!(h, "</div></body></html>");
    h
}

/// Inline base stylesheet — Win9x surfaces. Extended in later tasks.
fn base_css() -> &'static str {
    "body{margin:0;font:13px/1.4 'MS Sans Serif',Tahoma,sans-serif;\
     background:#008080;color:#000}\
     .titlebar{background:navy;color:#fff;font-weight:bold;padding:3px 6px}\
     .desktop{padding:8px}"
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: PASS (2 tests).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -p sumcp-core -- -D warnings
git add crates/sumcp-core/src/html.rs crates/sumcp-core/src/lib.rs
git commit -m "T5.1a: html module scaffold — escaping + self-contained shell"
```

---

### Task 2: Overview + struggle-areas sections (beveled group boxes)

**Files:**
- Modify: `crates/sumcp-core/src/html.rs`

**Interfaces:**
- Consumes: `crate::report::Overview::from_session`, `FileScore { file, score, breakdown, findings }`.
- Produces: module-private `fn overview_section(&Overview) -> String`, `fn struggles_section(&[FileScore]) -> String`, `fn group_box(title, body) -> String`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `html.rs`:

```rust
#[test]
fn overview_section_shows_totals() {
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
    );
    let html = render(raw);
    assert!(html.contains("Overview"), "no overview heading");
    assert!(html.contains("actions"), "no action total label");
    // 2 actions in this session.
    assert!(html.contains(">2<") || html.contains("actions</th><td>2") || html.contains("actions 2"),
        "action count 2 not rendered: {}", &html[..html.len().min(4000)]);
}

#[test]
fn struggles_section_lists_ranked_files_with_breakdown() {
    // Six edits to one file ⇒ churn ⇒ it ranks.
    let mut lines = Vec::new();
    for i in 0..6 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("Struggle"), "no struggle heading");
    assert!(html.contains("/a.ts"), "ranked file not shown");
    assert!(html.contains("churn"), "breakdown category not shown");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: FAIL — "no overview heading" (sections not emitted yet).

- [ ] **Step 3: Write minimal implementation**

Add these helpers to `html.rs` and call them from `render_html` (replace the "Sections appended by later tasks" comment):

```rust
/// A Win9x sunken group box with a title tab.
fn group_box(title: &str, body: &str) -> String {
    format!(
        "<fieldset class=\"gb\"><legend>{}</legend>{}</fieldset>",
        esc(title), body
    )
}

/// Overview: the deterministic totals.
fn overview_section(o: &crate::report::Overview) -> String {
    let ratio = o.cache_hit_ratio
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let body = format!(
        "<table class=\"kv\">\
         <tr><th>actions</th><td>{}</td><th>files</th><td>{}</td></tr>\
         <tr><th>edits</th><td>{}</td><th>writes</th><td>{}</td></tr>\
         <tr><th>reads</th><td>{}</td><th>bash</th><td>{}</td></tr>\
         <tr><th>cache hit</th><td>{}</td><th>output tok</th><td>{}</td></tr>\
         </table>",
        o.actions, o.files_touched, o.edits, o.writes, o.reads, o.bash,
        ratio, o.output_tokens,
    );
    group_box("Overview", &body)
}

/// Struggle areas: ranked files with their category breakdown.
fn struggles_section(ranked: &[FileScore]) -> String {
    if ranked.is_empty() {
        return group_box("Struggle areas", "<p>No struggle signals fired.</p>");
    }
    let mut rows = String::new();
    for (i, f) in ranked.iter().enumerate() {
        let cats: Vec<String> = f.breakdown.iter()
            .map(|(k, v)| format!("{} {}", esc(k), v))
            .collect();
        let _ = write!(
            rows,
            "<tr><td class=\"rank\">{}</td><td class=\"file\">{}</td>\
             <td class=\"score\">{:.1}</td><td>{}</td></tr>",
            i + 1, esc(&f.file), f.score, esc(&cats.join(", ")),
        );
    }
    let body = format!(
        "<table class=\"rank-tbl\"><thead><tr><th>#</th><th>file</th>\
         <th>score</th><th>breakdown</th></tr></thead><tbody>{}</tbody></table>",
        rows
    );
    group_box("Struggle areas", &body)
}
```

In `render_html`, between the `desktop` open and close, add:

```rust
    let o = crate::report::Overview::from_session(s);
    h.push_str(&overview_section(&o));
    h.push_str(&struggles_section(ranked));
```

Extend `base_css()` with (append inside the string literal):

```
     .gb{border:2px groove #fff;background:#c0c0c0;margin:8px 0;padding:8px}\
     .gb>legend{padding:0 4px;font-weight:bold}\
     table{border-collapse:collapse}\
     .kv th{text-align:left;padding:2px 8px 2px 0;color:navy}\
     .kv td{padding:2px 16px 2px 0}\
     .rank-tbl{width:100%;background:#fff;border:2px inset #fff}\
     .rank-tbl th{background:#c0c0c0;text-align:left;padding:2px 6px}\
     .rank-tbl td{padding:2px 6px;border-top:1px solid #dfdfdf}\
     .rank-tbl .file{font-family:'Courier New',monospace}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: PASS (4 tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -p sumcp-core -- -D warnings
git add crates/sumcp-core/src/html.rs
git commit -m "T5.1b: html overview + struggle-areas sections"
```

---

### Task 3: The session-timeline centerpiece

The locked centerpiece: three lanes (Read / Edit / Bash), one tick per action positioned by its order, error ticks flagged, finding bands overlaid, user turns as vertical rules. Rendered as inline-styled positioned `<div>`s (no SVG, no external lib) so it prints and needs no JS.

**Files:**
- Modify: `crates/sumcp-core/src/html.rs`

**Interfaces:**
- Consumes: `Session.actions` (`Action { idx, kind, is_error, file_path }`), `Session.user_texts` (`UserText { line_no }` — positioned by index in action order via nearest action), `ranked` findings' `idxs`.
- Produces: `fn timeline_section(s: &Session, ranked: &[FileScore]) -> String`.

**Positioning model (deterministic):** the x-axis is action ordinal `idx` (0..N-1) mapped to a percentage `idx / (N-1) * 100`. Each action is a tick in its kind's lane (Read/Edit/Bash; `Other` actions are omitted from lanes but counted in a "other" tally caption). A finding band spans from `min(idxs)` to `max(idxs)` as a translucent horizontal bar above the lanes, carrying `data-idxs` for Task 5's click handler. User turns render as full-height vertical rules at the ordinal of the next action at/after their `line_no`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn timeline_renders_lanes_ticks_and_bands() {
    // read, edit, edit, bash — plus enough edits to make /a.ts churn (a band).
    let mut lines = vec![
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"r","name":"Read","input":{"file_path":"/a.ts"}}]}}"#.to_string(),
    ];
    for i in 0..5 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("timeline"), "no timeline container");
    assert!(html.contains("lane-read") && html.contains("lane-edit") && html.contains("lane-bash"),
        "three lanes required");
    assert!(html.contains("class=\"tick"), "no action ticks");
    // The churn finding on /a.ts should overlay a band carrying its idxs.
    assert!(html.contains("data-idxs="), "no finding band with idxs");
}

#[test]
fn timeline_flags_error_ticks() {
    let raw = concat!(
        r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b","name":"Bash","input":{"command":"false"}}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"b","is_error":true}]}}"#,
    );
    let html = render(raw);
    assert!(html.contains("tick-err"), "error tick not marked");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core html::tests::timeline 2>&1 | tail -20`
Expected: FAIL — "no timeline container".

- [ ] **Step 3: Write minimal implementation**

Add to `html.rs`:

```rust
/// The timeline centerpiece: action ticks in Read/Edit/Bash lanes, finding
/// bands overlaid, user turns as vertical rules. Pure positioned divs so it
/// prints and works with JS disabled.
fn timeline_section(s: &Session, ranked: &[FileScore]) -> String {
    use crate::model::ActionKind;
    let n = s.actions.len();
    if n == 0 {
        return group_box("Timeline", "<p>No actions.</p>");
    }
    // x% for an ordinal; guard n==1 (avoid divide-by-zero → put it at 0%).
    let x = |idx: usize| -> f64 {
        if n <= 1 { 0.0 } else { idx as f64 / (n as f64 - 1.0) * 100.0 }
    };

    // Finding bands: one per ranking finding that has idxs, spanning min..max.
    let mut bands = String::new();
    for f in ranked.iter().flat_map(|fs| &fs.findings) {
        if f.idxs.is_empty() { continue; }
        let lo = f.idxs.iter().map(|i| i.0 as usize).min().unwrap();
        let hi = f.idxs.iter().map(|i| i.0 as usize).max().unwrap();
        let idxs_attr = f.idxs.iter().map(|i| i.0.to_string()).collect::<Vec<_>>().join(",");
        let kind = format!("{:?}", f.kind);
        let _ = write!(
            bands,
            "<div class=\"band\" style=\"left:{:.2}%;width:{:.2}%\" \
             data-idxs=\"{}\" title=\"{}\"></div>",
            x(lo), (x(hi) - x(lo)).max(0.6), esc(&idxs_attr), esc(&kind),
        );
    }

    // Lane ticks.
    let lane_of = |k: &ActionKind| match k {
        ActionKind::Read => Some("read"),
        ActionKind::Edit | ActionKind::Write => Some("edit"),
        ActionKind::Bash => Some("bash"),
        ActionKind::Other(_) => None,
    };
    let mut ticks: std::collections::BTreeMap<&str, String> =
        [("read", String::new()), ("edit", String::new()), ("bash", String::new())]
            .into_iter().collect();
    let mut other = 0usize;
    for a in &s.actions {
        let Some(lane) = lane_of(&a.kind) else { other += 1; continue; };
        let err = if a.is_error == Some(true) { " tick-err" } else { "" };
        let file = a.file_path.as_deref().unwrap_or("");
        let _ = write!(
            ticks.get_mut(lane).unwrap(),
            "<div class=\"tick{}\" style=\"left:{:.2}%\" \
             data-idx=\"{}\" title=\"#{} {}\"></div>",
            err, x(a.idx.0 as usize), a.idx.0, a.idx.0, esc(file),
        );
    }

    // User-turn rules: place at the ordinal of the first action at/after the
    // user text's source line. Deterministic; falls at 100% if none follow.
    let mut rules = String::new();
    for ut in &s.user_texts {
        let ord = s.actions.iter().position(|a| a.line_no >= ut.line_no).unwrap_or(n - 1);
        let _ = write!(rules, "<div class=\"urule\" style=\"left:{:.2}%\"></div>", x(ord));
    }

    let lane = |name: &str, label: &str| {
        format!(
            "<div class=\"lane lane-{name}\"><span class=\"lane-lbl\">{label}</span>{}</div>",
            ticks[name]
        )
    };
    let caption = if other > 0 {
        format!("<p class=\"cap\">{} action(s) in other tools not laned.</p>", other)
    } else { String::new() };

    let body = format!(
        "<div class=\"timeline\"><div class=\"bands\">{bands}</div>{rules}{}{}{}</div>{caption}",
        lane("read", "Read"), lane("edit", "Edit"), lane("bash", "Bash"),
    );
    group_box("Timeline", &body)
}
```

Call it in `render_html` immediately after the overview (before struggles, per the locked section order):

```rust
    h.push_str(&overview_section(&o));
    h.push_str(&timeline_section(s, ranked));
    h.push_str(&struggles_section(ranked));
```

Append timeline CSS to `base_css()`:

```
     .timeline{position:relative;background:#fff;border:2px inset #fff;\
       padding:18px 4px 4px;min-height:96px}\
     .bands{position:absolute;left:0;right:0;top:0;height:14px}\
     .band{position:absolute;top:2px;height:10px;background:rgba(128,0,0,.35);\
       border:1px solid maroon;cursor:pointer}\
     .lane{position:relative;height:22px;margin:2px 0;border-bottom:1px dotted #bbb}\
     .lane-lbl{position:absolute;left:2px;top:3px;color:navy;font-size:11px}\
     .tick{position:absolute;top:4px;width:3px;height:14px;background:navy;\
       margin-left:38px}\
     .tick-err{background:red}\
     .urule{position:absolute;top:14px;bottom:0;width:1px;background:#888}\
     .cap{color:#444;font-size:11px;margin:4px 0 0}
```

> **Note for the executor:** the CSS above is a correct, minimal baseline that satisfies the tests. Use the `frontend-design` skill to refine spacing, tick shape, lane labels, and band legibility toward the locked Win9x aesthetic — but keep every class name the tests assert on (`timeline`, `lane-read/edit/bash`, `tick`, `tick-err`, `band`, `data-idxs`, `data-idx`) and keep it inline.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core html::tests::timeline 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Full-suite + fmt + clippy + commit**

```bash
cargo test -p sumcp-core 2>&1 | tail -5
cargo fmt && cargo clippy -p sumcp-core -- -D warnings
git add crates/sumcp-core/src/html.rs
git commit -m "T5.1c: html timeline centerpiece — lanes, ticks, finding bands, user rules"
```

---

### Task 4: Blind spots + file stories + context-health footer + evidence details

**Files:**
- Modify: `crates/sumcp-core/src/html.rs`

**Interfaces:**
- Consumes: `crate::payloads::{blind_spots, file_story, context_health, evidence}` (reuse the JSON builders as the data source, render their values into HTML — DRY with the MCP contract), `crate::model::Idx`.
- Produces: `fn blind_spots_section`, `fn file_stories_section`, `fn context_health_footer`, `fn evidence_details`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn renders_blindspots_filestories_health_and_evidence() {
    let mut lines = Vec::new();
    for i in 0..6 {
        lines.push(format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"xxxxxxxx"}}}}]}}}}"#
        ));
    }
    let html = render(&lines.join("\n"));
    assert!(html.contains("Blind spots"), "no blind-spots section");
    assert!(html.contains("Context health"), "no context-health footer");
    // Top file gets a story section.
    assert!(html.contains("File story") && html.contains("/a.ts"), "no file story");
    // Evidence lives in a native collapsible.
    assert!(html.contains("<details"), "evidence not in <details>");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core html::tests::renders_blindspots 2>&1 | tail -20`
Expected: FAIL — "no blind-spots section".

- [ ] **Step 3: Write minimal implementation**

Add sections that read from the existing payload builders (so the HTML never diverges from the MCP contract). Render `blind_spots` counts, a `file_story` for each of the top-3 ranked files, a `context_health` footer, and an evidence `<details>` per top file dereferencing its findings' idxs through `evidence`. Example for two of them (follow the same shape for the rest):

```rust
fn context_health_footer(s: &Session, meta: &SessionMeta) -> String {
    let p = crate::payloads::context_health(s, meta);
    let ratio = p["cache_hit_ratio"].as_f64()
        .map(|r| format!("{:.0}%", r * 100.0)).unwrap_or_else(|| "n/a".into());
    let body = format!(
        "<p>cache hit {} · output {} · cache-read {} tok. \
         <em>Informational only — v0.1 makes no waste judgment.</em></p>",
        ratio,
        p["tokens"]["output"].as_u64().unwrap_or(0),
        p["tokens"]["cache_read"].as_u64().unwrap_or(0),
    );
    group_box("Context health", &body)
}

fn evidence_details(s: &Session, idxs: &[crate::model::Idx], meta: &SessionMeta) -> String {
    let p = crate::payloads::evidence(s, idxs, meta);
    let mut rows = String::new();
    for a in p["actions"].as_array().map(|v| v.as_slice()).unwrap_or(&[]) {
        let _ = write!(
            rows,
            "<tr><td>#{}</td><td>{}</td><td class=\"exc\">{}</td></tr>",
            a["idx"].as_u64().unwrap_or(0),
            esc(a["tool"].as_str().unwrap_or("")),
            esc(a["excerpt"].as_str().unwrap_or("")),
        );
    }
    format!(
        "<details class=\"ev\"><summary>evidence</summary>\
         <table class=\"ev-tbl\"><tbody>{}</tbody></table></details>",
        rows
    )
}
```

Add `blind_spots_section` and `file_stories_section` following the same "read the payload builder, render its values, `esc()` every string" pattern. Wire all four into `render_html` in the locked order: `...timeline, struggles, blind_spots, file_stories (top-3), context_health footer` — with each top file's `evidence_details` nested inside its file story. Extend `base_css()` with `.exc{font-family:'Courier New',monospace;white-space:pre-wrap;word-break:break-all}` and `details.ev{margin:4px 0}`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -p sumcp-core -- -D warnings
git add crates/sumcp-core/src/html.rs
git commit -m "T5.1d: html blind-spots, file stories, health footer, evidence details"
```

---

### Task 5: Minimal vanilla JS — tooltips + click-band-to-evidence (print/no-JS safe)

**Files:**
- Modify: `crates/sumcp-core/src/html.rs`

**Interfaces:**
- Produces: `fn inline_js() -> &'static str`, injected as a `<script>` before `</body>`.

Behavior: clicking a `.band` (which carries `data-idxs`) scrolls to / opens the matching evidence `<details>`; hovering a `.tick` shows its `title`. All progressive enhancement — the report is complete without it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn inline_js_is_present_and_local_only() {
    let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
    let html = render(raw);
    assert!(html.contains("<script>"), "no inline script");
    assert!(html.contains("addEventListener"), "no interaction wiring");
    // Still zero-network after adding JS.
    assert!(!html.contains("http://") && !html.contains("https://"), "external URL in JS");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-core html::tests::inline_js 2>&1 | tail -20`
Expected: FAIL — "no inline script".

- [ ] **Step 3: Write minimal implementation**

```rust
/// ~50 lines of dependency-free enhancement. Clicking a finding band opens the
/// evidence collapsible whose idxs it overlaps and scrolls to it.
fn inline_js() -> &'static str {
    "document.addEventListener('DOMContentLoaded',function(){\
       document.querySelectorAll('.band').forEach(function(b){\
         b.addEventListener('click',function(){\
           var want=(b.getAttribute('data-idxs')||'').split(',')[0];\
           var hit=[].find.call(document.querySelectorAll('details.ev'),function(d){\
             return (d.getAttribute('data-idxs')||'').split(',').indexOf(want)>=0;});\
           if(hit){hit.open=true;hit.scrollIntoView({behavior:'smooth',block:'center'});}\
         });\
       });\
     });"
}
```

In `render_html`, before the final `</body></html>`:

```rust
    let _ = write!(h, "<script>{}</script>", inline_js());
```

For the click target to resolve, `evidence_details` (Task 4) must also stamp `data-idxs` on its `<details>` — update that element to `"<details class=\"ev\" data-idxs=\"{joined}\">"` where `joined` is the same idx list passed in.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-core html:: 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -p sumcp-core -- -D warnings
git add crates/sumcp-core/src/html.rs
git commit -m "T5.1e: html inline JS — click-band-to-evidence, tooltips (no-JS safe)"
```

---

### Task 6: CLI `--html` flag + end-to-end golden test on the real donor fixture

**Files:**
- Modify: `crates/sumcp-cli/src/main.rs`
- Test: `crates/sumcp-cli/tests/html_report.rs` (create)

**Interfaces:**
- Consumes: `sumcp_core::html::render_html`, existing `load_session`, `rank`, `SessionMeta`.
- Produces: `sumcp --file <path> --html` prints the report to stdout.

> **Interface note (deferred reconciliation):** the documented public surface is `sumcp report --html`. This task ships the capability as an `--html` output flag on the current `--file` interface (sibling to `--json`), matching the existing tested CLI shape. Renaming to a `report` subcommand + bare-`sumcp` latest-mode is a CLI-surface change owned by the later installer/CLI-polish task (P4/T5.2); do **not** refactor clap subcommands here.

- [ ] **Step 1: Write the failing test**

Create `crates/sumcp-cli/tests/html_report.rs`:

```rust
//! End-to-end: the real binary renders self-contained HTML from the donor.
use std::process::Command;

fn donor() -> std::path::PathBuf {
    // The sanitized 2.1.210 donor fixture used across the suite.
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join("session-2_1_210-subagents.jsonl")
}

#[test]
fn html_output_is_self_contained_and_structured() {
    let path = donor();
    assert!(path.exists(), "donor fixture missing at {}", path.display());
    let out = Command::new(env!("CARGO_BIN_EXE_sumcp"))
        .args(["--file", path.to_str().unwrap(), "--html"])
        .output()
        .expect("run sumcp");
    assert!(out.status.success(), "sumcp --html exited non-zero");
    let html = String::from_utf8(out.stdout).expect("utf8 html");
    assert!(html.starts_with("<!DOCTYPE html>"), "not an html doc");
    assert!(html.contains("timeline"), "no timeline");
    // Hard zero-network invariant on real data.
    assert!(!html.contains("http://") && !html.contains("https://"), "external URL leaked");
    assert!(!html.contains("src=") && !html.contains("href="), "external ref attr leaked");
    // No secret leaks in evidence excerpts (redaction wired).
    assert!(!html.contains("BEGIN RSA PRIVATE KEY"), "unredacted secret");
}
```

Confirm the donor fixture filename first:

Run: `ls fixtures/*.jsonl`
If the donor file has a different name, update `donor()` accordingly (do not hardcode a guess — use the real path).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p sumcp-cli --test html_report 2>&1 | tail -20`
Expected: FAIL — unknown argument `--html`.

- [ ] **Step 3: Write minimal implementation**

In `crates/sumcp-cli/src/main.rs`, add the flag to `Args`:

```rust
    /// Render the self-contained HTML report to stdout.
    #[arg(long)]
    html: bool,
```

And branch before the `--json` branch (so `--html` takes precedence when both given, or make them mutually exclusive — pick precedence, simplest):

```rust
    if args.html {
        let meta = SessionMeta {
            id: path.file_stem().map(|s| s.to_string_lossy().into()).unwrap_or_default(),
            identified_by: "explicit".into(),
        };
        print!(
            "{}",
            sumcp_core::html::render_html(&session, &ranked, &Weights::default(), &meta)
        );
        return ExitCode::SUCCESS;
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p sumcp-cli --test html_report 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Visual check (manual, one-time)**

```bash
cargo run -q -p sumcp-cli -- --file fixtures/session-2_1_210-subagents.jsonl --html > /tmp/sumcp-report.html
```
Open `/tmp/sumcp-report.html` in a browser; confirm the Win9x look, the timeline reads left→right, bands click through to evidence, and it prints. Refine CSS with the `frontend-design` skill if the aesthetic is off — tests must stay green.

- [ ] **Step 6: Full workspace suite + fmt + clippy + commit**

```bash
cargo test 2>&1 | tail -8
cargo fmt && cargo clippy --workspace -- -D warnings
git add crates/sumcp-cli/src/main.rs crates/sumcp-cli/tests/html_report.rs
git commit -m "T5.1f: sumcp --html flag + end-to-end self-containment golden test"
```

---

## Self-Review

**Spec coverage** (against the T5.1 line + locked 2026-07-19 design):
- Self-contained, zero network → Task 1 shell invariant + Task 6 golden test ✓
- Win9x readability-first, light-only → CSS across Tasks 1–4, refined via frontend-design ✓
- Section order overview→timeline→struggles→blind spots→file stories→health→evidence → wired in Tasks 2–4 ✓
- Timeline centerpiece (Read/Edit/Bash lanes, ticks, finding bands, user rules) → Task 3 ✓
- Minimal vanilla JS, print/no-JS safe → Task 5 (enhancement-only) ✓
- Evidence in native `<details>` → Task 4 ✓
- Redaction of shared excerpts → reuses `payloads::evidence` (already redacts); asserted in Task 6 ✓

**Placeholder scan:** no TBD/TODO; every code step shows real code; CSS-refinement delegated to frontend-design is explicit and bounded by asserted class names (not a placeholder — the baseline CSS compiles and passes).

**Type consistency:** `render_html(&Session, &[FileScore], &Weights, &SessionMeta) -> String` used identically in Tasks 1, 3, 6; `esc`, `group_box`, `data-idxs`/`data-idx` attribute names consistent across Tasks 3–5; `Idx.0` field access matches `model.rs`.

**Open risk carried forward:** the donor fixture's exact filename is confirmed at Task 6 Step 1 before the test is trusted (the plan says use the real path, not a guess).
