//! Memoized work-unit loading (ADR A3) with resource caps (ADR A9(3)).
//!
//! A long-lived MCP server gets called many times while the transcripts it
//! is reading keep growing on disk, and the "unit" (the several transcripts
//! making up one continuous stretch of work, see `sumcp_core::work_unit`)
//! can also gain a brand-new sibling transcript at any moment. Re-parsing
//! several MB of JSONL on *every* call would be wasteful; holding a parsed
//! unit forever would go stale. The middle path: cheaply re-check whether the
//! unit still looks the same on every call, and only redo the expensive
//! parse-and-merge work when something has actually changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
// `Arc` = atomically reference-counted pointer. Handing out `Arc<LoadedUnit>`
// lets every caller share one parsed unit without cloning the whole thing;
// the unit is dropped when the last holder lets go.
use std::sync::Arc;
use sumcp_core::assemble::{MAX_TRANSCRIPT_BYTES as CORE_MAX_BYTES, load_work_unit};
use sumcp_core::model::Session;
use sumcp_core::payloads::{UnitMeta, unit_meta_from};
use sumcp_core::work_unit::discover_work_unit;

/// Cap on cached parsed units (T4.2). A long-lived server that outlives many
/// sessions would otherwise hold every unit it ever parsed; parsed units run
/// to tens of MB each. Four covers the realistic concurrent case (a couple of
/// open sessions plus one or two recently closed).
const MAX_CACHE_ENTRIES: usize = 4;

/// What the store caches: a fully assembled work unit plus the grouping
/// description its payloads need.
#[derive(Debug)]
pub struct LoadedUnit {
    /// The merged session covering the whole unit.
    pub session: Session,
    /// The grouping, or `None` when the unit turned out to be one transcript.
    pub meta_unit: Option<UnitMeta>,
}

/// Stat every path into a `(path, mtime-in-seconds, size)` fingerprint,
/// dropping any that no longer exist, then sort so the result does not depend
/// on what order `paths` was given in.
///
/// WHY NOT just the requested transcript's own `(mtime, size)`, the way this
/// store used to key freshness: a work unit is several transcripts, and two
/// different things can make an OLD unit stale. The newest member can simply
/// grow while a session is live, but an OLDER member can also *appear* out
/// of nowhere, when a second, concurrent Claude Code instance happens to
/// write into the same stretch of time (this project's own corpus has a real
/// 8-transcript unit where every join is exactly this: an overlap, not a
/// continuation). Keying on the single requested file would miss that second
/// case completely and keep serving a stale, too-small unit forever, which
/// is exactly the case that motivated grouping transcripts into "lanes" in
/// the first place. So every member's path, plus every subagent transcript
/// merged into any member, all go into one key, and ANY of them changing (or
/// a path from the list vanishing, which shortens the returned vector so the
/// `==` check below fails) forces a re-parse.
///
/// A vanished path is silently dropped rather than treated as an error:
/// dropping it is what makes the length mismatch, and hence the freshness
/// check, work; a stat that merely fails is not this function's problem to
/// diagnose.
fn unit_key(paths: &[PathBuf]) -> Vec<(PathBuf, u64, u64)> {
    let mut key: Vec<(PathBuf, u64, u64)> = paths
        .iter()
        .filter_map(|p| {
            let m = std::fs::metadata(p).ok()?;
            let mtime = m
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some((p.clone(), mtime, m.len()))
        })
        .collect();
    key.sort();
    key
}

/// Fingerprint a directory's own LISTING, with no file ever opened: every
/// entry's path, mtime-in-seconds and size, via the cheap `stat`-style
/// `DirEntry::metadata` (not a read of the file's bytes), sorted so the
/// result does not depend on what order the OS handed entries back in.
///
/// PLAIN-LANGUAGE VERSION OF WHY THIS IS ENOUGH: the expensive thing this
/// lets most calls skip, `discover_work_unit`, decides which transcripts
/// belong to a work unit by looking at exactly two things about this
/// directory: which `.jsonl` files are sitting in it, and what each one's
/// content says (its first and last timestamp, read from the file itself).
/// A directory listing already tells us the first half directly (a file
/// appearing or disappearing changes the list of entries here). For the
/// second half, it does not need to read the content itself, because a
/// file's `mtime` and `size` only change when its content changes. So if
/// every entry's path, `mtime` and `size` are BYTE-FOR-BYTE the same as the
/// last time we looked, nothing that `discover_work_unit` could possibly
/// see has changed either, and re-running it would necessarily produce the
/// exact same members. That is what lets a listing (cheap: one `stat` per
/// entry) stand in for actually opening and scanning every transcript
/// (expensive: a 256 KiB head read plus a 256 KiB tail read out of each).
///
/// This is deliberately coarser than `discover_work_unit` in one direction:
/// it fingerprints EVERY entry in the directory, including ones
/// `discover_work_unit` would itself ignore (non-`.jsonl` files, legacy
/// `agent-*.jsonl` subagent siblings). An unrelated file appearing changes
/// the fingerprint and forces a fresh discovery pass even though the work
/// unit's membership could not have changed. That costs an occasional
/// unnecessary discovery pass, never a missed change, so it costs no
/// correctness: this fingerprint is strictly MORE sensitive than the
/// member-list check it gates, not less.
fn dir_fingerprint(dir: &Path) -> Vec<(PathBuf, u64, u64)> {
    let mut fp: Vec<(PathBuf, u64, u64)> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let meta = entry.metadata().ok()?;
                    let mtime = meta
                        .modified()
                        .ok()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_secs();
                    Some((entry.path(), mtime, meta.len()))
                })
                .collect()
        })
        .unwrap_or_default();
    fp.sort();
    fp
}

/// What one cache slot remembers between calls.
struct CacheEntry {
    /// The member paths `discover_work_unit` returned at the last parse,
    /// oldest first, exactly as it returned them (readable or not: see
    /// `load_bounded`'s doc on why this must include unreadable members
    /// too). Freshness needs this kept separately from `key` (below)
    /// because it is the ONLY thing that can notice a brand-new sibling
    /// transcript joining the unit: a new member's path was never stated
    /// before, so it cannot show up as "changed" in a re-stat of paths we
    /// already knew about. Comparing this list against a fresh discovery is
    /// what catches it; `dir_fp` (below) is what lets most calls skip
    /// paying for that fresh discovery at all.
    member_paths: Vec<PathBuf>,
    /// Freshness key (see `unit_key`) over every member path plus every
    /// subagent transcript merged into any of them, at the last parse.
    /// Re-stating the same paths and comparing catches any of them growing,
    /// shrinking, or disappearing.
    key: Vec<(PathBuf, u64, u64)>,
    /// Fingerprint (see `dir_fingerprint`) of the requested path's own
    /// directory, taken at the last parse. Comparing this to a fresh
    /// fingerprint is a cheap stand-in for re-running `discover_work_unit`:
    /// if the directory's listing has not changed at all, discovery could
    /// not possibly return anything different, so there is no need to pay
    /// for the head/tail read of every candidate transcript in it just to
    /// confirm that.
    dir_fp: Vec<(PathBuf, u64, u64)>,
    /// The parsed unit, shared out via `Arc::clone` (cheap: bumps a counter).
    unit: Arc<LoadedUnit>,
    /// Logical clock value of the last hit: the LRU eviction key. A counter,
    /// not wall time: it can't go backwards and never collides under the lock.
    last_used: u64,
}

/// The state behind the lock: the map plus the logical clock that orders
/// entries by recency. Bundled in one struct so a single `Mutex` guards both
/// (two locks would allow the clock and map to drift apart).
struct Inner {
    /// Monotonic tick, bumped on every `load`.
    tick: u64,
    /// Cache keyed by the transcript path the caller actually asked for.
    /// Two different requested paths that happen to resolve to the same
    /// underlying work unit get two entries here, each holding an `Arc` to
    /// logically-equivalent content (a little redundant, but simple), and the
    /// LRU cap still bounds total memory either way.
    map: HashMap<PathBuf, CacheEntry>,
}

/// Cache of parsed work units keyed by the requested transcript path,
/// LRU-capped at [`MAX_CACHE_ENTRIES`].
pub struct SessionStore {
    /// `Mutex` because rmcp may serve calls concurrently. The lock is held
    /// across the parse (simpler), and a duplicate parse of the same unit
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

    /// Load the work unit `path` belongs to, re-parsing only if something
    /// about it actually changed since last time.
    pub fn load(&self, path: &Path) -> std::io::Result<Arc<LoadedUnit>> {
        self.load_bounded(path, CORE_MAX_BYTES)
    }

    /// `load` with an injectable ceiling (tests use a tiny one; production
    /// always goes through [`SessionStore::load`]).
    fn load_bounded(&self, path: &Path, max_bytes: u64) -> std::io::Result<Arc<LoadedUnit>> {
        // Stat first: this is the cheap freshness probe (ADR A3) and the
        // first A9(3) gate, unchanged from before this store cared about
        // whole units: it still only ever concerns the exact path we were
        // asked for.
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            // Devices, FIFOs, directories: a `stat` size of 0 can hide an
            // infinite read (`/dev/zero`), so only regular files qualify.
            return Err(std::io::Error::other("transcript is not a regular file"));
        }
        if meta.len() > max_bytes {
            return Err(std::io::Error::other("transcript exceeds size ceiling"));
        }

        // Second freshness probe, new in this version: find out which
        // transcripts belong to this unit RIGHT NOW, cheaply. The thing
        // that answers this authoritatively, `discover_work_unit`, reads a
        // 256 KiB head AND a 256 KiB tail out of EVERY candidate transcript
        // in the directory to learn its time span; on this project's own
        // 82-transcript corpus that is on the order of 40 MB read and
        // scanned, on every single call, which is not "small" at all. A
        // directory-listing fingerprint (`dir_fingerprint`, see its doc for
        // why a listing is enough) stands in for it: no file is ever
        // opened, just one cheap `stat`-style call per directory entry. Only
        // when that fingerprint has actually changed since the last parse
        // do we pay for the real thing.
        let dir_fp = path.parent().map(dir_fingerprint).unwrap_or_default();

        // `.unwrap()` on a Mutex only fails if another thread panicked while
        // holding the lock ("poisoning"); at that point crashing is honest.
        let mut cache = self.cache.lock().unwrap();
        cache.tick += 1;
        let now = cache.tick;

        // If the directory looks exactly like it did at the last parse, a
        // fresh `discover_work_unit` call could not possibly return
        // anything different, so reuse the member list we already have
        // instead of re-deriving it. Otherwise (first call for this path,
        // or something in the directory actually changed) pay for the real
        // discovery pass.
        let current_members: Vec<PathBuf> = match cache.map.get(path) {
            Some(entry) if entry.dir_fp == dir_fp => entry.member_paths.clone(),
            _ => discover_work_unit(path)
                .members
                .iter()
                .map(|m| m.path.clone())
                .collect(),
        };

        if let Some(entry) = cache.map.get_mut(path)
            && entry.member_paths == current_members
        {
            // The unit still has exactly the members it had last time.
            // Re-stat every path we merged last time (members AND
            // subagents); if all of them still match, nothing about the
            // unit's content could have changed either.
            let tracked: Vec<PathBuf> = entry.key.iter().map(|(p, _, _)| p.clone()).collect();
            if unit_key(&tracked) == entry.key {
                // Fresh enough. Touch the recency clock so a hot entry never
                // looks evictable, and refresh the directory fingerprint (it
                // may have changed even when nothing about the unit's own
                // content did, e.g. an unrelated file appearing).
                entry.last_used = now;
                entry.dir_fp = dir_fp;
                return Ok(Arc::clone(&entry.unit));
            }
        }

        // Miss or stale: assemble the whole unit. `load_work_unit` runs (or,
        // when the fast path above already ran `discover_work_unit`,
        // re-runs) that same discovery internally, and then does the
        // actually expensive part (reading and merging every member with
        // its own subagents), so the small possible redundancy in
        // discovery costs nothing worth engineering away.
        let assembled = load_work_unit(path, max_bytes)
            .map_err(|e| std::io::Error::other(format!("assemble failed: {e}")))?;

        // Cache the FULL discovered member list, readable or not, not just
        // the members that actually loaded. `current_members` (computed
        // above) is always equivalent to `discover_work_unit(...).members`,
        // which lists every discovered member regardless of readability; an
        // unreadable member (over the size ceiling, a permissions error, or
        // deleted between discovery and read -- all disclosed, expected
        // conditions, never fatal to the unit) is skipped by
        // `load_work_unit`, so `assembled.member_paths` only contains the
        // members that loaded successfully. Caching THAT list here would
        // make it permanently shorter than `current_members` whenever any
        // member is unreadable, so the equality check above could never
        // match again and every call would re-discover, re-parse and
        // re-merge the whole unit. `assembled.unit.members` is the same
        // source `current_members` is built from, so the two stay
        // comparable.
        let member_paths: Vec<PathBuf> = assembled
            .unit
            .members
            .iter()
            .map(|m| m.path.clone())
            .collect();
        let all_paths: Vec<PathBuf> = assembled
            .member_paths
            .iter()
            .chain(assembled.subagent_paths.iter())
            .cloned()
            .collect();
        let key = unit_key(&all_paths);

        // A unit of one (no sibling transcript ever joined, nothing
        // excluded) is reported exactly the way a single transcript always
        // was: no grouping to disclose. `unit_meta_from` itself makes that
        // call and returns `None`, so this store and the CLI cannot drift
        // apart on when the block appears (see `LoadedUnit`'s doc and
        // `finding_session` in `payloads`, which requires this to stay in
        // lockstep with whether a `session` key may appear on a finding).
        let meta_unit = unit_meta_from(&assembled);
        let unit = Arc::new(LoadedUnit {
            session: assembled.session,
            meta_unit,
        });

        cache.map.insert(
            path.to_path_buf(),
            CacheEntry {
                member_paths,
                key,
                dir_fp,
                unit: Arc::clone(&unit),
                last_used: now,
            },
        );
        // LRU eviction (T4.2): insert first, then trim. A stale re-parse of a
        // cached path replaces in place (no growth, no eviction), and the
        // fresh entry can never be the victim: its `last_used` is the
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
        Ok(unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// One minimal-but-valid transcript line (an Edit tool_use) at a given
    /// timestamp.
    fn line_at(id: &str, ts: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
        )
    }

    /// Convenience for tests where the transcript is the only file in its
    /// directory, so the exact timestamp cannot matter for grouping.
    fn line(id: &str) -> String {
        line_at(id, "2026-01-01T00:00:00Z")
    }

    /// A timestamp comfortably more than `WORK_UNIT_IDLE_GAP_SECS` (30 min)
    /// away from every other timestamp this helper produces. Tests that put
    /// several transcripts in the SAME directory use distinct `slot` values
    /// so `discover_work_unit` never folds them into one work unit purely
    /// because they happen to share a directory. These tests are about
    /// cache-slot mechanics per requested path, not work-unit grouping, and
    /// an accidental merge would silently change what they are testing.
    fn spaced_ts(slot: u32) -> String {
        format!("2026-01-01T{slot:02}:00:00Z")
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
        let sub_actions = a
            .session
            .actions
            .iter()
            .filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_)))
            .count();
        assert_eq!(sub_actions, 1, "subagent edit merged via the store");

        // Grow the subagent file; the merged session must re-parse.
        let mut f = std::fs::OpenOptions::new().append(true).open(&sib).unwrap();
        writeln!(f).unwrap();
        f.write_all(
            br#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/s2.rs","new_string":"y"}}]}}"#,
        )
        .unwrap();
        drop(f);

        let b = store.load(&main).unwrap();
        let sub_actions_b = b
            .session
            .actions
            .iter()
            .filter(|x| matches!(x.lane, sumcp_core::model::Lane::Sub(_)))
            .count();
        assert_eq!(
            sub_actions_b, 2,
            "appended subagent action picked up (freshness over sub files)"
        );
    }

    #[test]
    fn cache_hits_when_a_work_unit_member_is_unreadable() {
        // Reproduces the bug: a work unit with one readable member and one
        // member that fails to load (here, over the injected size ceiling,
        // the same disclosed condition `load_work_unit_survives_an_unreadable_member`
        // in assemble.rs covers) must still cache-hit on an unchanged
        // second `load`. Before the fix, `CacheEntry::member_paths` was
        // built from `assembled.member_paths` (only the members that loaded
        // successfully), while the comparison list on the next call is
        // built from `discover_work_unit(...).members` (every discovered
        // member, readable or not). Those two lists could never be equal
        // whenever any member failed to load, so the cache-hit check never
        // matched and every call re-discovered, re-parsed and re-merged the
        // whole unit.
        let dir = tempfile::tempdir().unwrap();
        let readable = dir
            .path()
            .join("aaaaaaaa-1111-2222-3333-444455556666.jsonl");
        std::fs::write(&readable, line_at("t0", "2026-01-01T00:00:00Z")).unwrap();

        // Same shape, plus padding, so this file is bigger than `readable`.
        // The padding sits after the timestamped line so `transcript_span`'s
        // head/tail scan still finds a timestamp and this file still counts
        // as a discovered member; it is `load_session`'s OWN size ceiling
        // (not `transcript_span`, which has none) that will refuse to read it.
        let unreadable = dir
            .path()
            .join("bbbbbbbb-1111-2222-3333-444455556666.jsonl");
        std::fs::write(
            &unreadable,
            format!(
                "{}\n{}",
                line_at("t1", "2026-01-01T00:05:00Z"),
                "x".repeat(4096)
            ),
        )
        .unwrap();

        // A ceiling that admits `readable` but not `unreadable`.
        let ceiling = std::fs::metadata(&readable).unwrap().len() + 100;
        assert!(std::fs::metadata(&unreadable).unwrap().len() > ceiling);

        let store = SessionStore::new();
        let a = store.load_bounded(&readable, ceiling).unwrap();
        let b = store.load_bounded(&readable, ceiling).unwrap();

        assert!(
            Arc::ptr_eq(&a, &b),
            "a work unit with an unreadable member must still cache-hit on an unchanged reload"
        );
    }

    #[test]
    fn second_load_of_unchanged_file_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, line("t1")).unwrap();

        let store = SessionStore::new();
        let a = store.load(&path).unwrap();
        let b = store.load(&path).unwrap();

        // Same allocation, not just equal content: proof no re-parse ran.
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn grown_file_is_reparsed_and_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, line("t1")).unwrap();

        let store = SessionStore::new();
        let before = store.load(&path).unwrap();
        assert_eq!(before.session.actions.len(), 1);

        // Append a second line: size changes even if mtime granularity is
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
        assert_eq!(
            after.session.actions.len(),
            2,
            "new content must be visible"
        );
    }

    #[test]
    fn cache_evicts_least_recently_used_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::new();

        // Fill the cache to the cap, oldest first. Each file gets its own
        // hour slot (see `spaced_ts`) so none of them join into a shared
        // work unit just because they sit in the same directory.
        let paths: Vec<PathBuf> = (0..MAX_CACHE_ENTRIES)
            .map(|i| {
                let p = dir.path().join(format!("s{i}.jsonl"));
                std::fs::write(&p, line_at(&format!("t{i}"), &spaced_ts(i as u32))).unwrap();
                store.load(&p).unwrap();
                p
            })
            .collect();

        // Touch the oldest so it becomes the most recent: the eviction
        // victim must now be paths[1], not paths[0].
        let kept = store.load(&paths[0]).unwrap();

        // One past the cap forces an eviction.
        let extra = dir.path().join("extra.jsonl");
        std::fs::write(&extra, line_at("tx", &spaced_ts(10))).unwrap();
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
                std::fs::write(&p, line_at(&format!("t{i}"), &spaced_ts(i as u32))).unwrap();
                store.load(&p).unwrap();
                p
            })
            .collect();

        // Grow an already-cached file. This is a stale re-parse, not a new
        // path, so nothing may be evicted for it.
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

    // Unix only because it needs a real non-regular file to point at, and
    // /dev/null does not exist on Windows. The production guard is
    // platform-independent (it asks whether the metadata says regular file), so
    // Windows is still protected; only this test's fixture is Unix specific.
    #[cfg(unix)]
    #[test]
    fn non_regular_file_is_refused() {
        // /dev/null stats as a char device, not a regular file, which is exactly
        // the class of target a symlink attack points at (/dev/zero would hang).
        let store = SessionStore::new();
        let err = store.load(Path::new("/dev/null")).unwrap_err();
        assert!(err.to_string().contains("not a regular file"), "{err}");
    }
}
