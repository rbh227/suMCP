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

/// Render the full self-contained report document.
pub fn render_html(
    s: &Session,
    ranked: &[FileScore],
    weights: &Weights,
    meta: &SessionMeta,
) -> String {
    let _ = weights; // wired in later tasks
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
    let o = crate::report::Overview::from_session(s);
    h.push_str(&overview_section(&o));
    h.push_str(&timeline_section(s, ranked));
    h.push_str(&struggles_section(ranked));
    h.push_str(&blind_spots_section(s, meta));
    h.push_str(&file_stories_section(s, ranked, meta));
    h.push_str(&context_health_footer(s, meta));
    let _ = write!(h, "</div><script>{}</script></body></html>", inline_js());
    h
}

/// Inline base stylesheet — Win9x surfaces. Extended in later tasks.
fn base_css() -> &'static str {
    "body{margin:0;font:13px/1.4 'MS Sans Serif',Tahoma,sans-serif;\
     background:#008080;color:#000}\
     .titlebar{background:navy;color:#fff;font-weight:bold;padding:3px 6px}\
     .desktop{padding:8px}\
     .gb{border:2px groove #fff;background:#c0c0c0;margin:8px 0;padding:8px}\
     .gb>legend{padding:0 4px;font-weight:bold}\
     table{border-collapse:collapse}\
     .kv th{text-align:left;padding:2px 8px 2px 0;color:navy}\
     .kv td{padding:2px 16px 2px 0}\
     .rank-tbl{width:100%;background:#fff;border:2px inset #fff}\
     .rank-tbl th{background:#c0c0c0;text-align:left;padding:2px 6px}\
     .rank-tbl td{padding:2px 6px;border-top:1px solid #dfdfdf}\
     .rank-tbl .file{font-family:'Courier New',monospace}\
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
     .cap{color:#444;font-size:11px;margin:4px 0 0}\
     .exc{font-family:'Courier New',monospace;white-space:pre-wrap;word-break:break-all}\
     details.ev{margin:4px 0}"
}

/// A Win9x sunken group box with a title tab.
fn group_box(title: &str, body: &str) -> String {
    format!(
        "<fieldset class=\"gb\"><legend>{}</legend>{}</fieldset>",
        esc(title),
        body
    )
}

/// Overview: the deterministic totals.
fn overview_section(o: &crate::report::Overview) -> String {
    let ratio = o
        .cache_hit_ratio
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let body = format!(
        "<table class=\"kv\">\
         <tr><th>actions</th><td>{}</td><th>files</th><td>{}</td></tr>\
         <tr><th>edits</th><td>{}</td><th>writes</th><td>{}</td></tr>\
         <tr><th>reads</th><td>{}</td><th>bash</th><td>{}</td></tr>\
         <tr><th>cache hit</th><td>{}</td><th>output tok</th><td>{}</td></tr>\
         </table>",
        o.actions, o.files_touched, o.edits, o.writes, o.reads, o.bash, ratio, o.output_tokens,
    );
    group_box("Overview", &body)
}

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
        if n <= 1 {
            0.0
        } else {
            idx as f64 / (n as f64 - 1.0) * 100.0
        }
    };

    // Finding bands: one per ranking finding that has idxs, spanning min..max.
    let mut bands = String::new();
    for f in ranked.iter().flat_map(|fs| &fs.findings) {
        if f.idxs.is_empty() {
            continue;
        }
        let lo = f.idxs.iter().map(|i| i.0 as usize).min().unwrap();
        let hi = f.idxs.iter().map(|i| i.0 as usize).max().unwrap();
        let idxs_attr = f
            .idxs
            .iter()
            .map(|i| i.0.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let kind = format!("{:?}", f.kind);
        let _ = write!(
            bands,
            "<div class=\"band\" style=\"left:{:.2}%;width:{:.2}%\" \
             data-idxs=\"{}\" title=\"{}\"></div>",
            x(lo),
            (x(hi) - x(lo)).max(0.6),
            esc(&idxs_attr),
            esc(&kind),
        );
    }

    // Lane ticks.
    let lane_of = |k: &ActionKind| match k {
        ActionKind::Read => Some("read"),
        ActionKind::Edit | ActionKind::Write => Some("edit"),
        ActionKind::Bash => Some("bash"),
        ActionKind::Other(_) => None,
    };
    let mut ticks: std::collections::BTreeMap<&str, String> = [
        ("read", String::new()),
        ("edit", String::new()),
        ("bash", String::new()),
    ]
    .into_iter()
    .collect();
    let mut other = 0usize;
    for a in &s.actions {
        let Some(lane) = lane_of(&a.kind) else {
            other += 1;
            continue;
        };
        let err = if a.is_error == Some(true) {
            " tick-err"
        } else {
            ""
        };
        let file = a.file_path.as_deref().unwrap_or("");
        let _ = write!(
            ticks.get_mut(lane).unwrap(),
            "<div class=\"tick{}\" style=\"left:{:.2}%\" \
             data-idx=\"{}\" title=\"#{} {}\"></div>",
            err,
            x(a.idx.0 as usize),
            a.idx.0,
            a.idx.0,
            esc(file),
        );
    }

    // User-turn rules: place at the ordinal of the first action at/after the
    // user text's source line. Deterministic; falls at 100% if none follow.
    let mut rules = String::new();
    for ut in &s.user_texts {
        let ord = s
            .actions
            .iter()
            .position(|a| a.line_no >= ut.line_no)
            .unwrap_or(n - 1);
        let _ = write!(
            rules,
            "<div class=\"urule\" style=\"left:{:.2}%\"></div>",
            x(ord)
        );
    }

    let lane = |name: &str, label: &str| {
        format!(
            "<div class=\"lane lane-{name}\"><span class=\"lane-lbl\">{label}</span>{}</div>",
            ticks[name]
        )
    };
    let caption = if other > 0 {
        format!(
            "<p class=\"cap\">{} action(s) in other tools not laned.</p>",
            other
        )
    } else {
        String::new()
    };

    let body = format!(
        "<div class=\"timeline\"><div class=\"bands\">{bands}</div>{rules}{}{}{}</div>{caption}",
        lane("read", "Read"),
        lane("edit", "Edit"),
        lane("bash", "Bash"),
    );
    group_box("Timeline", &body)
}

/// Struggle areas: ranked files with their category breakdown.
fn struggles_section(ranked: &[FileScore]) -> String {
    if ranked.is_empty() {
        return group_box("Struggle areas", "<p>No struggle signals fired.</p>");
    }
    let mut rows = String::new();
    for (i, f) in ranked.iter().enumerate() {
        let cats: Vec<String> = f
            .breakdown
            .iter()
            .map(|(k, v)| format!("{} {}", esc(k), v))
            .collect();
        let _ = write!(
            rows,
            "<tr><td class=\"rank\">{}</td><td class=\"file\">{}</td>\
             <td class=\"score\">{:.1}</td><td>{}</td></tr>",
            i + 1,
            esc(&f.file),
            f.score,
            esc(&cats.join(", ")),
        );
    }
    let body = format!(
        "<table class=\"rank-tbl\"><thead><tr><th>#</th><th>file</th>\
         <th>score</th><th>breakdown</th></tr></thead><tbody>{}</tbody></table>",
        rows
    );
    group_box("Struggle areas", &body)
}

/// Blind spots: counts of blind-write attempts, review-burden findings, and
/// approval outliers, plus whether the approval-latency signal is active or
/// suppressed (auto-accept mode). Reads `payloads::blind_spots` — never
/// recomputes — so the HTML can't drift from the MCP contract.
fn blind_spots_section(s: &Session, meta: &SessionMeta) -> String {
    let p = crate::payloads::blind_spots(s, meta);
    let count = |key: &str| p[key].as_array().map(|a| a.len()).unwrap_or(0);
    let latency = p["suppression"]["approval_latency"].as_str().unwrap_or("");
    let body = format!(
        "<table class=\"kv\">\
         <tr><th>blind-write attempts</th><td>{}</td></tr>\
         <tr><th>review-burden findings</th><td>{}</td></tr>\
         <tr><th>approval outliers</th><td>{}</td></tr>\
         <tr><th>approval latency</th><td>{}</td></tr>\
         </table>",
        count("blind_write_attempts"),
        count("review_burden"),
        count("approval_outliers"),
        esc(latency),
    );
    group_box("Blind spots", &body)
}

/// One file's chronological story: head events, an "elided" gap marker if the
/// middle was dropped, then tail events — mirrors `payloads::file_story`
/// exactly (middle-out elision, same field names).
fn file_story_section(s: &Session, path: &str, meta: &SessionMeta) -> String {
    let p = crate::payloads::file_story(s, path, meta);
    let render_events = |events: &[serde_json::Value]| -> String {
        let mut items = String::new();
        for v in events {
            let _ = write!(
                items,
                "<li>#{} · {} · {}</li>",
                v["idx"].as_u64().unwrap_or(0),
                esc(v["action"].as_str().unwrap_or("")),
                esc(v["outcome"].as_str().unwrap_or("n/a")),
            );
        }
        items
    };
    let mut body = String::from("<ol class=\"story\">");
    body.push_str(&render_events(
        p["events"].as_array().map(|v| v.as_slice()).unwrap_or(&[]),
    ));
    if let Some(elided) = p.get("elided").filter(|v| !v.is_null()) {
        let _ = write!(
            body,
            "<li class=\"cap\">… {} events elided — {} …</li>",
            elided["count"].as_u64().unwrap_or(0),
            esc(elided["note"].as_str().unwrap_or("")),
        );
    }
    body.push_str(&render_events(
        p["tail"].as_array().map(|v| v.as_slice()).unwrap_or(&[]),
    ));
    body.push_str("</ol>");
    group_box(&format!("File story: {}", path), &body)
}

/// The top-3 ranked files, each with its story and the evidence behind its
/// findings nested inside.
fn file_stories_section(s: &Session, ranked: &[FileScore], meta: &SessionMeta) -> String {
    if ranked.is_empty() {
        return group_box("File stories", "<p>No files ranked.</p>");
    }
    let mut out = String::new();
    for fs in ranked.iter().take(3) {
        let mut section = file_story_section(s, &fs.file, meta);
        // Nest the evidence for this file's findings inside its story box —
        // dropped just before the closing </fieldset>.
        let idxs: Vec<crate::model::Idx> = fs
            .findings
            .iter()
            .flat_map(|f| f.idxs.iter().copied())
            .collect();
        let ev = evidence_details(s, &idxs, meta);
        if let Some(pos) = section.rfind("</fieldset>") {
            section.insert_str(pos, &ev);
        } else {
            section.push_str(&ev);
        }
        out.push_str(&section);
    }
    out
}

/// Context-health footer: cache economics, informational only (v0.1 makes no
/// waste judgment — see `payloads::context_health`'s note).
fn context_health_footer(s: &Session, meta: &SessionMeta) -> String {
    let p = crate::payloads::context_health(s, meta);
    let ratio = p["cache_hit_ratio"]
        .as_f64()
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "n/a".into());
    let body = format!(
        "<p>cache hit {} · output {} · cache-read {} tok. \
         <em>Informational only — v0.1 makes no waste judgment.</em></p>",
        ratio,
        p["tokens"]["output"].as_u64().unwrap_or(0),
        p["tokens"]["cache_read"].as_u64().unwrap_or(0),
    );
    group_box("Context health", &body)
}

/// A native `<details>` collapsible dereferencing action `idxs` into raw
/// evidence via `payloads::evidence` — the same excerpts, caps, and
/// redaction the MCP tool returns. `data-idxs` is stamped for Task 5's
/// click-to-expand wiring from the timeline bands.
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
    let idxs_attr = idxs
        .iter()
        .map(|i| i.0.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "<details class=\"ev\" data-idxs=\"{}\"><summary>evidence</summary>\
         <table class=\"ev-tbl\"><tbody>{}</tbody></table></details>",
        esc(&idxs_attr),
        rows
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;
    use crate::payloads::SessionMeta;
    use crate::score::{Weights, rank};

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "sess-1".into(),
            identified_by: "explicit".into(),
        }
    }

    fn render(raw: &str) -> String {
        let s = ingest_str(raw, Lane::Main);
        let w = Weights::default();
        let r = rank(&s, &w);
        render_html(&s, &r, &w, &meta())
    }

    #[test]
    fn escapes_html_metacharacters_in_paths() {
        // A file path containing HTML must never render as live markup. Six
        // edits to one file ⇒ churn ⇒ it ranks ⇒ it shows up in the
        // struggle-areas table, which is the real (non-scaffold) path that
        // renders `esc(&f.file)`.
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/<script>x</script>.ts","new_string":"x"}}}}]}}}}"#
            ));
        }
        let html = render(&lines.join("\n"));
        assert!(!html.contains("<script>x</script>.ts"), "raw markup leaked");
        assert!(
            html.contains("&lt;script&gt;x&lt;/script&gt;.ts"),
            "path not escaped"
        );
    }

    #[test]
    fn overview_precedes_struggles() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(
            html.find("Overview").unwrap() < html.find("Struggle").unwrap(),
            "overview must precede struggles"
        );
    }

    #[test]
    fn struggles_section_empty_when_nothing_ranked() {
        // A single Read, no edits ⇒ no ranking findings ⇒ empty branch.
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(
            html.contains("No struggle signals fired."),
            "empty-ranked branch not shown"
        );
    }

    #[test]
    fn shell_is_a_complete_selfcontained_document() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(html.starts_with("<!DOCTYPE html>"), "missing doctype");
        assert!(html.contains("<html"), "missing html element");
        assert!(html.contains("sess-1"), "session id not shown");
        // Zero-network invariant (the whole point of the artifact). Robust form:
        // all dynamic content is esc()'d, so a content quote becomes &quot; and
        // `="http` / `<script src` can only come from OUR markup — never from an
        // evidence excerpt that legitimately contains a URL.
        assert!(!html.contains("=\"http"), "external URL in an attribute");
        assert!(
            !html.contains("<script src") && !html.contains("<link") && !html.contains("<img"),
            "external-loading element present"
        );
    }

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
        assert!(
            html.contains(">2<")
                || html.contains("actions</th><td>2")
                || html.contains("actions 2"),
            "action count 2 not rendered: {}",
            &html[..html.len().min(4000)]
        );
    }

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
        assert!(
            html.contains("lane-read") && html.contains("lane-edit") && html.contains("lane-bash"),
            "three lanes required"
        );
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
        assert!(
            html.contains("File story") && html.contains("/a.ts"),
            "no file story"
        );
        // Evidence lives in a native collapsible.
        assert!(html.contains("<details"), "evidence not in <details>");
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

    #[test]
    fn timeline_single_action_no_divide_by_zero() {
        // n == 1 makes the `x(idx)` ordinal-to-percent map divide by
        // `n - 1 == 0`; the guard short-circuits to 0.0 instead. Render a
        // session with exactly one action and confirm no NaN/inf leaked
        // into the output, and that the lone action still produces a tick
        // in its lane (Read here).
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(
            !html.contains("NaN") && !html.contains("inf"),
            "divide-by-zero leaked into rendered output"
        );
        let read_lane_start = html.find("lane-read").expect("read lane missing");
        let read_lane_end = html[read_lane_start..]
            .find("lane-edit")
            .map(|i| read_lane_start + i)
            .unwrap_or(html.len());
        assert!(
            html[read_lane_start..read_lane_end].contains("tick"),
            "no tick rendered in the read lane for the single action"
        );
    }

    #[test]
    fn render_is_deterministic() {
        // Same session in, byte-identical HTML out — no hidden nondeterminism
        // (unordered iteration, timestamps, addresses) should ever leak into
        // a report that's meant to be a stable, diffable artifact.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
        );
        let a = render(raw);
        let b = render(raw);
        assert_eq!(
            a, b,
            "render_html is not deterministic across repeated calls"
        );
    }

    #[test]
    fn inline_js_is_present_and_local_only() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        assert!(html.contains("<script>"), "no inline script");
        assert!(html.contains("addEventListener"), "no interaction wiring");
        // Still zero-network after adding JS (robust escaped-attribute form).
        assert!(!html.contains("=\"http"), "external URL reference present");
    }
}
