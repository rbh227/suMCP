//! Memoized transcript loading (ADR A3) with resource caps (ADR A9(3)).
//!
//! A long-lived MCP server gets called many times while the transcript keeps
//! growing on disk. Re-parsing 6 MB of JSONL on *every* call would be wasteful;
//! holding a parsed model forever would go stale. The middle path: stat the
//! file on each call and re-parse only when `(mtime, size)` changed. Stat is
//! microseconds; parse is ~tens of ms — so calls are always fresh and almost
//! always cheap.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
// `Arc` = atomically reference-counted pointer. Handing out `Arc<Session>`
// lets every caller share one parsed model without cloning the whole thing;
// the model is dropped when the last holder lets go.
use std::sync::Arc;
use std::time::SystemTime;
use sumcp_core::ingest::ingest_str;
use sumcp_core::model::{Lane, Session};

/// Hard ceiling on transcript bytes we will read (ADR A9(3)). Real sessions
/// top out in the tens of MB; anything past this is either corruption or an
/// attack (e.g. a device-file symlink), and refusing beats dying of OOM.
const MAX_TRANSCRIPT_BYTES: u64 = 256 * 1024 * 1024;

/// What we remember about one transcript between calls.
struct CacheEntry {
    /// File modification time at parse.
    mtime: SystemTime,
    /// File size in bytes at parse.
    size: u64,
    /// The parsed model, shared out via `Arc::clone` (cheap: bumps a counter).
    session: Arc<Session>,
}

/// Cache of parsed sessions keyed by transcript path.
pub struct SessionStore {
    /// `Mutex` because rmcp may serve calls concurrently. The lock is held
    /// across the parse — simpler, and a duplicate parse of the same file
    /// would be wasted work, not a bug.
    cache: Mutex<HashMap<PathBuf, CacheEntry>>,
}

impl SessionStore {
    /// An empty store.
    pub fn new() -> Self {
        SessionStore {
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Load `path`, re-parsing only if `(mtime, size)` changed since last time.
    pub fn load(&self, path: &Path) -> std::io::Result<Arc<Session>> {
        self.load_bounded(path, MAX_TRANSCRIPT_BYTES)
    }

    /// `load` with an injectable ceiling (tests use a tiny one; production
    /// always goes through [`SessionStore::load`]).
    fn load_bounded(&self, path: &Path, max_bytes: u64) -> std::io::Result<Arc<Session>> {
        // Stat first: this is the cheap freshness probe (ADR A3) and the
        // first A9(3) gate.
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            // Devices, FIFOs, directories: a `stat` size of 0 can hide an
            // infinite read (`/dev/zero`), so only regular files qualify.
            return Err(std::io::Error::other("transcript is not a regular file"));
        }
        if meta.len() > max_bytes {
            return Err(std::io::Error::other("transcript exceeds size ceiling"));
        }
        let (mtime, size) = (meta.modified()?, meta.len());

        // `.unwrap()` on a Mutex only fails if another thread panicked while
        // holding the lock ("poisoning") — at that point crashing is honest.
        let mut cache = self.cache.lock().unwrap();

        if let Some(entry) = cache.get(path)
            && entry.mtime == mtime
            && entry.size == size
        {
            // Fresh enough: same mtime and size as when we parsed.
            return Ok(Arc::clone(&entry.session));
        }

        // Miss or stale: read (bounded — the stat size above can lie for
        // files that grow between stat and read), parse, remember.
        let mut raw = String::new();
        std::fs::File::open(path)?
            .take(max_bytes + 1)
            .read_to_string(&mut raw)?;
        if raw.len() as u64 > max_bytes {
            return Err(std::io::Error::other("transcript exceeds size ceiling"));
        }
        let session = Arc::new(ingest_str(&raw, Lane::Main));
        cache.insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                session: Arc::clone(&session),
            },
        );
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// One minimal-but-valid transcript line (an Edit tool_use).
    fn line(id: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        )
    }

    #[test]
    fn second_load_of_unchanged_file_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, line("t1")).unwrap();

        let store = SessionStore::new();
        let a = store.load(&path).unwrap();
        let b = store.load(&path).unwrap();

        // Same allocation, not just equal content — proof no re-parse ran.
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn grown_file_is_reparsed_and_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, line("t1")).unwrap();

        let store = SessionStore::new();
        let before = store.load(&path).unwrap();
        assert_eq!(before.actions.len(), 1);

        // Append a second line — size changes even if mtime granularity is
        // too coarse to notice the difference.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f).unwrap();
        f.write_all(line("t2").as_bytes()).unwrap();
        drop(f);

        let after = store.load(&path).unwrap();
        // A fresh allocation proves the re-parse; the new count proves it
        // read the new content.
        assert!(!Arc::ptr_eq(&before, &after), "grown file must re-parse");
        assert_eq!(after.actions.len(), 2, "new content must be visible");
    }

    #[test]
    fn missing_file_is_an_io_error_not_a_panic() {
        let store = SessionStore::new();
        assert!(store.load(Path::new("/nonexistent/nope.jsonl")).is_err());
    }

    #[test]
    fn oversized_transcript_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, line("t1")).unwrap(); // ~180 bytes

        let store = SessionStore::new();
        let err = store.load_bounded(&path, 10).unwrap_err();
        assert!(err.to_string().contains("size ceiling"), "{err}");
    }

    #[test]
    fn non_regular_file_is_refused() {
        // /dev/null stats as a char device, not a regular file — exactly the
        // class of target a symlink attack points at (/dev/zero would hang).
        let store = SessionStore::new();
        let err = store.load(Path::new("/dev/null")).unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }
}
