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
    let _ = write!(h, "</div></body></html>");
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
     .cap{color:#444;font-size:11px;margin:4px 0 0}"
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
}
