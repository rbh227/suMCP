//! Secrets-file touches (spec 2026-07-26, added during execution).
//!
//! One finding per secrets-class file that the session read, edited, or wrote.
//! Exact and high-confidence: this is a literal fact about the action log, not
//! an inference. Per file rather than per action so a file read ten times
//! produces one review item carrying ten citations.

use crate::file_class::is_secrets;
use crate::model::{ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};
use std::collections::BTreeMap;

/// Findings for every secrets-class file the session touched, ordered by path.
pub fn secrets(s: &Session) -> Vec<Finding> {
    // BTreeMap so the output order is path order: deterministic without a
    // separate sort.
    let mut per_file: BTreeMap<&str, (u64, u64, Vec<Idx>)> = BTreeMap::new();
    for a in &s.actions {
        let Some(file) = a.file_path.as_deref() else {
            continue;
        };
        let is_read = matches!(a.kind, ActionKind::Read);
        let is_write = matches!(a.kind, ActionKind::Edit | ActionKind::Write);
        if !(is_read || is_write) || !is_secrets(file) {
            continue;
        }
        let entry = per_file.entry(file).or_insert((0, 0, Vec::new()));
        if is_read {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
        entry.2.push(a.idx);
    }

    per_file
        .into_iter()
        .map(|(file, (reads, edits, idxs))| {
            let mut nums = BTreeMap::new();
            nums.insert("reads".to_string(), reads as f64);
            nums.insert("edits".to_string(), edits as f64);
            Finding {
                kind: FindingKind::SecretsFileTouched,
                tier: Tier::T1,
                exact: true,
                confidence: Confidence::High,
                idxs,
                file: Some(file.to_string()),
                note: Some(format!(
                    "credentials or key file: {reads} read(s), {edits} write(s)"
                )),
                nums,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::{FindingKind, Lane};

    fn tool(id: &str, ts: &str, name: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"file_path":"{file}"}}}}]}}}}"#
        )
    }

    #[test]
    fn a_read_of_a_secrets_file_is_a_finding() {
        let raw = tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/.env");
        let s = ingest_str(&raw, Lane::Main);
        let f = secrets(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::SecretsFileTouched);
        assert_eq!(f[0].file.as_deref(), Some("/repo/.env"));
        assert_eq!(f[0].nums.get("reads"), Some(&1.0));
        assert_eq!(f[0].nums.get("edits"), Some(&0.0));
        assert_eq!(f[0].idxs.len(), 1);
    }

    #[test]
    fn reads_and_edits_of_one_file_collapse_into_one_finding() {
        let raw = format!(
            "{}\n{}\n{}",
            tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/.env"),
            tool("e1", "2026-01-01T00:00:02Z", "Edit", "/repo/.env"),
            tool("r2", "2026-01-01T00:00:03Z", "Read", "/repo/.env"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let f = secrets(&s);
        assert_eq!(f.len(), 1, "one finding per file, not per action");
        assert_eq!(f[0].nums.get("reads"), Some(&2.0));
        assert_eq!(f[0].nums.get("edits"), Some(&1.0));
        assert_eq!(f[0].idxs.len(), 3, "every touching action is cited");
    }

    #[test]
    fn ordinary_files_produce_nothing() {
        let raw = format!(
            "{}\n{}",
            tool("e1", "2026-01-01T00:00:01Z", "Edit", "/repo/src/main.rs"),
            tool("e2", "2026-01-01T00:00:02Z", "Edit", "/repo/Cargo.toml"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert!(secrets(&s).is_empty());
    }

    #[test]
    fn findings_are_ordered_by_path_for_determinism() {
        let raw = format!(
            "{}\n{}",
            tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/z.pem"),
            tool("r2", "2026-01-01T00:00:02Z", "Read", "/repo/a.pem"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let found = secrets(&s);
        let files: Vec<&str> = found.iter().filter_map(|f| f.file.as_deref()).collect();
        assert_eq!(files, vec!["/repo/a.pem", "/repo/z.pem"]);
    }
}
