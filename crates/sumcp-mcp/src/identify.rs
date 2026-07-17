//! Fail-closed session self-identification (ADR A4).
//!
//! Which session is calling us? The calling session has already appended the
//! very `tool_use` that triggered this call to its own transcript, so the
//! transcript whose tail contains our forwarded `tool_use` id (Claude Code
//! sends it as `_meta["claudecode/toolUseId"]`) IS the caller — the id is
//! unique by construction, so a match is proof, not inference.
//!
//! Exactly one match → verified (`identified_by: "tool_use_id"`). No
//! forwarded id, zero matches, or several matches → we REFUSE to guess and
//! return the `ambiguous_session` error payload listing candidates. There is
//! deliberately no weaker fallback (a bare tool-name scan was reviewed out:
//! transcript *content* — a grep result, a quoted doc — can contain any
//! marker text, and a plausible-but-wrong debrief is fatal for an honesty
//! tool). Recency inference exists only in the CLI's explicit `latest` mode,
//! never here.

use serde::Serialize;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use sumcp_core::locate::{SessionId, is_within};

/// Only this much of each file's end is searched for the marker. The caller's
/// `tool_use` was appended moments ago, so it lives in the tail; bounding the
/// read keeps the scan O(files), not O(bytes-on-disk).
const TAIL_BYTES: u64 = 64 * 1024;
/// Newest-first cap on how many transcripts one scan will open.
const MAX_SCAN_FILES: usize = 20;
/// Cap on candidates listed in the ambiguous_session error payload.
const MAX_CANDIDATES: usize = 5;

/// A successfully identified session.
#[derive(Debug)]
pub struct Resolved {
    /// Transcript path.
    pub path: PathBuf,
    /// Session id (the file stem, already validated).
    pub id: String,
    /// ADR A4 provenance: `"explicit"` or `"tool_use_id"`.
    pub identified_by: &'static str,
}

/// One entry in the fail-closed error payload's candidate list.
#[derive(Debug, Serialize)]
pub struct Candidate {
    /// Session id.
    pub id: String,
    /// Last-modified time, RFC 3339 UTC.
    pub mtime: String,
    /// Whether the transcript lives under the project dir for our cwd
    /// (always true today — we only scan that one dir).
    pub cwd_match: bool,
}

/// Why identification failed. The server maps each variant to a tool-level
/// error payload; none of them ever becomes a guess.
#[derive(Debug)]
pub enum IdentifyError {
    /// `session_id` didn't match the 36-char UUID shape (ADR A9: rejected
    /// before it ever touches the filesystem).
    InvalidId(String),
    /// Valid-shaped id, but no such transcript in this project.
    NotFound(String),
    /// Zero or multiple verified matches — the fail-closed case.
    Ambiguous(Vec<Candidate>),
}

/// Resolve an explicitly passed `session_id` (provenance `"explicit"`).
pub fn resolve_explicit(project_dir: &Path, raw_id: &str) -> Result<Resolved, IdentifyError> {
    let Some(id) = SessionId::parse(raw_id) else {
        return Err(IdentifyError::InvalidId(raw_id.to_string()));
    };
    let path = project_dir.join(format!("{}.jsonl", id.as_str()));
    // The id shape blocks `../` traversal, but `is_file()` + read still
    // *follow symlinks* — a planted `<uuid>.jsonl → ~/.ssh/id_rsa` link would
    // escape. ADR A9(1): resolve, then prefix-check against the project dir.
    if !path.is_file() || !is_within(project_dir, &path) {
        return Err(IdentifyError::NotFound(id.as_str().to_string()));
    }
    Ok(Resolved {
        path,
        id: id.as_str().to_string(),
        identified_by: "explicit",
    })
}

/// The validated session id a transcript path carries as its file stem.
pub fn stem_id(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// All `<uuid>.jsonl` transcripts in the project dir, newest mtime first.
fn transcripts_newest_first(project_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, SystemTime)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            // Stem must be a valid session id — anything else in the dir
            // (agent sidechains, stray files) is not a session we can name.
            let stem = path.file_stem()?.to_str()?;
            if path.extension()?.to_str()? != "jsonl" || SessionId::parse(stem).is_none() {
                return None;
            }
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((path, mtime))
        })
        .collect();
    // Newest first; `sort_by_key` + `Reverse` is the idiom for descending.
    files.sort_by_key(|(_, m)| std::cmp::Reverse(*m));
    files
}

/// Scan transcript tails for `marker` (the forwarded tool_use id). Returns
/// every matching path — the caller judges 0/1/many; only exactly-one is
/// ever treated as verified.
pub fn scan_for_marker(project_dir: &Path, marker: &str) -> Vec<PathBuf> {
    transcripts_newest_first(project_dir)
        .into_iter()
        .take(MAX_SCAN_FILES)
        // Same A9(1) boundary as the explicit path: a symlinked entry that
        // canonicalizes outside the project dir is not a session of ours.
        .filter(|(path, _)| is_within(project_dir, path))
        .filter(|(path, _)| tail_contains(path, marker))
        .map(|(path, _)| path)
        .collect()
}

/// Does the last `TAIL_BYTES` of `path` contain `marker`?
fn tail_contains(path: &Path, marker: &str) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return false;
    };
    // `saturating_sub` clamps at zero instead of underflowing for small files.
    if f.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))
        .is_err()
    {
        return false;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES.min(len) as usize);
    if f.read_to_end(&mut buf).is_err() {
        return false;
    }
    // Transcripts are UTF-8, but a tail cut mid-codepoint would break
    // `from_utf8` — lossy conversion sidesteps that (the marker is ASCII).
    String::from_utf8_lossy(&buf).contains(marker)
}

/// Recent sessions for the ambiguous_session payload, newest first.
pub fn recent_candidates(project_dir: &Path) -> Vec<Candidate> {
    transcripts_newest_first(project_dir)
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(path, mtime)| Candidate {
            id: stem_id(&path),
            mtime: rfc3339_utc(mtime),
            cwd_match: true,
        })
        .collect()
}

/// The fail-closed error payload (frozen shape, `docs/payload-schema.md`).
pub fn ambiguous_payload(candidates: &[Candidate]) -> serde_json::Value {
    serde_json::json!({
        "v": 0,
        "error": "ambiguous_session",
        "message": "Could not verify the calling session (own tool_use id not found after bounded retry). Refusing to guess.",
        "candidates": candidates,
        "hint": "Pass session_id explicitly, e.g. session_overview(session_id=\"5717aaaa-...\")."
    })
}

/// Format a `SystemTime` as RFC 3339 UTC (`2026-07-15T16:04:12Z`) without a
/// date-time dependency: days-since-epoch → civil date (Howard Hinnant's
/// `civil_from_days` algorithm), seconds-of-day → clock.
pub fn rfc3339_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0); // pre-1970 mtimes collapse to the epoch — fine here
    let days = (secs / 86_400) as i64;
    let (h, m, s) = {
        let rem = secs % 86_400;
        (rem / 3600, (rem % 3600) / 60, rem % 60)
    };
    // civil_from_days: shift epoch to 0000-03-01 so leap days land at the
    // end of the cycle, decompose into 400-year eras, then back.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const ID_A: &str = "5717aaaa-1111-2222-3333-444455556666";
    const ID_B: &str = "80b9a169-624f-4880-a2c3-24b96e2b4ea2";

    fn write_transcript(dir: &Path, id: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn explicit_valid_id_resolves_with_explicit_provenance() {
        let dir = tempfile::tempdir().unwrap();
        write_transcript(dir.path(), ID_A, "{}");
        let r = resolve_explicit(dir.path(), ID_A).unwrap();
        assert_eq!(r.identified_by, "explicit");
        assert_eq!(r.id, ID_A);
        assert!(r.path.ends_with(format!("{ID_A}.jsonl")));
    }

    #[test]
    fn explicit_traversal_is_rejected_before_touching_fs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_explicit(dir.path(), "../../../../etc/passwd"),
            Err(IdentifyError::InvalidId(_))
        ));
    }

    #[test]
    fn explicit_unknown_id_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            resolve_explicit(dir.path(), ID_A),
            Err(IdentifyError::NotFound(_))
        ));
    }

    #[test]
    fn marker_in_exactly_one_tail_is_found() {
        let dir = tempfile::tempdir().unwrap();
        write_transcript(dir.path(), ID_A, r#"{"x":"toolu_MARKER123"}"#);
        write_transcript(dir.path(), ID_B, r#"{"x":"nothing here"}"#);
        let hits = scan_for_marker(dir.path(), "toolu_MARKER123");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].ends_with(format!("{ID_A}.jsonl")));
    }

    #[test]
    fn marker_in_two_tails_returns_both_for_fail_closed_handling() {
        let dir = tempfile::tempdir().unwrap();
        write_transcript(dir.path(), ID_A, r#"{"x":"toolu_DUPED"}"#);
        write_transcript(dir.path(), ID_B, r#"{"y":"toolu_DUPED"}"#);
        let hits = scan_for_marker(dir.path(), "toolu_DUPED");
        assert_eq!(
            hits.len(),
            2,
            "ambiguity must be visible, never resolved by guessing"
        );
    }

    #[test]
    fn marker_outside_the_64k_tail_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        // marker first, then >64 KiB of padding pushes it out of the tail.
        let body = format!("toolu_EARLY{}", "x".repeat((TAIL_BYTES + 1024) as usize));
        write_transcript(dir.path(), ID_A, &body);
        assert!(scan_for_marker(dir.path(), "toolu_EARLY").is_empty());
    }

    #[test]
    fn non_session_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.jsonl"), "toolu_X").unwrap();
        std::fs::write(dir.path().join(format!("{ID_A}.txt")), "toolu_X").unwrap();
        assert!(scan_for_marker(dir.path(), "toolu_X").is_empty());
        assert!(recent_candidates(dir.path()).is_empty());
    }

    #[test]
    fn symlinked_transcript_escaping_the_project_dir_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        // The "secret" lives OUTSIDE the project dir; an attacker plants a
        // valid-uuid-named symlink to it inside (ADR A9's named scenario).
        let secret = root.path().join("id_rsa");
        std::fs::write(&secret, "toolu_SECRET private bits").unwrap();
        std::os::unix::fs::symlink(&secret, project.join(format!("{ID_A}.jsonl"))).unwrap();

        assert!(matches!(
            resolve_explicit(&project, ID_A),
            Err(IdentifyError::NotFound(_))
        ));
        assert!(scan_for_marker(&project, "toolu_SECRET").is_empty());
    }

    #[test]
    fn ambiguous_payload_matches_the_frozen_error_shape() {
        let dir = tempfile::tempdir().unwrap();
        write_transcript(dir.path(), ID_A, "{}");
        write_transcript(dir.path(), ID_B, "{}");
        let p = ambiguous_payload(&recent_candidates(dir.path()));
        assert_eq!(p["v"], 0);
        assert_eq!(p["error"], "ambiguous_session");
        assert!(p["message"].is_string() && p["hint"].is_string());
        let cands = p["candidates"].as_array().unwrap();
        assert_eq!(cands.len(), 2);
        for c in cands {
            assert!(c["id"].is_string());
            assert_eq!(c["cwd_match"], true);
            // RFC 3339 shape: 20 chars, T at 10, Z at the end.
            let m = c["mtime"].as_str().unwrap();
            assert_eq!(m.len(), 20);
            assert_eq!(&m[10..11], "T");
            assert!(m.ends_with('Z'));
        }
    }

    #[test]
    fn rfc3339_known_values() {
        assert_eq!(rfc3339_utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 2026-07-15T16:04:12Z == 1784131452 (leap-year path exercised: 2024)
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_131_452);
        assert_eq!(rfc3339_utc(t), "2026-07-15T16:04:12Z");
    }
}
