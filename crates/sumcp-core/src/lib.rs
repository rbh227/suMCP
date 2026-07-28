#![warn(missing_docs)]
//! Deterministic session-forensics core for suMCP.
//!
//! Pipeline: locate → ingest → model → signals → score → Report (SPEC §4).
//! This crate is synchronous and pure — no I/O below `ingest`, no async
//! runtime (ADR A2). Signals are pure functions `&Session -> Vec<Finding>`,
//! and every finding carries the action indices proving it.

pub mod assemble;
pub mod file_class;
pub mod html;
pub mod ingest;
pub mod locate;
pub mod merge;
pub mod model;
pub mod payloads;
pub mod redact;
pub mod report;
pub mod review;
pub mod score;
pub mod signals;
pub mod work_unit;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Parse a transcript file into an [`report::Overview`].
///
/// Reads the file as the main lane. Returns an `io::Error` only if the file
/// cannot be read — parsing itself never fails (bad lines are counted).
pub fn overview_of_file(path: &Path) -> std::io::Result<report::Overview> {
    // `?` propagates the io::Error to the caller if the read fails; on success
    // it unwraps the String and execution continues.
    let raw = std::fs::read_to_string(path)?;
    let session = ingest::ingest_str(&raw, model::Lane::Main);
    Ok(report::Overview::from_session(&session))
}

/// The user's home directory, shared by both binaries (the CLI's install
/// path and its transcript-recency lookup, the MCP server's transcript and
/// stale-config lookups).
///
/// Prefers `$HOME` when it is set and non-empty, so Unix, WSL, and git-bash
/// all keep resolving exactly as before this helper existed. Only when `HOME`
/// is unset or empty does it fall back to `%USERPROFILE%`, which is what
/// native Windows sets instead. Neither set (or both empty) is `None`: we
/// never guess a home directory.
///
/// This reads two environment variables and nothing else, so it stays inside
/// `sumcp-core`'s no-I/O-below-ingest discipline (env lookups, not filesystem
/// access).
pub fn home_dir() -> Option<PathBuf> {
    home_dir_from(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

/// The pure core of [`home_dir`] (env-free, so tests can drive it).
fn home_dir_from(home: Option<OsString>, userprofile: Option<OsString>) -> Option<PathBuf> {
    home.filter(|h| !h.is_empty())
        .or_else(|| userprofile.filter(|u| !u.is_empty()))
        .map(PathBuf::from)
}

#[cfg(test)]
mod home_dir_tests {
    use super::*;

    #[test]
    fn prefers_home_when_set_and_nonempty() {
        assert_eq!(
            home_dir_from(Some("/Users/dev".into()), Some(r"C:\Users\dev".into())),
            Some(PathBuf::from("/Users/dev"))
        );
    }

    #[test]
    fn falls_back_to_userprofile_when_home_unset() {
        assert_eq!(
            home_dir_from(None, Some(r"C:\Users\dev".into())),
            Some(PathBuf::from(r"C:\Users\dev"))
        );
    }

    #[test]
    fn falls_back_to_userprofile_when_home_empty() {
        assert_eq!(
            home_dir_from(Some("".into()), Some(r"C:\Users\dev".into())),
            Some(PathBuf::from(r"C:\Users\dev"))
        );
    }

    #[test]
    fn empty_userprofile_does_not_count_either() {
        assert_eq!(home_dir_from(Some("".into()), Some("".into())), None);
    }

    #[test]
    fn neither_set_is_none() {
        assert_eq!(home_dir_from(None, None), None);
    }
}
