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
    // Render file paths from actions.
    for action in &s.actions {
        if let Some(path) = &action.file_path {
            let _ = write!(h, "<div>{}</div>", esc(path));
        }
    }
    let o = crate::report::Overview::from_session(s);
    h.push_str(&overview_section(&o));
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
     .rank-tbl .file{font-family:'Courier New',monospace}"
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
        // A file path containing HTML must never render as live markup.
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/<script>x</script>.ts"}}]}}"#;
        let html = render(raw);
        assert!(!html.contains("<script>x</script>.ts"), "raw markup leaked");
        assert!(
            html.contains("&lt;script&gt;x&lt;/script&gt;.ts"),
            "path not escaped"
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
