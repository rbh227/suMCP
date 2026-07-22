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
    h.push_str(&needs_review_section(&review, &all, s)); // Task 5
    h.push_str(&timeline_section(s, ranked)); // Task 6
    h.push_str(&struggles_section(ranked, weights, &review)); // Task 7
    h.push_str(&file_stories_section(s, &review, &all, meta)); // Task 8
    h.push_str(&blind_spots_section(s, meta)); // Task 8
    h.push_str(&status_bar(s, &o));
    let _ = write!(h, "</div><script>{}</script></body></html>", inline_js());
    h
}

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

/// 254703 -> "254,703". Display only; payloads keep raw numbers.
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
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
        format!(
            "<span><b>{}</b> actions</span>",
            fmt_thousands(o.actions as u64)
        ),
        format!(
            "<span><b>{}</b> files</span>",
            fmt_thousands(o.files_touched as u64)
        ),
        format!(
            "<span><b>{}</b> edits</span>",
            fmt_thousands(o.edits as u64)
        ),
        format!(
            "<span><b>{}</b> writes</span>",
            fmt_thousands(o.writes as u64)
        ),
        format!(
            "<span><b>{}</b> reads</span>",
            fmt_thousands(o.reads as u64)
        ),
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

/// A Win9x sunken group box with a title tab.
fn group_box(title: &str, body: &str) -> String {
    format!(
        "<fieldset class=\"gb\"><legend>{}</legend>{}</fieldset>",
        esc(title),
        body
    )
}

/// Needs-review section: placeholder pending Task 5's plain-language
/// rewrite. Kept minimal here so the shell compiles; Task 5 replaces the
/// body with the full evidence-floor rendering (`review::needs_review`,
/// `reason_sentence`, `category_phrase`).
fn needs_review_section(
    review: &[&FileScore],
    all: &[crate::model::Finding],
    s: &Session,
) -> String {
    let _ = (review, all, s);
    "<section class=\"sec\"><h2>Needs review</h2></section>".to_string()
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

/// Struggle areas: ranked files with their category breakdown. `weights` and
/// `review` are wired in by Task 7's rewrite; bridged here (ignored) so the
/// crate compiles against the new `render_html` call shape.
fn struggles_section(ranked: &[FileScore], weights: &Weights, review: &[&FileScore]) -> String {
    let _ = (weights, review);
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

/// The needs-review files, each with its story and the evidence behind its
/// findings nested inside. Bridged for Task 4: iterates `review` (the
/// evidence-floor selection) instead of the old `ranked.iter().take(3)`;
/// `all` is threaded through for Task 8's rewrite and unused here.
fn file_stories_section(
    s: &Session,
    review: &[&FileScore],
    all: &[crate::model::Finding],
    meta: &SessionMeta,
) -> String {
    let _ = all;
    if review.is_empty() {
        return group_box("File stories", "<p>No files ranked.</p>");
    }
    let mut out = String::new();
    for fs in review {
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
    fn header_and_facts_precede_struggles() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","cwd":"/work/proj","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
        let html = render(raw);
        let hdr = html.find("class=\"hdr\"").expect("header band");
        let facts = html.find("class=\"facts\"").expect("facts strip");
        let strug = html.find("Struggle").expect("struggle section");
        assert!(
            hdr < facts && facts < strug,
            "order: header, facts, struggles"
        );
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
        let html = render(&lines.join("\n"));
        assert!(html.contains("Blind spots"), "no blind-spots section");
        assert!(html.contains("class=\"status\""));
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
