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
       document.querySelectorAll('.band.linked').forEach(function(b){\
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
    h.push_str(&needs_review_section(&review, &all, ranked, s)); // Task 5
    h.push_str(&timeline_section(s, ranked, &review)); // Task 6
    h.push_str(&struggles_section(ranked, weights, &review)); // Task 7
    h.push_str(&file_stories_section(s, &review, meta)); // Task 8
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
       background:rgba(0,0,128,.12);border:1px dashed var(--navy);\
       cursor:default}\
     .band.linked{border-style:solid;cursor:pointer}\
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
             actions, each capped at {} minutes\">active {} (span {})</span>",
            crate::report::ACTIVE_GAP_CAP_SECS / 60,
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
        facts.push(format!(
            "<span><b>{}</b> subagents</span>",
            fmt_thousands(s.spawns.len() as u64)
        ));
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

/// The lead section: which files need eyes, and why — or an explicit calm
/// state. Reasons are strictly descriptive (grill decision 2026-07-22).
fn needs_review_section(
    review: &[crate::review::ReviewCandidate],
    all: &[crate::model::Finding],
    ranked: &[FileScore],
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
        } else if !ranked.is_empty() {
            "No files met the review bar. Minor signals appear in struggle \
             areas below."
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
    for (i, c) in review.iter().enumerate() {
        let _ = write!(
            rows,
            "<div class=\"nr\"><span class=\"mono\">{file}</span>\
             <span class=\"why\">{why}</span>\
             <a href=\"#story-{n}\">story</a></div>",
            file = esc(&c.file),
            why = esc(&crate::review::reason_sentence(c)),
            n = i + 1,
        );
    }
    format!("<section class=\"sec\"><h2>Needs review</h2>{rows}</section>")
}

/// The timeline centerpiece: action ticks in Read/Edit/Bash lanes, finding
/// bands overlaid, user turns as vertical rules. Pure positioned divs so it
/// prints and works with JS disabled.
fn timeline_section(
    s: &Session,
    ranked: &[FileScore],
    review: &[crate::review::ReviewCandidate],
) -> String {
    use crate::model::ActionKind;
    let n = s.actions.len();
    if n == 0 {
        return "<section class=\"sec\"><h2>Timeline</h2><p class=\"calm\">No actions.</p></section>"
            .to_string();
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
    // Only files that qualified as review candidates have an evidence
    // collapsible to jump to, so only their bands are clickable (Fix 2:
    // the rest render but stay honestly inert).
    let candidate_files: std::collections::BTreeSet<&str> =
        review.iter().map(|c| c.file.as_str()).collect();
    let mut bands = String::new();
    for fs in ranked {
        let linked = candidate_files.contains(fs.file.as_str());
        let class = if linked { "band linked" } else { "band" };
        for f in &fs.findings {
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
                "<div class=\"{class}\" style=\"left:{:.2}%;width:{:.2}%\" \
                 data-idxs=\"{}\" title=\"{}\"></div>",
                x(lo),
                (x(hi) - x(lo)).max(0.6),
                esc(&idxs_attr),
                esc(&kind),
            );
        }
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
                 title=\"gap over {} minutes\"></div>",
                crate::report::ACTIVE_GAP_CAP_SECS / 60,
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
        let redacted = crate::redact::redact(&ut.text);
        let excerpt: String = redacted.chars().take(80).collect();
        let _ = write!(
            rules,
            "<div class=\"urule\" style=\"left:{:.2}%\" title=\"{}\"></div>",
            x(ord),
            esc(excerpt
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .as_str()),
        );
    }

    let lane = |name: &str, label: &str, content: &str| {
        format!(
            "<div class=\"lane lane-{name}\"><span class=\"lane-lbl\">{label}</span>\
             <div class=\"track\">{content}</div></div>"
        )
    };
    let legend = format!(
        "<div class=\"legend\">\
        <span><i class=\"sw sw-tick\"></i>action</span>\
        <span><i class=\"sw sw-err\"></i>error</span>\
        <span><i class=\"sw sw-band\"></i>finding span (solid: click for evidence)</span>\
        <span><i class=\"sw sw-turn\"></i>your turn (hover for prompt)</span>\
        <span><i class=\"sw sw-gap\"></i>gap &gt; {} min</span>\
        <span>x = action sequence, not time</span></div>",
        crate::report::ACTIVE_GAP_CAP_SECS / 60,
    );
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
}

/// Ranked files: cap 10, plain-language breakdown, top-3 emphasized and
/// linked to their stories, formula + exact weights footnoted (the
/// transparency promise in SPEC decision 6).
fn struggles_section(
    ranked: &[FileScore],
    weights: &Weights,
    review: &[crate::review::ReviewCandidate],
) -> String {
    if ranked.is_empty() {
        return "<section class=\"sec\"><h2>Struggle areas</h2>\
             <p class=\"calm\">No struggle signals fired.</p></section>"
            .to_string();
    }
    let story_anchor =
        |file: &str| -> Option<usize> { review.iter().position(|c| c.file == file).map(|i| i + 1) };
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
            Some(n) => format!("<a class=\"mono\" href=\"#story-{n}\">{}</a>", esc(&f.file)),
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
        "<p class=\"foot\">score = weight x count, low-confidence x{lcf}, \
         churn scaled by relative churn when known (x0.5 to x2) · \
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

/// One rendered story row: either a single event, a compressed run of 3+
/// consecutive same-kind non-failing events, or an "elided middle" marker
/// (also a single event under the hood, `outcome == "elided"`; `count`
/// carries how many events it stands for).
enum StoryRow {
    One {
        idx: u64,
        action: String,
        outcome: String,
        count: Option<u64>,
    },
    Run {
        action: String,
        first: u64,
        last: u64,
        n: usize,
    },
}

/// Collapse consecutive same-action, same-outcome events (never failures,
/// never the elided marker — its synthetic `__elided__` action never matches
/// a real neighbor) into runs of 3 or more. Pure; unit-testable via the
/// rendered HTML.
fn compress_runs(events: &[serde_json::Value]) -> Vec<StoryRow> {
    let get = |v: &serde_json::Value| {
        (
            v["idx"].as_u64().unwrap_or(0),
            v["action"].as_str().unwrap_or("").to_string(),
            v["outcome"].as_str().unwrap_or("n/a").to_string(),
            v["count"].as_u64(),
        )
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let (idx, action, outcome, _) = get(&events[i]);
        let mut j = i + 1;
        while j < events.len() && outcome != "fail" {
            let (_, a2, o2, _) = get(&events[j]);
            if a2 != action || o2 != outcome {
                break;
            }
            j += 1;
        }
        if j - i >= 3 {
            let (last, _, _, _) = get(&events[j - 1]);
            out.push(StoryRow::Run {
                action,
                first: idx,
                last,
                n: j - i,
            });
        } else {
            for v in &events[i..j] {
                let (idx, action, outcome, count) = get(v);
                out.push(StoryRow::One {
                    idx,
                    action,
                    outcome,
                    count,
                });
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Render story rows; failing events show their redacted error excerpt, the
/// elided marker renders its own "N events elided" line.
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
                count,
            } => {
                if outcome == "elided" {
                    let _ = write!(
                        out,
                        "<li class=\"run\">… {} events elided …</li>",
                        count.unwrap_or(0)
                    );
                } else if outcome == "fail" {
                    let err = s
                        .actions
                        .get(*idx as usize)
                        .and_then(|a| a.error.as_deref())
                        .unwrap_or("");
                    let redacted = crate::redact::redact(err);
                    let excerpt: String = redacted.chars().take(120).collect();
                    let _ = write!(
                        out,
                        "<li class=\"fail\">#{idx} · {} · fail \
                         <span class=\"exc\">{}</span></li>",
                        esc(action),
                        esc(&excerpt),
                    );
                } else {
                    let _ = write!(out, "<li>#{idx} · {} · {}</li>", esc(action), esc(outcome));
                }
            }
        }
    }
    out
}

/// Mirror of `payloads::kind_str` (private to that module): the action-name
/// string used to render an attributed Bash failure the same way a real
/// `file_story` event would.
fn action_kind_str(k: &crate::model::ActionKind) -> String {
    use crate::model::ActionKind;
    match k {
        ActionKind::Read => "Read".into(),
        ActionKind::Edit => "Edit".into(),
        ActionKind::Write => "Write".into(),
        ActionKind::Bash => "Bash".into(),
        ActionKind::Other(n) => n.clone(),
    }
}

/// The needs-review files, each opening with its plain-language "why", a
/// run-compressed chronological story, any repeated failing command behind a
/// failure loop, and the evidence for its findings nested inside. Iterates
/// `review` (the evidence-floor selection); boxes carry `id="story-N"`
/// (1-based) so the needs-review rows and struggle-table links can jump here.
fn file_stories_section(
    s: &Session,
    review: &[crate::review::ReviewCandidate],
    meta: &SessionMeta,
) -> String {
    use crate::model::FindingKind;
    let mut out = String::new();
    for (i, c) in review.iter().enumerate() {
        let p = crate::payloads::file_story(s, &c.file, meta);
        let empty = Vec::new();
        let head: Vec<serde_json::Value> = p["events"].as_array().unwrap_or(&empty).clone();
        let tail: Vec<serde_json::Value> = p["tail"].as_array().unwrap_or(&empty).clone();

        // Merge in attributed failing Bash commands from this file's
        // FailureLoop findings. `payloads::file_story` filters strictly by
        // `file_path`, and Bash actions never carry one — attribution lives
        // only in the finding's idxs — so without this merge those failures
        // never reach the chronology (Fix 5). `payloads.rs` itself stays
        // untouched: this is HTML-layer-only stitching of two already-frozen
        // payloads (`file_story`'s events, `struggle_areas`'s findings).
        let mut present: std::collections::BTreeSet<u64> = head
            .iter()
            .chain(tail.iter())
            .filter_map(|v| v["idx"].as_u64())
            .collect();
        let mut merged: Vec<serde_json::Value> = head.iter().chain(tail.iter()).cloned().collect();
        for f in c
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::FailureLoop)
        {
            for idx in &f.idxs {
                let idx_u64 = idx.0 as u64;
                if present.contains(&idx_u64) {
                    continue;
                }
                if let Some(a) = s.actions.get(idx.0 as usize) {
                    merged.push(serde_json::json!({
                        "idx": idx_u64,
                        "action": action_kind_str(&a.kind),
                        "outcome": "fail",
                    }));
                    present.insert(idx_u64);
                }
            }
        }
        // The elided-middle marker (if any) sorts right after the last head
        // event, before any tail or attributed-failure event.
        if let Some(elided) = p.get("elided").filter(|v| !v.is_null()) {
            let last_head_idx = head.last().and_then(|v| v["idx"].as_u64()).unwrap_or(0);
            merged.push(serde_json::json!({
                "idx": last_head_idx + 1,
                "action": "__elided__",
                "outcome": "elided",
                "count": elided["count"].as_u64().unwrap_or(0),
            }));
        }
        // Sort by idx; on a tie the elided marker sorts first (secondary key
        // 0 vs 1), matching the design: a marker never loses its place to a
        // same-idx event.
        merged.sort_by_key(|v| {
            let idx = v["idx"].as_u64().unwrap_or(0);
            let is_marker = v["outcome"].as_str() == Some("elided");
            (idx, if is_marker { 0u8 } else { 1u8 })
        });
        merged.dedup_by_key(|v| v["idx"].as_u64().unwrap_or(0));

        let mut body = String::from("<ol class=\"story\">");
        body.push_str(&story_rows_html(s, &compress_runs(&merged)));
        body.push_str("</ol>");
        // The repeated failing command behind any failure loop, verbatim.
        for f in c
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::FailureLoop)
        {
            if let Some(cmd) = f
                .idxs
                .first()
                .and_then(|i| s.actions.get(i.0 as usize))
                .and_then(|a| a.command.as_deref())
            {
                let redacted = crate::redact::redact(cmd);
                let capped: String = redacted.chars().take(120).collect();
                let _ = write!(
                    body,
                    "<p class=\"foot\">repeated failing command: \
                     <span class=\"exc\">{}</span></p>",
                    esc(&capped)
                );
            }
        }
        let idxs: Vec<crate::model::Idx> = c
            .findings
            .iter()
            .flat_map(|f| f.idxs.iter().copied())
            .collect();
        body.push_str(&evidence_details(s, &idxs, meta));
        let why = crate::review::reason_sentence(c);
        let why_line = match c.ranked {
            Some(fs) => format!("score {:.1} · {}", fs.score, esc(&why)),
            None => esc(&why),
        };
        let _ = write!(
            out,
            "<div class=\"story-box\" id=\"story-{n}\">\
             <h3 class=\"mono\">{file}</h3>\
             <p class=\"why\">{why_line}</p>{body}</div>",
            n = i + 1,
            file = esc(&c.file),
        );
    }
    if out.is_empty() {
        return String::new(); // calm sessions: no story section at all
    }
    format!("<section class=\"sec\"><h2>File stories</h2>{out}</section>")
}

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
            // The spec requires every listed finding to carry evidence.
            // idxs travel in the finding's payload JSON; empty ⇒ no proving
            // actions to show, so the collapsible is skipped.
            let idxs: Vec<crate::model::Idx> = f["idxs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_u64())
                .map(|n| crate::model::Idx(n as u32))
                .collect();
            let ev = if idxs.is_empty() {
                String::new()
            } else {
                evidence_details(s, &idxs, meta)
            };
            let _ = write!(
                rows,
                "<li><b>{label}</b>{}{}{tag}{ev}</li>",
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
    format!("<section class=\"sec\"><h2>Blind spots</h2>{body}{suppression}</section>")
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
        assert!(
            html.contains("lane-findings"),
            "findings strip is a labeled lane"
        );
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
        assert!(html.contains("class=\"gapmark\""), "gap glyph rendered");
        assert!(
            html.contains("title=\"gap over 5 minutes\""),
            "gap glyph tooltip"
        );
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
        assert!(
            !html.contains("sk-abcdefghijklmnopqrstuv"),
            "secret redacted"
        );
    }

    #[test]
    fn tooltip_secret_straddling_cap_is_redacted() {
        // 65 filler chars + " key sk-abcdefghijklmnopqrstuv" (a 25-char
        // prefixed token). Redacting AFTER truncating at 80 chars would slice
        // the token down to "sk-abcdefg" (10 chars, below the 16-char
        // MIN_TOKEN_LEN), so it survives un-redacted — the straddling-cap
        // leak this test guards against. Redacting BEFORE truncating
        // replaces the whole token first, so the 80-char cap never sees a
        // secret.
        let filler = "x".repeat(65);
        let raw = format!(
            concat!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"text","text":"{filler} key sk-abcdefghijklmnopqrstuv"}}]}}}}"#,
                "\n",
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"tool_use","id":"1","name":"Read","input":{{"file_path":"/a.ts"}}}}]}}}}"#,
            ),
            filler = filler,
        );
        let html = render(&raw);
        assert!(
            !html.contains("sk-abcdef"),
            "partial secret leaked past the truncation cap"
        );
        assert!(
            !html.contains("sk-abcdefghijklmnopqrstuv"),
            "full secret leaked"
        );
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
        // Top file gets a story box.
        assert!(
            html.contains("id=\"story-1\"") && html.contains("/a.ts"),
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
        assert!(html.contains("rewritten 6x"), "plain-language breakdown");
    }

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
        assert!(
            html.contains("relative churn"),
            "Fix 4: footnote must disclose the churn-scaling clause"
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
    fn below_bar_session_does_not_claim_no_signals() {
        // 6 edits to one file, each with a DIFFERENT new_string ⇒ churn fires
        // (1 finding, ranked) but nothing meets the needs-review evidence
        // floor (identical strings would also fire ActionLoop and push the
        // file over the floor — deliberately avoided here).
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x{i}"}}}}]}}}}"#
            ));
        }
        let html = render(&lines.join("\n"));
        assert!(
            html.contains("No files met the review bar"),
            "should not claim there are no signals when struggle areas has rows: {}",
            &html[..html.len().min(3000)]
        );
        assert!(
            !html.contains("No struggle signals"),
            "truthfulness regression: claimed no signals while ranked is non-empty"
        );
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
        assert!(
            !html.contains("class=\"nr\""),
            "no review rows on calm sessions"
        );
    }

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
        assert!(
            html.contains("id=\"story-1\""),
            "story anchor for review file"
        );
        assert!(html.contains("class=\"run\""), "run compression rendered");
        assert!(html.contains("x6"), "run count shown");
        assert!(
            html.contains("rewritten 6x"),
            "story box opens with its why"
        );
    }

    #[test]
    fn user_corrected_edit_reaches_needs_review() {
        // A single Edit whose tool_result carries `userModified: true` fires
        // a UserCorrected finding -- a non-ranking kind that `score::rank`
        // never includes (it doesn't contribute to any `FileScore`). Fix 1:
        // `needs_review` must still surface it: it's a solo-qualifying kind,
        // so the file appears as an unranked candidate.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":false}]},"toolUseResult":{"userModified":true}}"#,
        );
        let html = render(raw);
        let start = html.find("Needs review").expect("needs review section");
        let end = html[start..]
            .find("</section>")
            .map(|i| start + i)
            .unwrap_or(html.len());
        let section = &html[start..end];
        assert!(section.contains("/a.ts"), "file not listed: {section}");
        assert!(
            section.contains("user-corrected"),
            "reason missing: {section}"
        );
        assert!(
            !section.contains("No struggle signals"),
            "falsely claimed calm: {section}"
        );
    }

    #[test]
    fn below_floor_band_is_not_clickable() {
        // 6 edits to one file with DISTINCT new_string values: churn fires
        // and ranks (a band is drawn), but nothing meets the needs-review
        // evidence floor, so there's no evidence collapsible for the band to
        // open. Fix 2: it must render dashed/unclickable, never solid.
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x{i}"}}}}]}}}}"#
            ));
        }
        let html = render(&lines.join("\n"));
        assert!(html.contains("class=\"band\""), "no unlinked band rendered");
        assert!(
            !html.contains("class=\"band linked\""),
            "band should not be linked when below the review floor"
        );
        assert!(
            !html.contains("<details"),
            "nothing qualifies for review, so no evidence collapsible should \
             exist for the band to link to: {html}"
        );
    }

    #[test]
    fn review_band_links_to_evidence() {
        // 3 reads + 6 identical edits -> qualifies for review (churn +
        // re_read). Fix 2: its band must be linked/clickable, and its first
        // idx must appear in some evidence collapsible's data-idxs.
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
        let band_pos = html
            .find("class=\"band linked\"")
            .expect("no linked band rendered");
        let band_html = &html[band_pos..];
        let idxs_attr = band_html
            .split("data-idxs=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let first_idx = idxs_attr.split(',').next().unwrap();
        assert!(!first_idx.is_empty(), "band should carry at least one idx");
        let found = html
            .match_indices("<details class=\"ev\" data-idxs=\"")
            .any(|(pos, m)| {
                let after = &html[pos + m.len()..];
                let attr = after.split('"').next().unwrap();
                attr.split(',').any(|x| x == first_idx)
            });
        assert!(
            found,
            "band's first idx {first_idx} not found in any evidence \
             collapsible: {html}"
        );
    }

    #[test]
    fn attributed_bash_failures_render_inline() {
        // Edit /a.ts, then two failing Bash commands whose stderr names the
        // file (the failures.rs path-match attribution chain) -> a
        // FailureLoop finding attributed to /a.ts. `payloads::file_story`
        // filters strictly by `file_path`, and Bash actions never carry one,
        // so without the Fix 5 merge these two failures would never reach
        // the chronology.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"npm test"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"b1","is_error":true,"content":"Exit code 1"}]},"toolUseResult":{"stderr":"TypeError at /a.ts:10"}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"b2","name":"Bash","input":{"command":"npm test"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"b2","is_error":true,"content":"Exit code 1"}]},"toolUseResult":{"stderr":"TypeError at /a.ts:11"}}"#,
        );
        let html = render(raw);
        assert!(
            html.contains("id=\"story-1\""),
            "FailureLoop is a solo-qualifying kind, /a.ts should get a story box"
        );
        let fail_count = html.matches("class=\"fail\"").count();
        assert_eq!(
            fail_count, 2,
            "expected the two attributed Bash failures inline: {html}"
        );
        assert!(html.contains("· Bash ·"), "action name Bash not shown");
        assert!(html.contains("class=\"exc\""), "no rendered error excerpt");
    }

    #[test]
    fn blind_spot_findings_carry_evidence_collapsible() {
        // A blind-write attempt: an Edit rejected by the harness because the
        // file had not been read yet. Spec-mandated: every listed blind-spot
        // finding carries its own evidence collapsible.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"e1","is_error":true,"content":"File has not been read yet"}]}}"#,
        );
        let html = render(raw);
        let start = html.find("Blind spots").expect("blind spots section");
        let section = &html[start..];
        let end = section
            .find("</section>")
            .map(|i| i + "</section>".len())
            .unwrap_or(section.len());
        let section = &section[..end];
        assert!(
            section.contains("<details class=\"ev\""),
            "no evidence collapsible inside the blind spots section: {section}"
        );
    }

    #[test]
    fn blind_spots_calm_line_when_clean_and_suppression_sentence() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","mode":"acceptEdits","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#;
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
}
