//! Commit range to time window. The only part of this crate that shells out.
//!
//! Kept in its own module so the no-I/O-below-ingest rule (ADR A2) stays
//! visibly intact: nothing in `signals/` or `context.rs` may call this.

use std::path::Path;
use std::process::Command;

/// The time window a commit range covers, as Unix epoch seconds.
///
/// Returns `(start, end)`. Both are plain Unix epoch seconds, read straight
/// from `git log --format=%ct` rather than formatted as a string, and never
/// converted back to one: `git log --format=%cI` prints the committer date
/// in the LOCAL timezone of whoever made the commit
/// (`2026-08-11T15:41:52-04:00`), while transcript timestamps are UTC with
/// milliseconds (`2026-07-20T06:45:15.715Z`). Those two strings are not
/// comparable by plain lexical ordering: a commit at `15:41:52-04:00` is
/// `19:41:52Z`, so a transcript event at `18:00:00.000Z` is chronologically
/// earlier but lexically greater than the commit string. `%ct` has no
/// timezone or formatting ambiguity at all, so epoch seconds are the only
/// thing this module hands back. A caller compares them against a
/// transcript timestamp by first converting the transcript timestamp with
/// `work_unit::to_epoch_secs`, not by comparing strings.
///
/// The window is **(time the range's starting commit landed, time the
/// range's newest commit landed]**: it opens when the *previous* commit
/// landed and closes when the *last* commit in the range landed. A commit's
/// own timestamp is when the work behind it finished, so nearly all the
/// transcript activity that produced it happened *before* that instant.
/// `A..B` deliberately excludes `A` itself, so if the window were built only
/// from what `git log A..B` prints, a one-commit range (e.g. `HEAD~1..HEAD`)
/// would return the same instant for both bounds and the window would cover
/// almost none of the work that actually produced that commit. `A`'s own
/// commit time is therefore resolved separately (`git log -1 --format=%ct
/// A`, since `A..B` will not print it) and used as the window's start in
/// place of the oldest commit inside the range.
///
/// If `range` has no `..` (a bare `HEAD`, or any single revision, meaning
/// "everything reachable from here"), there is no commit outside the range
/// to treat as "the previous one". The window then starts at the oldest
/// commit *inside* the range instead. This is deliberate, documented
/// behavior, not a value falling through unnoticed: a bare revision already
/// means "everything up to here", so the oldest commit found in it really is
/// the first thing in scope.
///
/// Errors rather than guessing: when git rejects the range, when `range`
/// looks like a command-line option instead of a revision, or when any line
/// of git's output does not parse as an epoch integer. A guessed window
/// would silently select the wrong sessions, which is worse than failing.
pub fn range_window(repo: &Path, range: &str) -> std::io::Result<(i64, i64)> {
    reject_option_like(range)?;

    let times = commit_epoch_seconds(repo, range)?;
    let Some(newest) = times.iter().copied().max() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no commits in range '{range}'"),
        ));
    };
    let oldest_in_range = times
        .iter()
        .copied()
        .min()
        .expect("non-empty: newest was found above");

    let start = match range_start_ref(range) {
        Some(start_ref) => {
            reject_option_like(start_ref)?;
            resolve_single_commit_time(repo, start_ref)?
        }
        None => oldest_in_range,
    };

    Ok((start, newest))
}

/// Reject anything that looks like a command-line option rather than a
/// revision or range, before git ever runs.
///
/// `--end-of-options` (used below) stops git from *interpreting* a
/// dash-prefixed argument as a flag, but an option-shaped string is still
/// wrong to accept here: it is not a revision, and letting it through only
/// to have git fail on it later would depend on `--end-of-options` being
/// present on every call site forever. Rejecting the shape up front is the
/// one place this invariant has to hold.
fn reject_option_like(s: &str) -> std::io::Result<()> {
    if s.starts_with('-') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("range '{s}' looks like a command-line option, not a revision"),
        ));
    }
    Ok(())
}

/// If `range` is a two- or three-dot range (`A..B` or `A...B`), the ref
/// naming its starting point (`A`). `None` for a bare revision, which has no
/// starting point outside itself, or for a range where the left side was
/// left empty (`..B`), which has no separate starting commit to resolve.
fn range_start_ref(range: &str) -> Option<&str> {
    let idx = range.find("..")?;
    let start = &range[..idx];
    if start.is_empty() { None } else { Some(start) }
}

/// All commit times in `range`, as Unix epoch seconds, in whatever order git
/// happens to print them. Order is never trusted for meaning (see
/// `range_window`'s doc); only the parsed values are used, via `min`/`max`.
fn commit_epoch_seconds(repo: &Path, range: &str) -> std::io::Result<Vec<i64>> {
    let out = Command::new("git")
        .args(["log", "--format=%ct", "--end-of-options", range])
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("git log rejected the range '{range}'"),
        ));
    }
    parse_epoch_lines(&out.stdout, range)
}

/// The commit time of a single revision, as Unix epoch seconds.
fn resolve_single_commit_time(repo: &Path, rev: &str) -> std::io::Result<i64> {
    let out = Command::new("git")
        .args(["log", "-1", "--format=%ct", "--end-of-options", rev])
        .current_dir(repo)
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("git log could not resolve '{rev}'"),
        ));
    }
    let times = parse_epoch_lines(&out.stdout, rev)?;
    times.into_iter().next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no commit found for '{rev}'"),
        )
    })
}

/// Parse each non-empty line of `stdout` as an epoch-second integer,
/// erroring on the first line that does not parse. This is what makes it
/// safe to trust the output at all: a hostile or mistaken `--format` cannot
/// silently produce a value this module accepts as a time.
fn parse_epoch_lines(stdout: &[u8], range: &str) -> std::io::Result<Vec<i64>> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            l.parse::<i64>().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("git log for '{range}' printed a non-numeric line: '{l}'"),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_unit::to_epoch_secs;

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
    fn a_bare_rev_starts_at_the_oldest_commit_it_reaches() {
        // No `..`, so there is no commit outside the range to treat as "the
        // previous one": the window starts at the oldest commit inside it.
        let dir = fixture_repo();
        let (start, end) = range_window(dir.path(), "HEAD").unwrap();
        let first = to_epoch_secs("2026-01-01T10:00:00Z").unwrap();
        let second = to_epoch_secs("2026-01-02T15:30:00Z").unwrap();
        assert_eq!(start, first, "oldest reachable commit");
        assert_eq!(end, second, "newest reachable commit");
    }

    #[test]
    fn a_dot_dot_range_starts_at_the_left_sides_own_time_not_the_range() {
        // `HEAD~1..HEAD` excludes `HEAD~1` from what `git log` prints for the
        // range, so if the window were built only from that output, both
        // bounds would collapse to HEAD's own timestamp. The window's start
        // must instead be resolved from `HEAD~1` directly, and must differ
        // from the end.
        let dir = fixture_repo();
        let (start, end) = range_window(dir.path(), "HEAD~1..HEAD").unwrap();
        let first = to_epoch_secs("2026-01-01T10:00:00Z").unwrap();
        let second = to_epoch_secs("2026-01-02T15:30:00Z").unwrap();
        assert_eq!(start, first, "window opens when the PREVIOUS commit landed");
        assert_eq!(
            end, second,
            "window closes when the range's own commit landed"
        );
        assert_ne!(
            start, end,
            "a one-commit range must not collapse to an instant"
        );
    }

    #[test]
    fn a_bad_range_is_an_error_not_a_guess() {
        // Guessing a window from a range git rejected would silently analyze
        // the wrong sessions, which is worse than failing.
        let dir = fixture_repo();
        assert!(range_window(dir.path(), "no-such-ref..HEAD").is_err());
    }

    #[test]
    fn an_option_shaped_range_is_rejected_before_reaching_git() {
        // Without this guard, `--reverse` in range position succeeds and
        // reverses commit order, silently inverting the returned bounds.
        let dir = fixture_repo();
        let err = range_window(dir.path(), "--reverse").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_format_override_range_is_rejected_before_reaching_git() {
        // Without this guard, `--format=format:NOT_A_TIME` in range position
        // succeeds and returns a non-time string as both bounds.
        let dir = fixture_repo();
        let err = range_window(dir.path(), "--format=format:NOT_A_TIME").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
