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
    // Render file paths from actions.
    for action in &s.actions {
        if let Some(path) = &action.file_path {
            let _ = write!(h, "<div>{}</div>", esc(path));
        }
    }
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
}
