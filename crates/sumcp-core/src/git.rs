//! Commit range to time window. The only part of this crate that shells out.
//!
//! Kept in its own module so the no-I/O-below-ingest rule (ADR A2) stays
//! visibly intact: nothing in `signals/` or `context.rs` may call this.

use std::path::Path;
use std::process::Command;

/// The commit times bounding a range, oldest first, as ISO 8601 strings.
///
/// Returns `(oldest, newest)`. Both are in the strict ISO form
/// (`2026-01-02T15:30:00Z`) that transcript timestamps use, so a caller can
/// compare them against `effective_ts` with plain string ordering, which is
/// how every other time comparison in this crate already works.
///
/// Errors rather than guessing when git rejects the range: a guessed window
/// would silently select the wrong sessions, which is worse than failing.
pub fn range_window(repo: &Path, range: &str) -> std::io::Result<(String, String)> {
    let out = Command::new("git")
        .args(["log", "--format=%cI", range])
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("git log rejected the range '{range}'"),
        ));
    }
    // `git log` prints newest first, so the last line is the oldest commit.
    let times: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    let (Some(newest), Some(oldest)) = (times.first(), times.last()) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no commits in range '{range}'"),
        ));
    };
    Ok((oldest.clone(), newest.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a throwaway repo with two commits at known times.
    fn fixture_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let run = |args: &[&str], env: &[(&str, &str)]| {
            let mut c = Command::new("git");
            c.args(args).current_dir(p);
            for (k, v) in env {
                c.env(k, v);
            }
            let out = c.output().unwrap();
            assert!(out.status.success(), "git {args:?} failed: {out:?}");
        };
        run(&["init", "-q"], &[]);
        run(&["config", "user.email", "t@example.com"], &[]);
        run(&["config", "user.name", "t"], &[]);
        std::fs::write(p.join("a.txt"), "one").unwrap();
        run(&["add", "."], &[]);
        run(
            &["commit", "-q", "-m", "first"],
            &[
                ("GIT_AUTHOR_DATE", "2026-01-01T10:00:00Z"),
                ("GIT_COMMITTER_DATE", "2026-01-01T10:00:00Z"),
            ],
        );
        std::fs::write(p.join("a.txt"), "two").unwrap();
        run(&["add", "."], &[]);
        run(
            &["commit", "-q", "-m", "second"],
            &[
                ("GIT_AUTHOR_DATE", "2026-01-02T15:30:00Z"),
                ("GIT_COMMITTER_DATE", "2026-01-02T15:30:00Z"),
            ],
        );
        dir
    }

    #[test]
    fn range_window_returns_oldest_and_newest_commit_times() {
        let dir = fixture_repo();
        let (from, to) = range_window(dir.path(), "HEAD~1..HEAD").unwrap();
        assert!(from.starts_with("2026-01-02"), "got {from}");
        assert!(to.starts_with("2026-01-02"), "got {to}");
    }

    #[test]
    fn a_two_commit_range_spans_both_times() {
        // `HEAD~2..HEAD` (as originally drafted) does not resolve: the
        // fixture's first commit has no parent, so `HEAD~2` names a commit
        // that does not exist and git rejects the range outright. A bare
        // revision like `HEAD` means "everything reachable from here",
        // which covers both commits and is what "spans both times" needs.
        let dir = fixture_repo();
        let (from, to) = range_window(dir.path(), "HEAD").unwrap();
        assert!(from.starts_with("2026-01-01"), "oldest first: got {from}");
        assert!(to.starts_with("2026-01-02"), "newest last: got {to}");
    }

    #[test]
    fn a_bad_range_is_an_error_not_a_guess() {
        // Guessing a window from a range git rejected would silently analyze
        // the wrong sessions, which is worse than failing.
        let dir = fixture_repo();
        assert!(range_window(dir.path(), "no-such-ref..HEAD").is_err());
    }
}
