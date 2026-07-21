//! Locating transcripts on disk — with the ADR A9 input-safety boundary.
//!
//! Claude Code stores transcripts at
//! `~/.claude/projects/<dashified-cwd>/<session-id>.jsonl`. Untrusted callers
//! can pass a `session_id`; this module validates it *before* it ever touches
//! the filesystem, so `../../etc/passwd` can never become a path.

use std::path::{Path, PathBuf};

use crate::model::Spawn;

/// Cap on subagent files merged for one session (ADR A9(3)). ~5× the largest
/// real spawn count observed (12 on the donor); the rest count as missing.
pub const MAX_SUBAGENT_FILES: usize = 64;

/// A validated session id (36-char lowercase-hex-and-dashes UUID form).
///
/// The only way to construct one is [`SessionId::parse`], so if you hold a
/// `SessionId` the validation has already happened — the type carries the
/// proof. This is the "make illegal states unrepresentable" idea.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    /// Validate a raw string as a session id. Returns `None` if it doesn't
    /// match the UUID shape (ADR A9: reject traversal/injection at the door).
    pub fn parse(raw: &str) -> Option<SessionId> {
        let ok = raw.len() == 36 && raw.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        // `.then(...)` turns a bool into an Option — idiomatic for "validate
        // then wrap". Returns None when `ok` is false.
        ok.then(|| SessionId(raw.to_string()))
    }

    /// The validated id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Encode a working directory the way Claude Code names its project folders:
/// every path separator (and other non-alphanumerics) becomes a dash.
pub fn project_dir_name(cwd: &Path) -> String {
    let s = cwd.to_string_lossy();
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// Resolve the projects directory for a cwd under a given `~/.claude` root.
pub fn project_dir(claude_home: &Path, cwd: &Path) -> PathBuf {
    claude_home.join("projects").join(project_dir_name(cwd))
}

/// Assert `candidate` resolves inside `root` after canonicalization (ADR A9:
/// reject symlink/`..` escapes — resolve *then* prefix-check).
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(r),
        _ => false,
    }
}

/// The 2.1.x subagents directory for a main transcript: `<dir>/<stem>/subagents`,
/// where `<stem>` is the main file's name without `.jsonl` (the session uuid).
pub fn subagents_dir(main_path: &Path) -> PathBuf {
    let stem = main_path.file_stem().unwrap_or_default();
    let parent = main_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(stem).join("subagents")
}

/// The legacy sibling transcript path for a given agent id:
/// `<dir>/agent-<agentId>.jsonl` next to the main transcript.
fn legacy_sibling(main_path: &Path, agent_id: &str) -> PathBuf {
    let parent = main_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!("agent-{agent_id}.jsonl"))
}

/// Discover this session's subagent transcript files, safety-checked and
/// count-capped. Layout is auto-detected: if the 2.1.x `subagents/` directory
/// exists we list it; otherwise we resolve legacy siblings from the spawns'
/// agent ids. Returns only existing regular-file paths that resolve INSIDE the
/// session's own directory tree (ADR A9 symlink/`..` guard).
pub fn discover_subagent_paths(main_path: &Path, spawns: &[Spawn]) -> Vec<PathBuf> {
    let dir = subagents_dir(main_path);
    let mut out: Vec<PathBuf> = if dir.is_dir() {
        // 2.1.x: list agent-*.jsonl in the session-namespaced directory. The
        // directory itself guarantees these belong to this session, so no
        // spawn-linking is needed here (content validation happens at read).
        let root = main_path.parent().unwrap_or_else(|| Path::new("."));
        let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| is_agent_jsonl(p))
                    .filter(|p| is_within(root, p))
                    .collect()
            })
            .unwrap_or_default();
        // Deterministic order regardless of filesystem enumeration.
        v.sort();
        v
    } else {
        // Legacy: resolve exactly the siblings our own spawns name. Never list
        // the shared project dir — that would false-merge other sessions.
        let root = main_path.parent().unwrap_or_else(|| Path::new("."));
        spawns
            .iter()
            .filter_map(|s| s.agent_id.as_deref())
            .map(|id| legacy_sibling(main_path, id))
            .filter(|p| p.is_file() && is_within(root, p))
            .collect()
    };
    out.truncate(MAX_SUBAGENT_FILES);
    out
}

/// True for a regular file named `agent-*.jsonl`.
fn is_agent_jsonl(p: &Path) -> bool {
    p.is_file()
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_uuid_parses_traversal_rejected() {
        assert!(SessionId::parse("5717aaaa-1111-2222-3333-444455556666").is_some());
        assert!(SessionId::parse("../../../../etc/passwd").is_none());
        assert!(SessionId::parse("not-a-uuid").is_none());
        assert!(SessionId::parse("").is_none());
    }

    #[test]
    fn project_dir_name_dashifies_the_path() {
        let name = project_dir_name(Path::new("/Users/dev/Desktop/example-app"));
        assert_eq!(name, "-Users-dev-Desktop-example-app");
    }

    use crate::model::Spawn;

    #[test]
    fn discovers_2_1_x_subagents_dir() {
        // Layout: <dir>/<uuid>.jsonl (main) + <dir>/<uuid>/subagents/agent-*.jsonl
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        std::fs::write(subs.join("agent-aaa.jsonl"), "{}").unwrap();
        std::fs::write(subs.join("agent-bbb.jsonl"), "{}").unwrap();
        std::fs::write(subs.join("notes.txt"), "ignore me").unwrap();

        let found = discover_subagent_paths(&main, &[]);
        assert_eq!(found.len(), 2, "two agent-*.jsonl, notes.txt ignored");
    }

    #[test]
    fn discovers_legacy_siblings_by_spawn_agent_id() {
        // Layout: <dir>/<uuid>.jsonl (main) + <dir>/agent-<id>.jsonl (siblings),
        // no <uuid>/subagents dir → legacy path.
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        std::fs::write(td.path().join("agent-present.jsonl"), "{}").unwrap();
        // A decoy sibling for a DIFFERENT session's agent — must not be found.
        std::fs::write(td.path().join("agent-decoy.jsonl"), "{}").unwrap();

        let spawns = vec![
            Spawn { agent_id: Some("present".into()) },
            Spawn { agent_id: Some("absent".into()) }, // file does not exist
            Spawn { agent_id: None },                  // unresolved, skipped
        ];
        let found = discover_subagent_paths(&main, &spawns);
        assert_eq!(found.len(), 1, "only the spawn-linked, existing sibling");
        assert!(found[0].ends_with("agent-present.jsonl"));
    }

    #[test]
    fn file_count_is_capped() {
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        let subs = td.path().join(uuid).join("subagents");
        std::fs::create_dir_all(&subs).unwrap();
        for i in 0..(MAX_SUBAGENT_FILES + 10) {
            std::fs::write(subs.join(format!("agent-{i:03}.jsonl")), "{}").unwrap();
        }
        let found = discover_subagent_paths(&main, &[]);
        assert_eq!(found.len(), MAX_SUBAGENT_FILES, "capped");
    }
}
