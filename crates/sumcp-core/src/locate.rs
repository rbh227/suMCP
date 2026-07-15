//! Locating transcripts on disk — with the ADR A9 input-safety boundary.
//!
//! Claude Code stores transcripts at
//! `~/.claude/projects/<dashified-cwd>/<session-id>.jsonl`. Untrusted callers
//! can pass a `session_id`; this module validates it *before* it ever touches
//! the filesystem, so `../../etc/passwd` can never become a path.

use std::path::{Path, PathBuf};

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
}
