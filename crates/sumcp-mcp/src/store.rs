//! Memoized transcript loading (ADR A3) with resource caps (ADR A9(3)).
//!
//! A long-lived MCP server gets called many times while the transcript keeps
//! growing on disk. Re-parsing 6 MB of JSONL on *every* call would be wasteful;
//! holding a parsed model forever would go stale. The middle path: stat the
//! file on each call and re-parse only when `(mtime, size)` changed. Stat is
//! microseconds; parse is ~tens of ms — so calls are always fresh and almost
//! always cheap.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
// `Arc` = atomically reference-counted pointer. Handing out `Arc<Session>`
// lets every caller share one parsed model without cloning the whole thing;
// the model is dropped when the last holder lets go.
use std::sync::Arc;
use std::time::SystemTime;
use sumcp_core::assemble::{load_session, MAX_TRANSCRIPT_BYTES as CORE_MAX_BYTES};
use sumcp_core::model::Session;

/// Cap on cached parsed sessions (T4.2). A long-lived server that outlives
/// many sessions would otherwise hold every model it ever parsed; parsed
/// models run to tens of MB each. Four covers the realistic concurrent case
/// (a couple of open sessions plus one or two recently closed).
const MAX_CACHE_ENTRIES: usize = 4;

/// Stat a list of subagent paths into (path, mtime, size) fingerprints,
/// skipping any that vanished (a vanished sub file forces a reload).
///
/// Plain-language: for each sub-transcript path we merged, ask the filesystem
/// "when was this last changed and how big is it now?". We keep only the ones
/// that still exist. Comparing this list to the one we stored at parse time is
/// how we decide whether a sub file changed under us: if a path disappeared or
/// its (mtime, size) differs, the lists won't be equal and we re-parse.
fn fingerprint_subs(paths: &[PathBuf]) -> Vec<(PathBuf, SystemTime, u64)> {
    paths
        .iter()
        .filter_map(|p| {
            let m = std::fs::metadata(p).ok()?;
            Some((p.clone(), m.modified().ok()?, m.len()))
        })
        .collect()
}

/// What we remember about one transcript between calls.
struct CacheEntry {
    /// File modification time at parse.
    mtime: SystemTime,
    /// File size in bytes at parse.
    size: u64,
    /// (path, mtime, size) for every subagent file merged into `session`.
    /// Freshness requires ALL of these to be unchanged too, so an appended or
    /// added subagent transcript re-parses.
    subs: Vec<(PathBuf, SystemTime, u64)>,
    /// The parsed model, shared out via `Arc::clone` (cheap: bumps a counter).
    session: Arc<Session>,
    /// Logical clock value of the last hit — the LRU eviction key. A counter,
    /// not wall time: it can't go backwards and never collides under the lock.
    last_used: u64,
}

/// The state behind the lock: the map plus the logical clock that orders
/// entries by recency. Bundled in one struct so a single `Mutex` guards both
/// (two locks would allow the clock and map to drift apart).
struct Inner {
    /// Monotonic tick, bumped on every `load`.
    tick: u64,
    /// Cache keyed by transcript path.
    map: HashMap<PathBuf, CacheEntry>,
}

/// Cache of parsed sessions keyed by transcript path, LRU-capped at
/// [`MAX_CACHE_ENTRIES`].
pub struct SessionStore {
    /// `Mutex` because rmcp may serve calls concurrently. The lock is held
    /// across the parse — simpler, and a duplicate parse of the same file
    /// would be wasted work, not a bug.
    cache: Mutex<Inner>,
}

impl SessionStore {
    /// An empty store.
    pub fn new() -> Self {
        SessionStore {
            cache: Mutex::new(Inner {
                tick: 0,
                map: HashMap::new(),
            }),
        }
    }

    /// Load `path`, re-parsing only if `(mtime, size)` changed since last time.
    pub fn load(&self, path: &Path) -> std::io::Result<Arc<Session>> {
        self.load_bounded(path, CORE_MAX_BYTES)
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
        cache.tick += 1;
        let now = cache.tick;

        if let Some(entry) = cache.map.get_mut(path)
            && entry.mtime == mtime
            && entry.size == size
        {
            // Main file is unchanged. But a merged session also depends on its
            // subagent transcripts, so re-stat every sub file we merged last
            // time and compare fingerprints. `fingerprint_subs` drops any that
            // vanished, so a deleted sub file shortens the fresh vector and the
            // `==` below fails — forcing a reload. (A brand-new spawn appends a
            // `tool_result` line to the main file, so main's mtime/size already
            // catches it; this `subs` check is specifically for an *existing*
            // sub file that grew.)
            let cached_paths: Vec<PathBuf> =
                entry.subs.iter().map(|(p, _, _)| p.clone()).collect();
            let subs_fresh = entry.subs == fingerprint_subs(&cached_paths);
            if subs_fresh {
                // Fresh enough: main and every merged sub unchanged. Touch the
                // recency clock so a hot entry never looks evictable.
                entry.last_used = now;
                return Ok(Arc::clone(&entry.session));
            }
        }

        // Miss or stale: assemble the main transcript with its subagents
        // merged (`load_session` repeats the bounded read of the main file
        // internally and reads/merges any discovered sub files), then remember
        // both the parsed model and a fingerprint of every sub file we merged.
        let assembled = load_session(path, max_bytes)
            .map_err(|e| std::io::Error::other(format!("assemble failed: {e}")))?;
        let subs = fingerprint_subs(&assembled.subagent_paths);
        let session = Arc::new(assembled.session);

        cache.map.insert(
            path.to_path_buf(),
            CacheEntry {
                mtime,
                size,
                subs,
                session: Arc::clone(&session),
                last_used: now,
            },
        );
        // LRU eviction (T4.2): insert first, then trim. A stale re-parse of a
        // cached path replaces in place (no growth, no eviction), and the
        // fresh entry can never be the victim — its `last_used` is the
        // current tick, strictly newest. With a cap of 4, a linear min-scan
        // beats carrying a linked-list LRU crate.
        if cache.map.len() > MAX_CACHE_ENTRIES
            && let Some(coldest) = cache
                .map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(p, _)| p.clone())
        {
            cache.map.remove(&coldest);
        }
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
    fn store_merges_subagents_and_reparses_on_sub_change() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "5717aaaa-1111-2222-3333-444455556666";
        let main = dir.path().join(format!("{uuid}.jsonl"));
        // main spawns one Agent with a legacy sibling.
        std::fs::write(
            &main,
            format!(
                "{}\n{}",
                r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"a1","name":"Agent","input":{"subagent_type":"x"}}]}}"#,
                r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"a1","is_error":false}]},"toolUseResult":{"agentId":"present"}}"#,
            ),
        )
        .unwrap();
        let sib = dir.path().join("agent-present.jsonl");
        std::fs::write(
            &sib,
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/s.rs","new_string":"x"}}]}}"#,
        )
        .unwrap();

        let store = SessionStore::new();
        let a = store.load(&main).unwrap();
        let sub_actions =
            a.actions.iter().filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_))).count();
        assert_eq!(sub_actions, 1, "subagent edit merged via the store");

        // Grow the subagent file; the merged session must re-parse.
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new().append(true).open(&sib).unwrap();
        writeln!(f).unwrap();
        f.write_all(
            br#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/s2.rs","new_string":"y"}}]}}"#,
        )
        .unwrap();
        drop(f);

        let b = store.load(&main).unwrap();
        let sub_actions_b =
            b.actions.iter().filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_))).count();
        assert_eq!(sub_actions_b, 2, "appended subagent action picked up (freshness over sub files)");
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
    fn cache_evicts_least_recently_used_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();

        // Fill the cache to the cap, oldest first.
        let paths: Vec<PathBuf> = (0..MAX_CACHE_ENTRIES)
            .map(|i| {
                let p = dir.path().join(format!("s{i}.jsonl"));
                std::fs::write(&p, line(&format!("t{i}"))).unwrap();
                store.load(&p).unwrap();
                p
            })
            .collect();

        // Touch the oldest so it becomes the most recent — the eviction
        // victim must now be paths[1], not paths[0].
        let kept = store.load(&paths[0]).unwrap();

        // One past the cap forces an eviction.
        let extra = dir.path().join("extra.jsonl");
        std::fs::write(&extra, line("tx")).unwrap();
        store.load(&extra).unwrap();

        // The touched entry survived: same allocation proves a cache hit.
        let again = store.load(&paths[0]).unwrap();
        assert!(Arc::ptr_eq(&kept, &again), "recently-used entry evicted");

        // Direct proof the map obeys the cap (not just indirect Arc checks).
        assert!(store.cache.lock().unwrap().map.len() <= MAX_CACHE_ENTRIES);
        assert!(
            !store.cache.lock().unwrap().map.contains_key(&paths[1]),
            "the least-recently-used entry must be the one evicted"
        );
    }

    #[test]
    fn stale_reparse_replaces_in_place_without_eviction() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();
        let paths: Vec<PathBuf> = (0..MAX_CACHE_ENTRIES)
            .map(|i| {
                let p = dir.path().join(format!("s{i}.jsonl"));
                std::fs::write(&p, line(&format!("t{i}"))).unwrap();
                store.load(&p).unwrap();
                p
            })
            .collect();

        // Grow an already-cached file: a stale re-parse, not a new path —
        // nothing may be evicted for it.
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&paths[0])
            .unwrap();
        writeln!(f).unwrap();
        f.write_all(line("t0b").as_bytes()).unwrap();
        drop(f);
        store.load(&paths[0]).unwrap();

        let inner = store.cache.lock().unwrap();
        assert_eq!(inner.map.len(), MAX_CACHE_ENTRIES);
        for p in &paths {
            assert!(inner.map.contains_key(p), "no entry may be evicted");
        }
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
