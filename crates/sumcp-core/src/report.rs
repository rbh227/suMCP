//! Overview report: the counts behind `session_overview` and the bare CLI.
//!
//! v0.1 slice — struggle findings arrive in later tasks. This computes the
//! deterministic totals from a [`Session`] and shapes them for display.

use crate::model::{ActionKind, Session};
use serde::Serialize;
use std::collections::BTreeSet;

/// The overview counts for one session.
#[derive(Debug, Serialize)]
pub struct Overview {
    /// Total actions (tool calls) after dedup.
    pub actions: usize,
    /// Distinct files touched by Read/Edit/Write.
    pub files_touched: usize,
    /// Edit count.
    pub edits: usize,
    /// Write count.
    pub writes: usize,
    /// Read count.
    pub reads: usize,
    /// Bash count.
    pub bash: usize,
    /// Output tokens.
    pub output_tokens: u64,
    /// Cache-read tokens.
    pub cache_read_tokens: u64,
    /// Cache-hit ratio, if computable.
    pub cache_hit_ratio: Option<f64>,
    /// First → last effective timestamp (ISO strings), if any actions exist.
    pub span: Option<(String, String)>,
    /// Event-type histogram.
    pub type_counts: std::collections::BTreeMap<String, u64>,
    /// Lines that failed to parse (never fatal).
    pub parse_errors: u64,
    /// Lines with no timestamp (ordering used carry-forward).
    pub untimestamped_lines: u64,
}

impl Overview {
    /// Compute the overview from a parsed session.
    pub fn from_session(s: &Session) -> Self {
        // Count by kind in one pass. `iter().filter(...).count()` is the
        // idiomatic Rust way to count matching elements.
        let count = |k: &ActionKind| s.actions.iter().filter(|a| &a.kind == k).count();

        // Distinct files: collect into a set. `flatten()` drops the `None`s
        // from `Option<&String>`, so only real paths land in the set.
        let files: BTreeSet<&String> = s
            .actions
            .iter()
            .filter(|a| {
                matches!(
                    a.kind,
                    ActionKind::Read | ActionKind::Edit | ActionKind::Write
                )
            })
            .filter_map(|a| a.file_path.as_ref())
            .collect();

        // Actions are already in total order, so first/last give the span.
        let span = match (s.actions.first(), s.actions.last()) {
            (Some(f), Some(l)) => Some((f.effective_ts.clone(), l.effective_ts.clone())),
            _ => None,
        };

        Overview {
            actions: s.actions.len(),
            files_touched: files.len(),
            edits: count(&ActionKind::Edit),
            writes: count(&ActionKind::Write),
            reads: count(&ActionKind::Read),
            bash: count(&ActionKind::Bash),
            output_tokens: s.tokens.output,
            cache_read_tokens: s.tokens.cache_read,
            cache_hit_ratio: s.tokens.cache_hit_ratio(),
            span,
            type_counts: s.type_counts.clone(),
            parse_errors: s.parse_errors,
            untimestamped_lines: s.untimestamped_lines,
        }
    }

    /// Render a human-readable overview (the bare-`sumcp` view).
    pub fn to_text(&self) -> String {
        let ratio = self
            .cache_hit_ratio
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "n/a".into());
        let mut out = String::new();
        out.push_str("── session overview ──\n");
        out.push_str(&format!(
            "actions {}  |  files {}  |  edits {}  writes {}  reads {}  bash {}\n",
            self.actions, self.files_touched, self.edits, self.writes, self.reads, self.bash
        ));
        out.push_str(&format!(
            "tokens: output {}  cache-read {}  (cache hit {})\n",
            self.output_tokens, self.cache_read_tokens, ratio
        ));
        if let Some((a, b)) = &self.span {
            out.push_str(&format!("span: {a} → {b}\n"));
        }
        if self.parse_errors > 0 || self.untimestamped_lines > 0 {
            out.push_str(&format!(
                "parse: {} bad lines, {} untimestamped\n",
                self.parse_errors, self.untimestamped_lines
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;

    #[test]
    fn overview_counts_kinds_and_distinct_files() {
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Read","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"2","name":"Edit","input":{"file_path":"/a.ts"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_use","id":"3","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        let o = Overview::from_session(&ingest_str(raw, Lane::Main));
        assert_eq!(o.actions, 3);
        assert_eq!(o.reads, 1);
        assert_eq!(o.edits, 1);
        assert_eq!(o.bash, 1);
        assert_eq!(o.files_touched, 1, "same file read+edited counts once");
        assert_eq!(o.span.unwrap().0, "2026-01-01T00:00:00Z");
    }
}
