//! Locating transcripts on disk — with the ADR A9 input-safety boundary.
//!
//! Claude Code stores transcripts at
//! `~/.claude/projects/<dashified-cwd>/<session-id>.jsonl`. Untrusted callers
//! can pass a `session_id`; this module validates it *before* it ever touches
//! the filesystem, so `../../etc/passwd` can never become a path.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// Every `<uuid>.jsonl` transcript in a project dir, newest mtime first.
///
/// Returns the mtime alongside each path because callers that report
/// candidates need to show it, and re-`stat`ing the file later could observe a
/// different value than the one we sorted on. An unreadable directory (never
/// opened in Claude Code, no permission) is an empty list, not an error: "no
/// sessions here" is a normal answer, and the mtime of a file we cannot stat
/// is unknowable, so those entries drop out too.
pub fn transcripts_newest_first(project_dir: &Path) -> Vec<(PathBuf, SystemTime)> {
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

/// The most recently modified transcript in a project dir, if any.
///
/// This is recency *inference*: it answers "the session I was last working in
/// here", which is exactly what a human at a terminal means by a bare `sumcp`.
/// The MCP server must never use it (ADR A4: a plausible-but-wrong debrief is
/// fatal), which is why nothing here claims verification — the caller is
/// expected to label the result as recency-derived provenance.
pub fn newest_transcript(project_dir: &Path) -> Option<PathBuf> {
    transcripts_newest_first(project_dir)
        .into_iter()
        // ADR A9(1): the uuid-shaped name blocks traversal, but reading still
        // follows symlinks — a planted `<uuid>.jsonl → ~/.ssh/id_rsa` with a
        // fresh mtime would otherwise be "the newest session". Resolve, then
        // prefix-check, and fall through to the next-newest real transcript.
        .find(|(path, _)| is_within(project_dir, path))
        .map(|(path, _)| path)
}

/// How much of a transcript's head and tail we read to find its time span.
/// 256 KB each end is far more than enough: transcript lines are rarely over
/// a few KB, so this always covers hundreds of lines at each end, while a
/// whole-file read of the largest observed transcript would be 8 MB.
const SPAN_PROBE_BYTES: u64 = 256 * 1024;

/// The time a transcript covers: its first and last timestamp, as the raw
/// RFC 3339 strings found in the file.
///
/// These are compared as STRINGS everywhere, not parsed into a date type.
/// That is safe because Claude Code writes fixed-width UTC timestamps ending
/// in `Z`, so lexical order and chronological order are the same. `merge.rs`
/// already sorts the whole action stream on that assumption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptSpan {
    /// Earliest timestamp seen.
    pub first: String,
    /// Latest timestamp seen.
    pub last: String,
}

/// Pull every `"timestamp":"..."` value out of a chunk of transcript text.
///
/// This deliberately does NOT parse JSON. Scanning for the key is roughly an
/// order of magnitude cheaper than `serde_json::from_str` per line, and a
/// span only needs the min and max, so a stray match inside some other
/// string value would have to be a plausible timestamp to matter at all.
fn scan_timestamps(chunk: &str) -> Vec<String> {
    const KEY: &str = "\"timestamp\":\"";
    let mut out = Vec::new();
    // `match_indices` walks every occurrence of KEY and hands us its byte
    // offset. `start + KEY.len()` is then the first character of the value.
    for (at, _) in chunk.match_indices(KEY) {
        let rest = &chunk[at + KEY.len()..];
        // The value runs to the next quote. `find` returns a byte offset
        // relative to `rest`, or None if the chunk was cut mid-value (which
        // is expected at a probe boundary, and simply skipped).
        if let Some(end) = rest.find('"') {
            let v = &rest[..end];
            // Cheap sanity filter: real timestamps look like
            // `2026-01-01T00:00:01...Z`. Anything else is not a timestamp.
            if v.len() >= 20 && v.as_bytes()[10] == b'T' {
                out.push(v.to_string());
            }
        }
    }
    out
}

/// The time span a transcript covers, or `None` if it has no timestamps.
///
/// Reads at most `SPAN_PROBE_BYTES` from each end of the file rather than
/// the whole file, keeping memory use bounded regardless of transcript size.
pub fn transcript_span(path: &Path) -> Option<TranscriptSpan> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    let mut f = std::fs::File::open(path).ok()?;
    transcript_span_from_reader(&mut f, len)
}

/// Does the actual work of [`transcript_span`], but over any `Read + Seek`
/// instead of opening the file itself.
///
/// This split exists purely for testability. The public function's signature
/// (`&Path -> Option<TranscriptSpan>`) cannot be probed from a test without
/// either touching real disk I/O (which cannot tell you how many bytes were
/// read) or pulling in a mocking crate (which this project does not depend
/// on). Taking the "seam" out into its own function that accepts a generic
/// `R: Read + Seek` lets a test hand in something that looks like a file to
/// this function but also counts bytes on the side. See `CountingReader` in
/// the test module below.
///
/// The head buffer and the tail buffer are both ordinary locals: they are
/// both still alive at once in the moment just before the function returns
/// (that is the peak), and both go out of scope together when it does. So
/// peak memory here is 512 KB regardless of file size, and the two buffers
/// are not dropped one before the other; they drop together at return.
fn transcript_span_from_reader<R: std::io::Read + std::io::Seek>(
    f: &mut R,
    len: u64,
) -> Option<TranscriptSpan> {
    use std::io::SeekFrom;

    // Head probe.
    let mut head = vec![0u8; SPAN_PROBE_BYTES.min(len) as usize];
    f.read_exact(&mut head).ok()?;
    let mut stamps = scan_timestamps(&String::from_utf8_lossy(&head));

    // Tail probe, only when the file is bigger than one probe (otherwise the
    // head already covered the whole thing and a second read would duplicate).
    if len > SPAN_PROBE_BYTES {
        let from = len - SPAN_PROBE_BYTES;
        f.seek(SeekFrom::Start(from)).ok()?;
        let mut tail = vec![0u8; SPAN_PROBE_BYTES as usize];
        f.read_exact(&mut tail).ok()?;
        stamps.extend(scan_timestamps(&String::from_utf8_lossy(&tail)));
    }

    // `min`/`max` over the collected strings. `cloned()` copies the winners
    // out so we can return owned Strings after `stamps` is dropped.
    let first = stamps.iter().min().cloned()?;
    let last = stamps.iter().max().cloned()?;
    Some(TranscriptSpan { first, last })
}

/// Assert `candidate` resolves inside `root` after canonicalization (ADR A9:
/// reject symlink/`..` escapes — resolve *then* prefix-check).
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(r), Ok(c)) => c.starts_with(r),
        _ => false,
    }
}

/// True if `candidate` is inside `main_path`'s parent directory tree — the
/// guard assembly applies before reading any discovered subagent file.
pub fn is_within_or_root(main_path: &Path, candidate: &Path) -> bool {
    let root = main_path.parent().unwrap_or_else(|| Path::new("."));
    is_within(root, candidate)
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
    // Dedup: two spawns can name the same agentId, which would map to the same
    // sibling path twice — assembly would then read and merge that child
    // transcript twice, doubling its actions. Sort first so `dedup` (which only
    // removes CONSECUTIVE duplicates) is complete, and so the order stays
    // deterministic. The 2.1.x branch is already sorted and duplicate-free, so
    // this is a harmless no-op there.
    out.sort();
    out.dedup();
    out.truncate(MAX_SUBAGENT_FILES);
    out
}

/// True for a regular file named `agent-*.jsonl`.
///
/// `pub(crate)` because `work_unit::discover_work_unit` also needs this exact
/// shape, to skip a legacy-layout subagent sibling when it enumerates a
/// project directory (see the comment there). Sharing the one predicate
/// keeps the naming convention defined in a single place; two independent
/// copies of "what a subagent filename looks like" would drift the moment
/// one of them changed.
pub(crate) fn is_agent_jsonl(p: &Path) -> bool {
    p.is_file()
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("agent-") && n.ends_with(".jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a `<id>.jsonl` with an EXPLICIT mtime. Recency tests must not
    /// depend on how fast the test ran: two files written back-to-back can
    /// share an mtime on a coarse-grained filesystem, which would make the
    /// ordering assertion flaky. Stamping the time makes it deterministic.
    fn write_session(dir: &Path, id: &str, mtime_secs: u64) -> PathBuf {
        let path = dir.join(format!("{id}.jsonl"));
        std::fs::write(&path, "{}").unwrap();
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs),
        )
        .unwrap();
        path
    }

    #[test]
    fn newest_transcript_picks_the_latest_mtime() {
        let td = tempfile::tempdir().unwrap();
        write_session(td.path(), "5717aaaa-1111-2222-3333-444455556666", 1_000_000);
        let newest = write_session(td.path(), "80b9a169-624f-4880-a2c3-24b96e2b4ea2", 3_000_000);
        write_session(td.path(), "aaaabbbb-cccc-dddd-eeee-ffff00001111", 2_000_000);

        assert_eq!(newest_transcript(td.path()), Some(newest));
    }

    #[test]
    fn newest_transcript_ignores_non_session_files() {
        let td = tempfile::tempdir().unwrap();
        // Neither of these names a session: the first has no uuid stem, the
        // second is not a transcript. A wrong pick here would analyze garbage.
        std::fs::write(td.path().join("notes.jsonl"), "{}").unwrap();
        std::fs::write(
            td.path().join("5717aaaa-1111-2222-3333-444455556666.txt"),
            "{}",
        )
        .unwrap();

        assert_eq!(newest_transcript(td.path()), None);
    }

    #[test]
    fn newest_transcript_of_a_missing_dir_is_none() {
        let td = tempfile::tempdir().unwrap();
        // A project that has never been opened in Claude Code has no dir at
        // all; that must be an ordinary "nothing found", not a panic.
        assert_eq!(newest_transcript(&td.path().join("never-used")), None);
    }

    // Creating a symlink needs elevated privileges on Windows, so this test
    // cannot run there. The ADR A9(1) protection it exercises is NOT
    // Unix-only; `is_within`'s resolve-then-prefix-check applies on every
    // platform, this is just the only way we have to plant one in a test.
    #[cfg(unix)]
    #[test]
    fn newest_transcript_skips_a_symlink_escaping_the_project_dir() {
        // ADR A9(1): a planted `<uuid>.jsonl → ~/.ssh/id_rsa` symlink must not
        // become "the newest session", even though its mtime wins.
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let real = write_session(&project, "5717aaaa-1111-2222-3333-444455556666", 1_000_000);
        let secret = root.path().join("id_rsa");
        std::fs::write(&secret, "private bits").unwrap();
        let planted = project.join("80b9a169-624f-4880-a2c3-24b96e2b4ea2.jsonl");
        std::os::unix::fs::symlink(&secret, &planted).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&planted)
            .unwrap()
            .set_modified(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(9_000_000),
            )
            .unwrap();

        assert_eq!(newest_transcript(&project), Some(real));
    }

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
            Spawn {
                agent_id: Some("present".into()),
            },
            Spawn {
                agent_id: Some("absent".into()),
            }, // file does not exist
            Spawn { agent_id: None }, // unresolved, skipped
        ];
        let found = discover_subagent_paths(&main, &spawns);
        assert_eq!(found.len(), 1, "only the spawn-linked, existing sibling");
        assert!(found[0].ends_with("agent-present.jsonl"));
    }

    #[test]
    fn duplicate_spawn_agent_ids_yield_one_path() {
        // WHY: two spawns naming the SAME agentId map to the same sibling path.
        // Without dedup, assembly would read + merge that child transcript
        // twice, doubling its actions (inflating churn/re-read counts) while
        // files_missing still reads 0. The returned Vec must be duplicate-free.
        let td = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = td.path().join(format!("{uuid}.jsonl"));
        std::fs::write(&main, "{}").unwrap();
        std::fs::write(td.path().join("agent-dup.jsonl"), "{}").unwrap();

        let spawns = vec![
            Spawn {
                agent_id: Some("dup".into()),
            },
            Spawn {
                agent_id: Some("dup".into()),
            }, // same id → same path
        ];
        let found = discover_subagent_paths(&main, &spawns);
        assert_eq!(
            found.len(),
            1,
            "the shared sibling path is merged exactly once"
        );
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

    #[test]
    fn transcript_span_reads_first_and_last_timestamp() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("s.jsonl");
        // Three lines; the middle one has no timestamp, which is normal
        // (about 20% of real lines carry none).
        std::fs::write(
            &p,
            concat!(
                r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z"}"#,
                "\n",
                r#"{"type":"system"}"#,
                "\n",
                r#"{"type":"user","timestamp":"2026-01-01T05:30:00Z"}"#,
                "\n"
            ),
        )
        .unwrap();

        let span = transcript_span(&p).expect("a span");
        assert_eq!(span.first, "2026-01-01T00:00:01Z");
        assert_eq!(span.last, "2026-01-01T05:30:00Z");
    }

    #[test]
    fn transcript_span_is_none_without_timestamps() {
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("s.jsonl");
        std::fs::write(&p, "{\"type\":\"system\"}\n").unwrap();
        assert!(transcript_span(&p).is_none());
    }

    #[test]
    fn transcript_span_does_not_read_the_whole_file() {
        // WHY: this only checks CORRECTNESS on a large file: a 4 MB file
        // padded with untimestamped filler still resolves the right span,
        // because the head and tail probes carry the real timestamps. It
        // does NOT prove the function avoided reading the whole file: a
        // naive whole-file implementation would produce the identical span
        // and pass this test unchanged. The actual boundedness proof, which
        // watches bytes read, is
        // `transcript_span_reader_stays_within_the_probe_budget` below.
        let td = tempfile::tempdir().unwrap();
        let p = td.path().join("big.jsonl");
        let filler = format!("{}\n", r#"{"type":"system","pad":"xxxxxxxxxxxxxxxx"}"#);
        let mut body = String::new();
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z"}"#);
        body.push('\n');
        while body.len() < 4 * 1024 * 1024 {
            body.push_str(&filler);
        }
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T09:00:00Z"}"#);
        body.push('\n');
        std::fs::write(&p, &body).unwrap();

        let span = transcript_span(&p).expect("a span");
        assert_eq!(span.first, "2026-01-01T00:00:01Z");
        assert_eq!(span.last, "2026-01-01T09:00:00Z");
    }

    /// A fake "file" that lets a test observe how many bytes
    /// `transcript_span_from_reader` actually reads, which a real file
    /// cannot do on its own. This is the standard way to make I/O testable
    /// in Rust without a mocking framework: the function under test does not
    /// know or care whether it is talking to a real `std::fs::File` or to
    /// this struct, because both implement the same two traits, `Read` and
    /// `Seek`. All this wrapper adds on top of a normal in-memory buffer is
    /// a counter that grows every time `read` hands bytes back.
    struct CountingReader {
        // `Cursor<Vec<u8>>` already implements `Read` and `Seek` over an
        // in-memory byte buffer, tracking a "current position" the same way
        // a real file's cursor does. Wrapping it means we get correct seek
        // behavior for free and only have to write the counting part
        // ourselves.
        inner: std::io::Cursor<Vec<u8>>,
        // Running total of bytes returned across every call to `read`.
        // This is the number the test asserts on: it is the only way to
        // observe from OUTSIDE the function how much of the fake file it
        // touched.
        bytes_read: usize,
    }

    impl CountingReader {
        fn new(data: Vec<u8>) -> Self {
            CountingReader {
                inner: std::io::Cursor::new(data),
                bytes_read: 0,
            }
        }
    }

    // Implementing `Read` ourselves (instead of just using `Cursor`
    // directly) is what lets us intercept every read and add to the
    // counter. `Read` has one required method, `read`: it is handed an
    // empty-ish `buf` to fill, and must return how many bytes it actually
    // wrote into `buf` (which can be less than `buf.len()`, e.g. near
    // end-of-data).
    impl std::io::Read for CountingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            // Let the wrapped `Cursor` do the real copying; it returns how
            // many bytes it actually wrote into `buf`.
            let n = self.inner.read(buf)?;
            // That count is ground truth for "bytes read just now". A
            // caller like `read_exact` can call `read` several times to
            // fill one buffer, so we must add on every call rather than
            // assume one call fills the whole request.
            self.bytes_read += n;
            Ok(n)
        }
    }

    // `Seek` (jumping to a byte offset, used for the tail probe) is
    // separate from `Read` in std: a type can support one without the
    // other. We forward it to the inner `Cursor` untouched. Seeking does
    // not read any bytes, so it must NOT add to `bytes_read`; only actual
    // data transfer through `read` counts.
    impl std::io::Seek for CountingReader {
        fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn transcript_span_reader_stays_within_the_probe_budget() {
        // WHY: `transcript_span_does_not_read_the_whole_file` above proves
        // the RESULT is correct on a large file, but a whole-file
        // implementation would pass that test too, since it would compute
        // the same span. This test is the one that actually watches bytes
        // read through the reader, so it is the one that would catch a
        // regression back to "just read the whole file".
        let filler = format!("{}\n", r#"{"type":"system","pad":"xxxxxxxxxxxxxxxx"}"#);
        let mut body = String::new();
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z"}"#);
        body.push('\n');
        while body.len() < 4 * 1024 * 1024 {
            body.push_str(&filler);
        }
        body.push_str(r#"{"type":"user","timestamp":"2026-01-01T09:00:00Z"}"#);
        body.push('\n');
        let len = body.len() as u64;

        let mut reader = CountingReader::new(body.into_bytes());
        let span = transcript_span_from_reader(&mut reader, len).expect("a span");

        // Correctness: still the right span, same as the whole-file test.
        assert_eq!(span.first, "2026-01-01T00:00:01Z");
        assert_eq!(span.last, "2026-01-01T09:00:00Z");

        // Boundedness: this is the assertion the old test was missing. At
        // most one head probe and one tail probe should ever be read,
        // however many megabytes the fake file holds. If someone changes
        // the function to read the whole file (or to loop past the
        // probes), this fails; today it does not.
        let budget = 2 * SPAN_PROBE_BYTES as usize;
        assert!(
            reader.bytes_read <= budget,
            "expected at most {budget} bytes read (one head + one tail probe), \
             but {} bytes were actually read out of a {len}-byte file",
            reader.bytes_read
        );
    }
}
