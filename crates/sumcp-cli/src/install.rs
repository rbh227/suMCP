//! The install / uninstall write path — the ONLY place suMCP writes anything
//! outside its own read-only analysis. Everything here honors ADR A8:
//!
//! - **Dry-run by default.** `install` and `uninstall` just *print* what they
//!   would do; nothing touches disk unless you pass `--apply`.
//! - **Atomic writes.** Every file is written to a temp file in the same
//!   directory and then `rename`d over the target, so a crash mid-write can
//!   never leave a half-written config.
//! - **Timestamped backups.** Any pre-existing file we replace wholesale is
//!   copied to `<file>.sumcp-bak.<unix-seconds>` first.
//! - **Rollback.** If `--apply` fails partway, every step already done is
//!   undone (best-effort) before we return the error. On a failed reinstall
//!   the previous install is left intact and usable, with its manifest.
//! - **Manifest-tracked uninstall.** We record every action in
//!   `~/.claude/sumcp/manifest.json`; `uninstall` reads it and removes only
//!   what we created (and restores backups of files we replaced). Directories
//!   we created are removed only if empty by then, so anything the user later
//!   placed inside them (say, another skill) survives. Files carry a content
//!   hash, so a reinstall can tell "still ours" from "the user changed this"
//!   and never throws away the only copy of a user's version.
//! - **Retryable uninstall.** If an undo step fails, uninstall reports the
//!   error and rewrites the manifest with just the remaining steps, so it can
//!   simply be run again once the cause is fixed.
//! - **Safety rails.** We refuse to write through a symlink, and every target
//!   must resolve to a path under `$HOME`. Files are `0700`, dirs `0700`.
//!
//! ## What gets installed (all under `$HOME`)
//!
//! | Target                                   | What                                   |
//! |------------------------------------------|----------------------------------------|
//! | `~/.claude/sumcp/bin/sumcp{,-mcp}`       | copies of the two binaries             |
//! | `~/.claude/sumcp/hooks/stop-nudge.sh`    | the Stop-hook nudge script             |
//! | `~/.claude/sumcp/manifest.json`          | the uninstall ledger                   |
//! | `~/.claude/skills/debrief/SKILL.md`      | the debrief skill                      |
//! | `~/.claude.json` → `mcpServers.sumcp`    | registers the MCP server (user scope)  |
//! | `~/.claude/settings.json` → `hooks.Stop` | registers the Stop hook (user scope)   |

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bumped if the manifest layout ever changes so an old `uninstall` can refuse
/// a manifest it doesn't understand rather than mis-handle it.
const MANIFEST_VERSION: u32 = 1;

/// The key we register the MCP server under, in both `~/.claude.json` and the
/// backup/removal logic. One constant so the two can never drift.
const SERVER_KEY: &str = "sumcp";

/// The debrief skill body, baked into the binary at compile time. Embedding it
/// (rather than copying from the repo) means `install` works even if the clone
/// is later deleted — the installed skill is self-contained.
const SKILL_MD: &str = include_str!("../../../skills/debrief/SKILL.md");

/// The Stop-hook script, also baked in. It shells out to the *installed* sumcp
/// binary (absolute path substituted at write time) so it has no dependency on
/// the repo or on `sumcp` being on `PATH`.
const HOOK_TEMPLATE: &str = r#"#!/bin/sh
# suMCP Stop-hook nudge (installed by `sumcp install`). Read-only.
# Claude Code pipes a JSON blob on stdin including "transcript_path". We ask the
# installed sumcp binary how many edits the session had; if it's a substantial
# session we print a one-line reminder to run the debrief. Never blocks.
set -eu
SUMCP="__SUMCP_BIN__"
THRESHOLD=3
input="$(cat)"
# Pull transcript_path out of the stdin JSON without needing jq.
transcript="$(printf '%s' "$input" | sed -n 's/.*"transcript_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
[ -n "${transcript:-}" ] || exit 0
[ -x "$SUMCP" ] || exit 0
edits="$("$SUMCP" --file "$transcript" --json 2>/dev/null | sed -n 's/.*"edits"[[:space:]]*:[[:space:]]*\([0-9]*\).*/\1/p' | head -n1)"
[ -n "${edits:-}" ] || exit 0
if [ "$edits" -ge "$THRESHOLD" ]; then
  printf '{"systemMessage":"suMCP: this session had %s edits — run the debrief skill to see what actually struggled."}\n' "$edits"
fi
exit 0
"#;

// ---------------------------------------------------------------------------
// Paths: every destination derived from one home root. In tests we point this
// at a temp dir; in production it comes from `$HOME`.
// ---------------------------------------------------------------------------

/// All install destinations, derived from a single home directory.
pub struct Paths {
    home: PathBuf,
}

impl Paths {
    /// Build from `$HOME`. Errors if it's unset or empty (we never guess a home).
    pub fn from_env() -> io::Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
        Ok(Self { home })
    }

    /// Build from an explicit home (used by tests).
    #[cfg(test)]
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    fn claude(&self) -> PathBuf {
        self.home.join(".claude")
    }
    fn sumcp(&self) -> PathBuf {
        self.claude().join("sumcp")
    }
    fn bin(&self) -> PathBuf {
        self.sumcp().join("bin")
    }
    fn hooks(&self) -> PathBuf {
        self.sumcp().join("hooks")
    }
    fn manifest(&self) -> PathBuf {
        self.sumcp().join("manifest.json")
    }
    fn hook_script(&self) -> PathBuf {
        self.hooks().join("stop-nudge.sh")
    }
    fn skill_dest(&self) -> PathBuf {
        self.claude().join("skills").join("debrief")
    }
    fn installed_sumcp(&self) -> PathBuf {
        self.bin().join("sumcp")
    }
    fn installed_mcp(&self) -> PathBuf {
        self.bin().join("sumcp-mcp")
    }
    fn claude_json(&self) -> PathBuf {
        self.home.join(".claude.json")
    }
    fn settings_json(&self) -> PathBuf {
        self.claude().join("settings.json")
    }
}

// ---------------------------------------------------------------------------
// Manifest: the ledger `uninstall` replays in reverse.
// ---------------------------------------------------------------------------

/// One recorded install action. `#[serde(tag = "kind")]` makes each variant
/// serialize as a JSON object with a `"kind"` field, e.g.
/// `{"kind":"created","path":"…"}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Entry {
    /// A file or directory that did not exist before — remove it on uninstall.
    /// For files, `written_hash` records what we wrote (see [`fnv1a64`]) so a
    /// later reinstall can tell our content from a user's edit. `None` for
    /// directories and for manifests written before hashes existed.
    Created {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        written_hash: Option<u64>,
    },
    /// A file that existed and we replaced wholesale — restore `backup` on
    /// uninstall. `written_hash` as above.
    Replaced {
        path: PathBuf,
        backup: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        written_hash: Option<u64>,
    },
    /// A nested key we spliced into a shared JSON file — delete just that key
    /// on uninstall. `backup` (if the file pre-existed) is kept on disk as a
    /// recovery artifact but is NOT blindly restored, since the user may have
    /// added other servers/hooks after we installed.
    JsonKey {
        file: PathBuf,
        key_path: Vec<String>,
        backup: Option<PathBuf>,
    },
    /// One group we appended to a JSON array (`hooks.Stop`), identified by the
    /// command it runs. Uninstall removes only the matching group, leaving any
    /// hooks the user added untouched.
    StopHook {
        file: PathBuf,
        command: String,
        backup: Option<PathBuf>,
    },
}

/// The full uninstall ledger, serialized to `manifest.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    version: u32,
    entries: Vec<Entry>,
}

// ---------------------------------------------------------------------------
// Low-level safety-checked filesystem primitives (all honor A8).
// ---------------------------------------------------------------------------

/// Stable 64-bit FNV-1a hash of a byte string. Stored in the manifest beside
/// each file we write so a reinstall can detect whether the file on disk is
/// still ours or was edited/replaced by the user since. FNV is not a crypto
/// hash; it only needs to catch accidental differences, and unlike Rust's
/// `DefaultHasher` its output is stable across compiler versions, which a
/// value persisted to disk must be.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Current wall-clock seconds, used to make backup filenames unique-ish.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Refuse to touch a path whose final component is a symlink — a classic way to
/// trick a privileged writer into clobbering a file elsewhere. `symlink_metadata`
/// does NOT follow the link, so this sees the link itself.
fn refuse_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to write through a symlink: {}", path.display()),
        )),
        _ => Ok(()), // absent, or a real file/dir — both fine
    }
}

/// Assert `path` is under `home`, in two layers:
///
/// 1. **Lexical.** Collapse `..`/`.` and require the result to start with a
///    lexically-normalized `home`. Works even when the target doesn't exist yet,
///    and blocks a crafted path like `~/.claude/../../etc`.
/// 2. **Symlink-aware.** If `home` really exists on disk, `canonicalize` the
///    deepest *existing* ancestor of the target and require it to still resolve
///    under the real `home`. This catches the case a lexical check can't: an
///    intermediate directory (e.g. `~/.claude/skills`) that is itself a symlink
///    pointing outside `$HOME`, which `create_dir_all` would otherwise follow.
///    Skipped when `home` doesn't exist (synthetic paths in some unit tests).
fn assert_under_home(path: &Path, home: &Path) -> io::Result<()> {
    let norm = lexical_normalize(path);
    let home_norm = lexical_normalize(home);
    if !norm.starts_with(&home_norm) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("target escapes home: {}", path.display()),
        ));
    }
    if let Ok(home_real) = home.canonicalize() {
        // Walk up from the (lexical) target to the first ancestor that exists,
        // staying within the lexical home. Canonicalizing it resolves any
        // symlinks in the chain; if the real path leaves home, refuse.
        let mut anc: &Path = &norm;
        loop {
            match anc.canonicalize() {
                Ok(real) => {
                    if !real.starts_with(&home_real) {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!(
                                "target resolves outside home via symlink: {}",
                                path.display()
                            ),
                        ));
                    }
                    break;
                }
                Err(_) => match anc.parent() {
                    Some(p) if p.starts_with(&home_norm) => anc = p,
                    _ => break,
                },
            }
        }
    }
    Ok(())
}

/// Collapse `.` and `..` segments lexically without hitting the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        use std::path::Component::*;
        match comp {
            ParentDir => {
                out.pop();
            }
            CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Atomically write `bytes` to `dest` with mode `mode`: write to a sibling temp
/// file, set perms, then `rename` over the target (rename is atomic on the same
/// filesystem). Callers have already backed up any pre-existing file.
fn atomic_write(dest: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = dest
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent dir"))?;
    let tmp = parent.join(format!(
        ".{}.sumcp-tmp-{}",
        dest.file_name().and_then(|n| n.to_str()).unwrap_or("out"),
        now_secs()
    ));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.sync_all()?;
    }
    // Rename over the destination. On failure, don't leave the temp behind.
    if let Err(e) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Copy a pre-existing regular file to a timestamped backup and return its
/// path. Never reuses an existing backup name: two backups in the same second
/// (say, an install and a quick reinstall) must not overwrite each other,
/// because the earlier one may hold the user's original file.
fn make_backup(path: &Path) -> io::Result<PathBuf> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let secs = now_secs();
    let mut backup = path.with_file_name(format!("{name}.sumcp-bak.{secs}"));
    let mut n = 0u32;
    while backup.exists() {
        n += 1;
        backup = path.with_file_name(format!("{name}.sumcp-bak.{secs}.{n}"));
    }
    fs::copy(path, &backup)?;
    Ok(backup)
}

// ---------------------------------------------------------------------------
// The plan/execute engine. One code path drives both dry-run and apply so the
// printed plan can never lie about what apply will do.
// ---------------------------------------------------------------------------

/// Accumulates human-readable plan lines and, when applying, the manifest
/// entries needed to undo the work.
struct Journal<'a> {
    home: &'a Path,
    apply: bool,
    lines: Vec<String>,
    entries: Vec<Entry>,
}

impl<'a> Journal<'a> {
    fn new(home: &'a Path, apply: bool) -> Self {
        Self {
            home,
            apply,
            lines: Vec::new(),
            entries: Vec::new(),
        }
    }

    /// Ensure a directory exists. Every level we actually create is recorded
    /// as its own `Created` entry, in creation order (topmost first), so the
    /// reverse-order rollback removes deepest first. Uninstall removes each
    /// one with a plain (non-recursive) `remove_dir`: a directory that gained
    /// other content after install simply survives instead of being deleted.
    fn ensure_dir(&mut self, dir: &Path) -> io::Result<()> {
        assert_under_home(dir, self.home)?;
        // Refuse a symlinked directory as a target: even one pointing inside
        // home shouldn't be written *through* (A8: no following symlinks at
        // write targets). Outside-home symlinks are already caught above.
        refuse_symlink(dir)?;
        if dir.exists() {
            return Ok(());
        }
        // Collect every missing level, from the topmost missing ancestor down
        // to `dir` itself. Each is a directory this install brings into being.
        let mut missing = vec![dir.to_path_buf()];
        let mut cur = dir;
        while let Some(parent) = cur.parent() {
            if parent.exists() {
                break;
            }
            missing.push(parent.to_path_buf());
            cur = parent;
        }
        missing.reverse();
        self.lines.push(format!("mkdir  {}  (0700)", dir.display()));
        if self.apply {
            fs::create_dir_all(dir)?;
            for d in missing {
                fs::set_permissions(&d, fs::Permissions::from_mode(0o700))?;
                self.entries.push(Entry::Created {
                    path: d,
                    written_hash: None,
                });
            }
        }
        Ok(())
    }

    /// Place a wholesale file (binary copy or embedded text). Backs up any
    /// pre-existing regular file first and records Created/Replaced accordingly.
    fn place_file(&mut self, dest: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        assert_under_home(dest, self.home)?;
        refuse_symlink(dest)?;
        let existed = dest.exists();
        self.lines.push(format!(
            "{} {}  ({} bytes, {:o})",
            if existed { "replace" } else { "create " },
            dest.display(),
            bytes.len(),
            mode
        ));
        if self.apply {
            let written_hash = Some(fnv1a64(bytes));
            let entry = if existed {
                let backup = make_backup(dest)?;
                Entry::Replaced {
                    path: dest.to_path_buf(),
                    backup,
                    written_hash,
                }
            } else {
                Entry::Created {
                    path: dest.to_path_buf(),
                    written_hash,
                }
            };
            atomic_write(dest, bytes, mode)?;
            self.entries.push(entry);
        }
        Ok(())
    }

    /// Splice `value` into a shared JSON file at `key_path` (e.g.
    /// `["mcpServers","sumcp"]`), preserving everything else. Missing file →
    /// treated as `{}`. Records a `JsonKey` entry for surgical removal.
    fn merge_json(&mut self, file: &Path, key_path: &[&str], value: Value) -> io::Result<()> {
        assert_under_home(file, self.home)?;
        refuse_symlink(file)?;
        let existed = file.exists();
        self.lines.push(format!(
            "merge  {}  ← {}",
            file.display(),
            key_path.join(".")
        ));
        if self.apply {
            let mut root = read_json_object(file)?;
            validate_merge_path(&root, key_path, file)?;
            insert_nested(&mut root, key_path, value);
            let pretty = serde_json::to_vec_pretty(&Value::Object(root))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let backup = if existed {
                Some(make_backup(file)?)
            } else {
                None
            };
            atomic_write(file, &pretty, 0o600)?;
            self.entries.push(Entry::JsonKey {
                file: file.to_path_buf(),
                key_path: key_path.iter().map(|s| s.to_string()).collect(),
                backup,
            });
        }
        Ok(())
    }

    /// Append our Stop-hook group to `file`'s `hooks.Stop` array (idempotent:
    /// skips the append if our command is already present). Records a `StopHook`
    /// entry so uninstall removes only our group.
    fn append_stop_hook(&mut self, file: &Path, script: &Path) -> io::Result<()> {
        assert_under_home(file, self.home)?;
        refuse_symlink(file)?;
        let existed = file.exists();
        let command = script.to_string_lossy().to_string();
        self.lines.push(format!(
            "hook   {}  ← hooks.Stop += {command}",
            file.display()
        ));
        if self.apply {
            let mut root = read_json_object(file)?;
            validate_hooks_shape(&root, file)?;
            if !stop_contains_command(&root, &command) {
                append_stop_group(&mut root, script);
            }
            let pretty = serde_json::to_vec_pretty(&Value::Object(root))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            let backup = if existed {
                Some(make_backup(file)?)
            } else {
                None
            };
            atomic_write(file, &pretty, 0o600)?;
            self.entries.push(Entry::StopHook {
                file: file.to_path_buf(),
                command,
                backup,
            });
        }
        Ok(())
    }
}

/// Read a JSON file into an object map. Absent file → empty map. A file whose
/// top level isn't an object is an error (we won't guess how to merge into it).
fn read_json_object(file: &Path) -> io::Result<Map<String, Value>> {
    if !file.exists() {
        return Ok(Map::new());
    }
    let text = fs::read_to_string(file)?;
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
    {
        Value::Object(m) => Ok(m),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not a JSON object", file.display()),
        )),
    }
}

/// Refuse to merge into a JSON file whose existing nodes along `key_path` are
/// not objects. Coercing (or panicking on) someone's hand-shaped config would
/// silently destroy it; erroring out leaves the file byte-for-byte untouched.
fn validate_merge_path(
    root: &Map<String, Value>,
    key_path: &[&str],
    file: &Path,
) -> io::Result<()> {
    let mut cur = root;
    if let Some((_, parents)) = key_path.split_last() {
        for p in parents {
            match cur.get(*p) {
                None => return Ok(()), // absent from here down: we'd create it
                Some(Value::Object(o)) => cur = o,
                Some(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "{}: key \"{p}\" exists but is not a JSON object; refusing to modify",
                            file.display()
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Same idea for the Stop hook target: `hooks` must be absent or an object,
/// and `hooks.Stop` absent or an array. Anything else is refused untouched.
fn validate_hooks_shape(root: &Map<String, Value>, file: &Path) -> io::Result<()> {
    match root.get("hooks") {
        None => Ok(()),
        Some(Value::Object(h)) => match h.get("Stop") {
            None | Some(Value::Array(_)) => Ok(()),
            Some(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: hooks.Stop exists but is not an array; refusing to modify",
                    file.display()
                ),
            )),
        },
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{}: \"hooks\" exists but is not a JSON object; refusing to modify",
                file.display()
            ),
        )),
    }
}

/// Insert `value` at a nested key path, creating intermediate objects as needed.
/// Callers must have run `validate_merge_path` first so the `expect` below can
/// never fire on a user's odd-shaped config.
fn insert_nested(root: &mut Map<String, Value>, key_path: &[&str], value: Value) {
    if let Some((last, parents)) = key_path.split_last() {
        let mut cur = root;
        for p in parents {
            cur = cur
                .entry(p.to_string())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("intermediate JSON node is an object");
        }
        cur.insert(last.to_string(), value);
    }
}

/// Remove a nested key path from a JSON object if present, pruning any parent
/// object we just left empty (so `mcpServers.sumcp` removal doesn't strand an
/// empty `"mcpServers": {}`). Returns whether it removed anything. Recursive so
/// the pruning unwinds naturally back up the path.
fn remove_nested(root: &mut Map<String, Value>, key_path: &[String]) -> bool {
    match key_path.split_first() {
        None => false,
        // Leaf: remove the key here.
        Some((head, [])) => root.remove(head).is_some(),
        // Interior: recurse, then prune this level if the child is now empty.
        Some((head, rest)) => {
            let removed = match root.get_mut(head).and_then(|v| v.as_object_mut()) {
                Some(child) => remove_nested(child, rest),
                None => false,
            };
            if removed
                && let Some(obj) = root.get(head).and_then(|v| v.as_object())
                && obj.is_empty()
            {
                root.remove(head);
            }
            removed
        }
    }
}

// ---------------------------------------------------------------------------
// The Stop-hook value we merge into settings.json.
// ---------------------------------------------------------------------------

/// Build ONE `hooks.Stop` group registering our script. Claude Code's shape for
/// Stop hooks: an array of groups, each `{ "hooks": [ {type,command} ] }`. Stop
/// events take no `matcher`. We append this group rather than replace the array,
/// so a user's existing Stop hooks survive.
fn stop_hook_group(script: &Path) -> Value {
    serde_json::json!({
        "hooks": [
            { "type": "command", "command": script.to_string_lossy() }
        ]
    })
}

/// Does any group in `hooks.Stop` already reference `command`? (Idempotency +
/// uninstall targeting both key off this.)
fn stop_contains_command(root: &Map<String, Value>, command: &str) -> bool {
    stop_array(root)
        .map(|arr| arr.iter().any(|g| group_has_command(g, command)))
        .unwrap_or(false)
}

/// Borrow the `hooks.Stop` array if it exists and is an array.
fn stop_array(root: &Map<String, Value>) -> Option<&Vec<Value>> {
    root.get("hooks")?.get("Stop")?.as_array()
}

/// Does one Stop group contain a hook whose command equals `command`?
fn group_has_command(group: &Value, command: &str) -> bool {
    group
        .get("hooks")
        .and_then(|h| h.as_array())
        .map(|hooks| {
            hooks
                .iter()
                .any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command))
        })
        .unwrap_or(false)
}

/// Append our Stop group to `root.hooks.Stop`, creating the `hooks` object and
/// `Stop` array as needed. Callers must have run `validate_hooks_shape` first:
/// a pre-existing `hooks`/`Stop` of the wrong shape is refused there, never
/// coerced (and thereby destroyed) here.
fn append_stop_group(root: &mut Map<String, Value>, script: &Path) {
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let stop = hooks
        .as_object_mut()
        .expect("validate_hooks_shape guarantees hooks is an object")
        .entry("Stop".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    stop.as_array_mut()
        .expect("validate_hooks_shape guarantees hooks.Stop is an array")
        .push(stop_hook_group(script));
}

/// Remove any Stop group referencing `command`, pruning an emptied `Stop`/`hooks`.
/// Returns whether anything was removed.
fn remove_stop_command(root: &mut Map<String, Value>, command: &str) -> bool {
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };
    let Some(stop) = hooks.get_mut("Stop").and_then(|s| s.as_array_mut()) else {
        return false;
    };
    let before = stop.len();
    stop.retain(|g| !group_has_command(g, command));
    let removed = stop.len() != before;
    if stop.is_empty() {
        hooks.remove("Stop");
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    removed
}

/// The `mcpServers.sumcp` value: run the installed MCP binary directly.
fn server_value(mcp_bin: &Path) -> Value {
    serde_json::json!({
        "command": mcp_bin.to_string_lossy(),
        "args": []
    })
}

// ---------------------------------------------------------------------------
// Install / uninstall orchestration.
// ---------------------------------------------------------------------------

/// The identity of the thing a manifest entry describes, used to match entries
/// across a reinstall: same file path, same JSON key, or same hook command.
fn entry_key(e: &Entry) -> (u8, PathBuf, String) {
    match e {
        // Created and Replaced share a tag on purpose: either way the entry is
        // "about" that path, and a reinstall pairs old and new entries by it.
        Entry::Created { path, .. } | Entry::Replaced { path, .. } => {
            (0, path.clone(), String::new())
        }
        Entry::JsonKey { file, key_path, .. } => (1, file.clone(), key_path.join(".")),
        Entry::StopHook { file, command, .. } => (2, file.clone(), command.clone()),
    }
}

/// A copy of `base` with its `written_hash` swapped for `nh` (files only).
/// Used when a reinstall carries an old entry forward: the entry's history
/// (created vs replaced, original backup) stays, but the hash must describe
/// the bytes THIS run just wrote, or the next reinstall would see false drift.
fn with_written_hash(base: &Entry, nh: Option<u64>) -> Entry {
    match base.clone() {
        Entry::Created { path, .. } => Entry::Created {
            path,
            written_hash: nh,
        },
        Entry::Replaced { path, backup, .. } => Entry::Replaced {
            path,
            backup,
            written_hash: nh,
        },
        other => other,
    }
}

/// Did the file we just backed up during a reinstall still hold exactly what
/// the PREVIOUS install wrote (per the hash recorded in the old manifest)?
/// `None` means the old manifest predates hashes; we assume unmodified, which
/// matches the old behavior and only ever affects pre-hash installs.
fn backup_matches_recorded_hash(prev: &Entry, backup: &Path) -> bool {
    let recorded = match prev {
        Entry::Created { written_hash, .. } | Entry::Replaced { written_hash, .. } => *written_hash,
        _ => None,
    };
    match recorded {
        None => true,
        // Unreadable backup counts as drift: when unsure, keep the backup.
        Some(h) => fs::read(backup).map(|b| fnv1a64(&b) == h).unwrap_or(false),
    }
}

/// Fold a previous install's manifest entries into a reinstall's journal.
///
/// When both manifests mention the same target and the file still held our
/// bytes, the OLD entry wins: it remembers the state before suMCP ever touched
/// the machine (for example the backup of the user's original skill file), and
/// this run's backup is just a copy of our own previous version, returned
/// separately for deletion once the merged manifest is safely on disk.
///
/// But if the content had DRIFTED (the user edited or replaced our file after
/// install), this run's backup is the only copy of their version. The new
/// `Replaced` entry then wins, so uninstall restores their file, and nothing
/// is deleted.
fn merge_manifests(prev: &[Entry], new: &[Entry]) -> (Vec<Entry>, Vec<PathBuf>) {
    let mut merged = Vec::new();
    let mut redundant = Vec::new();
    let mut consumed = vec![false; new.len()];
    for p in prev {
        let pk = entry_key(p);
        let hit = new
            .iter()
            .enumerate()
            .find(|(i, n)| !consumed[*i] && entry_key(n) == pk);
        match hit {
            // Not touched this run (e.g. a dir that already existed): the old
            // entry is still the whole story, carry it forward.
            None => merged.push(p.clone()),
            Some((i, n)) => {
                consumed[i] = true;
                match n {
                    Entry::Replaced {
                        backup,
                        written_hash,
                        ..
                    } => {
                        if backup_matches_recorded_hash(p, backup) {
                            merged.push(with_written_hash(p, *written_hash));
                            redundant.push(backup.clone());
                        } else {
                            merged.push(n.clone());
                        }
                    }
                    // File was missing this run (user deleted ours) and got
                    // recreated: the old entry still describes the pre-suMCP
                    // state, carry it forward with the fresh content hash.
                    Entry::Created { written_hash, .. } => {
                        merged.push(with_written_hash(p, *written_hash));
                    }
                    Entry::JsonKey {
                        backup: Some(b), ..
                    }
                    | Entry::StopHook {
                        backup: Some(b), ..
                    } => {
                        merged.push(p.clone());
                        redundant.push(b.clone());
                    }
                    _ => merged.push(p.clone()),
                }
            }
        }
    }
    for (i, n) in new.iter().enumerate() {
        if !consumed[i] {
            merged.push(n.clone());
        }
    }
    (merged, redundant)
}

/// Roll back a failed apply. On a failed REINSTALL, skip undoing the JSON keys
/// and hook groups the previous (still valid) manifest also owns: their values
/// are identical between versions, and removing them would break the previous
/// install this failure is supposed to leave working. Files are always undone:
/// `Replaced` restores the pre-reinstall version from its backup.
fn rollback_failed_install(entries: &[Entry], prev: Option<&Manifest>) {
    match prev {
        None => rollback(entries),
        Some(m) => {
            let undo: Vec<Entry> = entries
                .iter()
                .filter(|e| match e {
                    Entry::JsonKey { .. } | Entry::StopHook { .. } => {
                        let k = entry_key(e);
                        !m.entries.iter().any(|p| entry_key(p) == k)
                    }
                    _ => true,
                })
                .cloned()
                .collect();
            rollback(&undo);
        }
    }
}

/// Run the full install plan through a fresh journal. On apply, the manifest
/// write is part of the same fallible transaction as every other step: if ANY
/// of it fails (the manifest included), everything already done is rolled back.
fn run_install(paths: &Paths, exe_dir: &Path, apply: bool) -> io::Result<Vec<String>> {
    let home = paths.home.clone();
    let mut j = Journal::new(&home, apply);

    // Reinstall support: if a prior manifest exists we do NOT tear the old
    // install down first. We install over it (place_file backs files up) and,
    // on success, fold the old manifest's entries into the new one so uninstall
    // still knows about every dir and backup from the first install. A failed
    // reinstall therefore leaves the previous install intact: its manifest is
    // still on disk and still accurate.
    let prev: Option<Manifest> = if paths.manifest().exists() {
        let parsed = fs::read_to_string(paths.manifest())
            .ok()
            .and_then(|t| serde_json::from_str::<Manifest>(&t).ok())
            .filter(|m| m.version == MANIFEST_VERSION);
        match parsed {
            Some(m) => {
                j.lines.push(format!(
                    "reinstall: carrying forward the install recorded in {}",
                    paths.manifest().display()
                ));
                Some(m)
            }
            None => {
                j.lines.push(format!(
                    "warning: unreadable manifest at {}; treating as a fresh install",
                    paths.manifest().display()
                ));
                None
            }
        }
    } else {
        None
    };

    // The closure lets us use `?` for early-exit and still catch the error to
    // roll back. (Rust has no try/finally; this is the idiomatic stand-in.)
    let steps = |j: &mut Journal| -> io::Result<()> {
        // 0. Read and validate everything fallible BEFORE the first write, so
        //    the common failure modes (missing sibling binary, corrupt or
        //    odd-shaped config JSON) abort with zero changes on disk.
        let src_sumcp = exe_dir.join("sumcp");
        let src_mcp = exe_dir.join("sumcp-mcp");
        let sumcp_bytes = fs::read(&src_sumcp).map_err(|e| {
            io::Error::new(e.kind(), format!("reading {}: {e}", src_sumcp.display()))
        })?;
        let mcp_bytes = fs::read(&src_mcp)
            .map_err(|e| io::Error::new(e.kind(), format!("reading {}: {e}", src_mcp.display())))?;
        let cj = read_json_object(&paths.claude_json())?;
        validate_merge_path(&cj, &["mcpServers", SERVER_KEY], &paths.claude_json())?;
        let sj = read_json_object(&paths.settings_json())?;
        validate_hooks_shape(&sj, &paths.settings_json())?;

        // 1. Our own tree.
        j.ensure_dir(&paths.bin())?;
        j.ensure_dir(&paths.hooks())?;

        // 2. Place the two binaries read above.
        j.place_file(&paths.installed_sumcp(), &sumcp_bytes, 0o700)?;
        j.place_file(&paths.installed_mcp(), &mcp_bytes, 0o700)?;

        // 3. The Stop-hook script, with the installed sumcp path substituted in.
        let script =
            HOOK_TEMPLATE.replace("__SUMCP_BIN__", &paths.installed_sumcp().to_string_lossy());
        j.place_file(&paths.hook_script(), script.as_bytes(), 0o700)?;

        // 4. The debrief skill (embedded).
        j.ensure_dir(&paths.skill_dest())?;
        j.place_file(
            &paths.skill_dest().join("SKILL.md"),
            SKILL_MD.as_bytes(),
            0o600,
        )?;

        // 5. Register the MCP server (user scope) in ~/.claude.json.
        j.merge_json(
            &paths.claude_json(),
            &["mcpServers", SERVER_KEY],
            server_value(&paths.installed_mcp()),
        )?;

        // 6. Register the Stop hook (user scope) in ~/.claude/settings.json.
        //    Appended (not replaced) so existing Stop hooks survive.
        j.append_stop_hook(&paths.settings_json(), &paths.hook_script())?;

        // 7. The manifest, written last so it reflects everything above, but
        //    still inside this fallible block: if the write fails, the error
        //    path below rolls the whole install back instead of stranding an
        //    installation that has no uninstall ledger.
        if j.apply {
            let (entries, redundant) = match &prev {
                Some(m) => merge_manifests(&m.entries, &j.entries),
                None => (j.entries.clone(), Vec::new()),
            };
            let manifest = Manifest {
                version: MANIFEST_VERSION,
                entries,
            };
            let bytes = serde_json::to_vec_pretty(&manifest)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            atomic_write(&paths.manifest(), &bytes, 0o600)?;
            // Only now, with the merged manifest safely on disk, drop this
            // run's redundant backups (copies of files we already owned).
            for b in redundant {
                let _ = fs::remove_file(&b);
            }
        }
        Ok(())
    };

    match steps(&mut j) {
        Ok(()) => Ok(j.lines),
        Err(e) => {
            if apply {
                // Best-effort rollback of whatever we managed to do.
                rollback_failed_install(&j.entries, prev.as_ref());
            }
            Err(e)
        }
    }
}

/// Undo one manifest entry. Three outcomes:
///
/// - `Ok(true)`: fully undone (or already gone; undo is idempotent).
/// - `Ok(false)`: deliberately left in place. Only directories do this: a
///   directory that still has content is not ours to force-remove.
/// - `Err(..)`: the undo failed; the caller should keep the entry recorded
///   so the operation can be retried.
fn undo_entry(entry: &Entry) -> io::Result<bool> {
    /// Treat "already absent" as success so a retried uninstall doesn't trip
    /// over the steps that worked the first time.
    fn remove_file_idempotent(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
    /// Write back a shared JSON file after our key/hook was removed from it.
    /// `backup.is_none()` means the file didn't exist before install (we
    /// created it), so if it's now empty we delete it rather than leave `{}`.
    fn write_back(
        file: &Path,
        root: Map<String, Value>,
        backup: &Option<PathBuf>,
    ) -> io::Result<()> {
        if backup.is_none() && root.is_empty() {
            remove_file_idempotent(file)
        } else {
            let bytes = serde_json::to_vec_pretty(&Value::Object(root))
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            atomic_write(file, &bytes, 0o600)
        }
    }

    match entry {
        Entry::Created { path, .. } => {
            // A directory is removed only if empty (`remove_dir`, never
            // `remove_dir_all`): whatever the user added inside it after
            // install is not ours to delete.
            if path.is_dir() {
                match fs::remove_dir(path) {
                    Ok(()) => Ok(true),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
                    Err(_) => Ok(false), // non-empty (or stuck): leave it be
                }
            } else {
                remove_file_idempotent(path).map(|()| true)
            }
        }
        Entry::Replaced { path, backup, .. } => {
            // Restore the pre-existing file we overwrote. A missing backup is
            // a real failure: the restore cannot happen.
            fs::rename(backup, path).map(|()| true)
        }
        Entry::JsonKey {
            file,
            key_path,
            backup,
        } => {
            let mut root = read_json_object(file)?;
            if remove_nested(&mut root, key_path) {
                write_back(file, root, backup)?;
            }
            Ok(true)
        }
        Entry::StopHook {
            file,
            command,
            backup,
        } => {
            let mut root = read_json_object(file)?;
            if remove_stop_command(&mut root, command) {
                write_back(file, root, backup)?;
            }
            Ok(true)
        }
    }
}

/// Undo a list of manifest entries, best-effort: used to roll back a failed
/// install, where pressing on past individual errors beats stranding the rest.
/// (`uninstall` instead walks entries itself so it can report failures and
/// keep a retryable ledger.)
fn rollback(entries: &[Entry]) {
    // Reverse order: undo last action first, and empty out directories'
    // contents before the directories themselves come up.
    for entry in entries.iter().rev() {
        let _ = undo_entry(entry);
    }
}

/// Uninstall by replaying the manifest. Dry-run prints; apply removes.
fn run_uninstall(paths: &Paths, apply: bool) -> io::Result<Vec<String>> {
    let manifest_path = paths.manifest();
    if !manifest_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no manifest at {} — nothing installed (or installed by hand)",
                manifest_path.display()
            ),
        ));
    }
    let text = fs::read_to_string(&manifest_path)?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "manifest version {} unsupported (expected {})",
                manifest.version, MANIFEST_VERSION
            ),
        ));
    }

    let mut lines = Vec::new();
    for entry in &manifest.entries {
        lines.push(entry_undo_desc(entry));
    }
    if apply {
        // Walk the ledger in reverse (undo last action first), keeping every
        // entry that is not fully undone. The manifest is NOT deleted up
        // front: it is the only record of what remains, and a failed step must
        // leave a retryable ledger behind rather than vanish silently.
        let mut kept_rev: Vec<Entry> = Vec::new();
        let mut errors: Vec<String> = Vec::new();
        for entry in manifest.entries.iter().rev() {
            match undo_entry(entry) {
                Ok(true) => {}
                Ok(false) => kept_rev.push(entry.clone()), // dir with content
                Err(e) => {
                    kept_rev.push(entry.clone());
                    errors.push(format!("{}: {e}", entry_undo_desc(entry)));
                }
            }
        }
        if errors.is_empty() {
            // Everything undone except (at most) directories that were still
            // waiting on the manifest file inside them. Remove the ledger,
            // then sweep those directories; `kept_rev` is already ordered
            // children-before-parents.
            let _ = fs::remove_file(&manifest_path);
            for entry in &kept_rev {
                if let Entry::Created { path, .. } = entry
                    && path.is_dir()
                {
                    let _ = fs::remove_dir(path);
                }
            }
        } else {
            // Rewrite the manifest with what remains (back in original order)
            // so `uninstall` can simply be run again after the cause is fixed.
            kept_rev.reverse();
            let retained = Manifest {
                version: MANIFEST_VERSION,
                entries: kept_rev,
            };
            let mut msg = format!("uninstall incomplete: {}", errors.join("; "));
            let rewrite = serde_json::to_vec_pretty(&retained)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
                .and_then(|bytes| atomic_write(&manifest_path, &bytes, 0o600));
            match rewrite {
                Ok(()) => msg.push_str(
                    "; the remaining steps were kept in the manifest, re-run uninstall to retry",
                ),
                Err(e) => msg.push_str(&format!(
                    "; additionally failed to rewrite the manifest: {e}"
                )),
            }
            return Err(io::Error::other(msg));
        }
    }
    Ok(lines)
}

/// One human-readable line describing what undoing `entry` means. Used both
/// for the dry-run plan and to name a failed step in an error message.
fn entry_undo_desc(entry: &Entry) -> String {
    match entry {
        Entry::Created { path, .. } => format!("remove  {}", path.display()),
        Entry::Replaced { path, backup, .. } => {
            format!("restore {}  ← {}", path.display(), backup.display())
        }
        Entry::JsonKey { file, key_path, .. } => {
            format!("unmerge {}  ✗ {}", file.display(), key_path.join("."))
        }
        Entry::StopHook { file, .. } => format!("unhook  {}  ✗ hooks.Stop", file.display()),
    }
}

// ---------------------------------------------------------------------------
// Public command entry points, called from main.rs.
// ---------------------------------------------------------------------------

/// Locate the directory holding the running `sumcp` binary (its siblings are the
/// binaries we copy). `current_exe` can be spoofed on some platforms, but for a
/// user installing their own build it's exactly right.
fn exe_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "cannot locate binary directory"))
}

/// `sumcp install [--apply]`.
pub fn cmd_install(apply: bool) -> io::Result<()> {
    let paths = Paths::from_env()?;
    let dir = exe_dir()?;
    let lines = run_install(&paths, &dir, apply)?;
    print_plan("install", apply, &lines);
    Ok(())
}

/// `sumcp uninstall [--apply]`.
pub fn cmd_uninstall(apply: bool) -> io::Result<()> {
    let paths = Paths::from_env()?;
    let lines = run_uninstall(&paths, apply)?;
    print_plan("uninstall", apply, &lines);
    Ok(())
}

/// Shared plan printer: header, each line, and a footer telling the user how to
/// actually apply (or confirming it was applied).
fn print_plan(verb: &str, apply: bool, lines: &[String]) {
    if apply {
        println!("sumcp {verb}: applied {} step(s):", lines.len());
    } else {
        println!("sumcp {verb} (dry-run — nothing written). Would do:");
    }
    for line in lines {
        println!("  {line}");
    }
    if !apply {
        println!("\nRe-run with --apply to execute.");
    }
}

// ---------------------------------------------------------------------------
// Unit tests for the pure logic (JSON merge/remove, path safety, manifest
// round-trip). End-to-end install/uninstall lives in tests/install.rs.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_remove_nested_roundtrip() {
        let mut root = Map::new();
        // Pre-existing unrelated content must survive.
        root.insert("model".into(), Value::String("opus".into()));
        insert_nested(
            &mut root,
            &["mcpServers", "sumcp"],
            serde_json::json!({"command": "x"}),
        );
        assert!(root.contains_key("model"), "unrelated key clobbered");
        assert_eq!(root["mcpServers"]["sumcp"]["command"], "x");

        let removed = remove_nested(&mut root, &["mcpServers".into(), "sumcp".into()]);
        assert!(removed);
        // The now-empty `mcpServers` parent is pruned, but siblings survive.
        assert!(!root.contains_key("mcpServers"), "empty parent not pruned");
        assert!(root.contains_key("model"), "unrelated key lost on removal");
    }

    #[test]
    fn remove_nested_keeps_sibling_servers() {
        let mut root = Map::new();
        insert_nested(
            &mut root,
            &["mcpServers", "other"],
            serde_json::json!({"command": "y"}),
        );
        insert_nested(
            &mut root,
            &["mcpServers", "sumcp"],
            serde_json::json!({"command": "x"}),
        );
        assert!(remove_nested(
            &mut root,
            &["mcpServers".into(), "sumcp".into()]
        ));
        // Parent stays because a sibling server remains.
        assert_eq!(root["mcpServers"]["other"]["command"], "y");
        assert!(
            root["mcpServers"]
                .as_object()
                .unwrap()
                .get("sumcp")
                .is_none()
        );
    }

    #[test]
    fn remove_nested_absent_is_false() {
        let mut root = Map::new();
        assert!(!remove_nested(&mut root, &["a".into(), "b".into()]));
    }

    #[test]
    fn assert_under_home_blocks_escape() {
        let home = Path::new("/Users/x");
        assert!(assert_under_home(Path::new("/Users/x/.claude/sumcp"), home).is_ok());
        assert!(assert_under_home(Path::new("/Users/x/../y/evil"), home).is_err());
        assert!(assert_under_home(Path::new("/etc/passwd"), home).is_err());
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let m = Manifest {
            version: MANIFEST_VERSION,
            entries: vec![
                Entry::Created {
                    path: "/a/b".into(),
                    written_hash: None,
                },
                Entry::Replaced {
                    path: "/c".into(),
                    backup: "/c.bak".into(),
                    written_hash: Some(fnv1a64(b"content")),
                },
                Entry::JsonKey {
                    file: "/d.json".into(),
                    key_path: vec!["hooks".into(), "Stop".into()],
                    backup: None,
                },
            ],
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.entries, m.entries);
    }

    #[test]
    fn read_json_object_absent_is_empty() {
        let missing = Path::new("/definitely/not/here/x.json");
        assert!(read_json_object(missing).unwrap().is_empty());
    }

    // --- A8 end-to-end scenarios against a temp $HOME + fake exe dir ---------

    use tempfile::{TempDir, tempdir};

    /// A directory with dummy `sumcp` / `sumcp-mcp` "binaries" to copy. Returns
    /// the guard (keep it alive to keep the dir) plus its path.
    fn fake_exe_dir() -> (TempDir, PathBuf) {
        let d = tempdir().unwrap();
        fs::write(d.path().join("sumcp"), b"#!fake sumcp\n").unwrap();
        fs::write(d.path().join("sumcp-mcp"), b"#!fake mcp\n").unwrap();
        let p = d.path().to_path_buf();
        (d, p)
    }

    /// A temp home with `~/.claude` already present (mirrors a real machine,
    /// where install must NOT own/remove `~/.claude` itself).
    fn temp_home() -> TempDir {
        let h = tempdir().unwrap();
        fs::create_dir_all(h.path().join(".claude")).unwrap();
        h
    }

    fn read_json(p: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap()
    }

    #[test]
    fn fresh_install_writes_all_targets() {
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        assert!(paths.installed_sumcp().exists());
        assert!(paths.installed_mcp().exists());
        assert!(paths.hook_script().exists());
        assert!(paths.skill_dest().join("SKILL.md").exists());
        assert!(paths.manifest().exists());

        let cj = read_json(&paths.claude_json());
        assert_eq!(
            cj["mcpServers"]["sumcp"]["command"].as_str(),
            Some(paths.installed_mcp().to_string_lossy().as_ref())
        );
        let sj = read_json(&paths.settings_json());
        assert_eq!(sj["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_preserves_preexisting_config() {
        let home = temp_home();
        let settings = home.path().join(".claude/settings.json");
        fs::write(
            &settings,
            serde_json::to_vec_pretty(&serde_json::json!({
                "model": "opus",
                "hooks": { "Stop": [ { "hooks": [ { "type":"command", "command":"/usr/bin/true" } ] } ] }
            }))
            .unwrap(),
        )
        .unwrap();
        let cjp = home.path().join(".claude.json");
        fs::write(
            &cjp,
            serde_json::to_vec_pretty(&serde_json::json!({
                "mcpServers": { "other": { "command": "otherbin", "args": [] } }
            }))
            .unwrap(),
        )
        .unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let sj = read_json(&settings);
        assert_eq!(sj["model"], "opus", "unrelated setting clobbered");
        assert_eq!(
            sj["hooks"]["Stop"].as_array().unwrap().len(),
            2,
            "our hook should append, not replace the user's"
        );
        let cj = read_json(&cjp);
        assert!(cj["mcpServers"]["other"].is_object(), "user server lost");
        assert!(cj["mcpServers"]["sumcp"].is_object(), "our server missing");

        // Backups of both pre-existing files exist.
        let has_backup = |dir: &Path| {
            fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().contains(".sumcp-bak."))
        };
        assert!(
            has_backup(&home.path().join(".claude")),
            "no settings backup"
        );
        assert!(has_backup(home.path()), "no .claude.json backup");
    }

    #[test]
    fn reinstall_idempotent_then_uninstall_preserves_user_edits() {
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();
        run_install(&paths, &exe, true).unwrap(); // second time: no-op-ish, still Ok

        let sj = read_json(&paths.settings_json());
        let ours = paths.hook_script().to_string_lossy().to_string();
        let dupes = sj["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|g| group_has_command(g, &ours))
            .count();
        assert_eq!(dupes, 1, "reinstall duplicated the Stop hook");

        // User adds their own MCP server AFTER install.
        let cjp = paths.claude_json();
        let mut cj = read_json(&cjp);
        cj["mcpServers"]["userthing"] = serde_json::json!({ "command": "z", "args": [] });
        fs::write(&cjp, serde_json::to_vec_pretty(&cj).unwrap()).unwrap();

        run_uninstall(&paths, true).unwrap();

        assert!(!paths.sumcp().exists(), "uninstall left the sumcp tree");
        assert!(!paths.skill_dest().exists(), "uninstall left the skill");
        let cj2 = read_json(&cjp);
        assert!(
            cj2["mcpServers"]["userthing"].is_object(),
            "user server lost"
        );
        assert!(
            cj2["mcpServers"].get("sumcp").is_none(),
            "our server not removed"
        );
    }

    #[test]
    fn preexisting_skill_is_backed_up_and_restored() {
        let home = temp_home();
        // A user (or older suMCP) already has a debrief skill with custom text.
        let skill = home.path().join(".claude/skills/debrief");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), b"MY CUSTOM SKILL").unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        // Install replaced it with the embedded skill (and made a backup).
        let installed = fs::read_to_string(skill.join("SKILL.md")).unwrap();
        assert_ne!(installed, "MY CUSTOM SKILL", "skill not replaced");
        assert!(
            fs::read_dir(&skill)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| { e.file_name().to_string_lossy().contains(".sumcp-bak.") }),
            "no backup of the pre-existing skill"
        );

        // Uninstall restores the user's original content.
        run_uninstall(&paths, true).unwrap();
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "MY CUSTOM SKILL",
            "uninstall did not restore the backed-up skill"
        );
    }

    #[test]
    fn symlinked_skill_dir_target_is_refused() {
        let home = temp_home();
        let outside = tempdir().unwrap();
        fs::create_dir_all(home.path().join(".claude/skills")).unwrap();
        // The skill destination itself is a symlink pointing outside home.
        std::os::unix::fs::symlink(outside.path(), home.path().join(".claude/skills/debrief"))
            .unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "symlinked skill dir must be refused"
        );
        assert!(
            !outside.path().join("SKILL.md").exists(),
            "wrote through the symlinked target dir"
        );
        assert!(!paths.sumcp().exists(), "not rolled back");
    }

    #[test]
    fn symlinked_skills_parent_is_refused() {
        let home = temp_home();
        let outside = tempdir().unwrap();
        // `~/.claude/skills` is a symlink outside home; `debrief` doesn't exist yet,
        // so only the canonicalized-ancestor check can catch this.
        std::os::unix::fs::symlink(outside.path(), home.path().join(".claude/skills")).unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "symlinked skills parent must be refused"
        );
        assert!(
            !outside.path().join("debrief").exists(),
            "created a dir through the symlinked parent"
        );
        assert!(!paths.sumcp().exists(), "not rolled back");
    }

    #[test]
    fn partial_failure_rolls_back() {
        let home = temp_home();
        let cjp = home.path().join(".claude.json");
        fs::write(
            &cjp,
            serde_json::to_vec_pretty(&serde_json::json!({
                "mcpServers": { "other": { "command": "o", "args": [] } }
            }))
            .unwrap(),
        )
        .unwrap();
        // Make settings.json a symlink so step 6 refuses and the install fails.
        // The decoy holds valid JSON so the up-front validation (which reads
        // through the symlink) passes and the failure happens mid-mutation.
        let decoy = home.path().join("decoy");
        fs::write(&decoy, b"{}").unwrap();
        std::os::unix::fs::symlink(&decoy, home.path().join(".claude/settings.json")).unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "symlinked settings.json should abort install"
        );

        // Everything we started is rolled back; the user's config is intact.
        assert!(!paths.sumcp().exists(), "rollback left the sumcp tree");
        assert!(!paths.skill_dest().exists(), "rollback left the skill");
        let cj = read_json(&cjp);
        assert!(
            cj["mcpServers"]["other"].is_object(),
            "rollback lost user server"
        );
        assert!(
            cj["mcpServers"].get("sumcp").is_none(),
            "rollback left our server key"
        );
        assert_eq!(
            fs::read_to_string(&decoy).unwrap(),
            "{}",
            "symlink target written"
        );
    }

    // --- Regression tests from the T5.2 installer review ---------------------

    #[test]
    fn uninstall_preserves_sibling_skill_added_later() {
        // Install when `~/.claude/skills` does not exist yet, so the installer
        // creates it. A skill the user adds afterwards must survive uninstall:
        // we only own the directories we created, never their later contents.
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let other = home.path().join(".claude/skills/other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), b"USER SKILL").unwrap();

        run_uninstall(&paths, true).unwrap();

        assert!(!paths.skill_dest().exists(), "our skill not removed");
        assert_eq!(
            fs::read(other.join("SKILL.md")).unwrap(),
            b"USER SKILL",
            "uninstall deleted a sibling skill it never installed"
        );
    }

    #[test]
    fn uninstall_preserves_files_added_to_claude_dir_it_created() {
        // Harsher variant: `~/.claude` itself does not exist at install time.
        // Anything the user later puts under it must survive uninstall.
        let home = tempdir().unwrap(); // note: no .claude inside
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let notes = home.path().join(".claude/notes.txt");
        fs::write(&notes, b"precious").unwrap();

        run_uninstall(&paths, true).unwrap();

        assert!(!paths.sumcp().exists(), "uninstall left the sumcp tree");
        assert_eq!(
            fs::read(&notes).unwrap(),
            b"precious",
            "uninstall deleted a user file inside ~/.claude"
        );
    }

    #[test]
    fn failed_reinstall_missing_binary_preserves_previous_install() {
        // An upgrade attempt whose source dir lacks `sumcp-mcp` must fail
        // WITHOUT touching the working install it was meant to replace.
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let broken = tempdir().unwrap();
        fs::write(broken.path().join("sumcp"), b"#!new sumcp\n").unwrap();
        // no sumcp-mcp sibling
        assert!(
            run_install(&paths, broken.path(), true).is_err(),
            "reinstall with a missing binary should fail"
        );

        assert_eq!(
            fs::read(paths.installed_sumcp()).unwrap(),
            b"#!fake sumcp\n",
            "previous sumcp binary lost"
        );
        assert!(paths.installed_mcp().exists(), "previous mcp binary lost");
        assert!(paths.manifest().exists(), "previous manifest lost");
        let cj = read_json(&paths.claude_json());
        assert!(
            cj["mcpServers"]["sumcp"].is_object(),
            "server registration lost"
        );
        let sj = read_json(&paths.settings_json());
        assert_eq!(
            sj["hooks"]["Stop"].as_array().unwrap().len(),
            1,
            "hook registration lost"
        );

        // The surviving install must still uninstall cleanly.
        run_uninstall(&paths, true).unwrap();
        assert!(!paths.sumcp().exists());
    }

    #[test]
    fn failed_reinstall_midway_preserves_previous_install() {
        // Failure AFTER the reinstall already replaced files: rollback must
        // restore the previous binaries and leave the old registrations alone.
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        // Sabotage the hook registration step: settings.json becomes a symlink
        // (valid JSON behind it, so any early validation passes and only the
        // write itself is refused, well after the binaries were replaced).
        let decoy = home.path().join("decoy.json");
        fs::write(&decoy, b"{}").unwrap();
        fs::remove_file(paths.settings_json()).unwrap();
        std::os::unix::fs::symlink(&decoy, paths.settings_json()).unwrap();

        let exe2 = tempdir().unwrap();
        fs::write(exe2.path().join("sumcp"), b"#!new sumcp\n").unwrap();
        fs::write(exe2.path().join("sumcp-mcp"), b"#!new mcp\n").unwrap();
        assert!(
            run_install(&paths, exe2.path(), true).is_err(),
            "symlinked settings.json should abort the reinstall"
        );

        assert_eq!(
            fs::read(paths.installed_sumcp()).unwrap(),
            b"#!fake sumcp\n",
            "failed reinstall did not restore the previous sumcp binary"
        );
        assert_eq!(
            fs::read(paths.installed_mcp()).unwrap(),
            b"#!fake mcp\n",
            "failed reinstall did not restore the previous mcp binary"
        );
        assert!(paths.manifest().exists(), "previous manifest lost");
        let cj = read_json(&paths.claude_json());
        assert!(
            cj["mcpServers"]["sumcp"].is_object(),
            "failed reinstall removed the previous server registration"
        );
    }

    #[test]
    fn manifest_write_failure_rolls_back() {
        // If the final manifest write fails, the install must roll itself back
        // rather than leave binaries and config changes with no uninstall ledger.
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        // Occupy the manifest path with a non-empty directory: the final
        // atomic rename onto it must fail.
        fs::create_dir_all(paths.manifest().join("blocker")).unwrap();

        assert!(
            run_install(&paths, &exe, true).is_err(),
            "install should fail when the manifest cannot be written"
        );
        assert!(
            !paths.installed_sumcp().exists(),
            "binary left with no ledger"
        );
        assert!(!paths.skill_dest().exists(), "skill left with no ledger");
        assert!(
            !paths.claude_json().exists(),
            "server registration left with no ledger"
        );
        assert!(
            !paths.settings_json().exists(),
            "hook registration left with no ledger"
        );
    }

    #[test]
    fn install_refuses_scalar_hooks_value() {
        // A `hooks` value that is not an object is someone else's config
        // experiment; refuse to touch it rather than silently replace it.
        let home = temp_home();
        let settings = home.path().join(".claude/settings.json");
        let original = serde_json::to_vec_pretty(&serde_json::json!({"hooks": "custom"})).unwrap();
        fs::write(&settings, &original).unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "non-object hooks value must be refused"
        );
        assert_eq!(
            fs::read(&settings).unwrap(),
            original,
            "install modified a config it refused"
        );
        assert!(!paths.sumcp().exists(), "failed install left artifacts");
    }

    #[test]
    fn install_refuses_non_array_stop_value() {
        let home = temp_home();
        let settings = home.path().join(".claude/settings.json");
        let original =
            serde_json::to_vec_pretty(&serde_json::json!({"hooks": {"Stop": {"weird": true}}}))
                .unwrap();
        fs::write(&settings, &original).unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "non-array hooks.Stop must be refused"
        );
        assert_eq!(fs::read(&settings).unwrap(), original, "config modified");
        assert!(!paths.sumcp().exists(), "failed install left artifacts");
    }

    #[test]
    fn install_refuses_scalar_mcp_servers() {
        let home = temp_home();
        let cjp = home.path().join(".claude.json");
        let original =
            serde_json::to_vec_pretty(&serde_json::json!({"mcpServers": "nope"})).unwrap();
        fs::write(&cjp, &original).unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        assert!(
            run_install(&paths, &exe, true).is_err(),
            "non-object mcpServers must be refused, not panicked over"
        );
        assert_eq!(fs::read(&cjp).unwrap(), original, "config modified");
        assert!(!paths.sumcp().exists(), "failed install left artifacts");
    }

    #[test]
    fn reinstall_preserves_user_replaced_file() {
        // First install CREATED the skill file; the user then replaces it with
        // their own version. Reinstall must notice the drift and treat the
        // user's version like any pre-existing user file: back it up and
        // restore it on uninstall, never delete it.
        let home = temp_home();
        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let skill_md = paths.skill_dest().join("SKILL.md");
        fs::write(&skill_md, b"USER CONTENT").unwrap();

        run_install(&paths, &exe, true).unwrap(); // reinstall
        run_uninstall(&paths, true).unwrap();

        assert_eq!(
            fs::read(&skill_md).unwrap(),
            b"USER CONTENT",
            "user's replacement file was deleted instead of restored"
        );
    }

    /// Shared setup for the failed-uninstall tests: a user skill that install
    /// backs up, whose backup we then delete so the restore must fail.
    fn install_then_break_restore() -> (TempDir, TempDir, Paths, PathBuf, PathBuf) {
        let home = temp_home();
        let skill = home.path().join(".claude/skills/debrief");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), b"MY CUSTOM SKILL").unwrap();

        let (guard, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();

        let backup = fs::read_dir(&skill)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name().to_string_lossy().contains(".sumcp-bak."))
            .expect("expected a backup of the user's skill")
            .path();
        fs::remove_file(&backup).unwrap();
        (home, guard, paths, skill, backup)
    }

    #[test]
    fn failed_uninstall_keeps_manifest_and_reports_error() {
        // A restore that cannot happen (its backup vanished) must surface as
        // an error and leave the manifest ledger in place for a retry.
        let (_home, _g, paths, _skill, _backup) = install_then_break_restore();

        let res = run_uninstall(&paths, true);
        assert!(
            res.is_err(),
            "uninstall reported success despite a failed restore"
        );
        assert!(
            paths.manifest().exists(),
            "uninstall deleted its ledger despite failing"
        );
    }

    #[test]
    fn failed_uninstall_is_retryable() {
        // Same sabotage, then repair: putting the backup file back and
        // re-running uninstall must finish the job cleanly.
        let (_home, _g, paths, skill, backup) = install_then_break_restore();
        assert!(run_uninstall(&paths, true).is_err());

        fs::write(&backup, b"MY CUSTOM SKILL").unwrap();
        run_uninstall(&paths, true).unwrap();

        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "MY CUSTOM SKILL",
            "retry did not restore the user's skill"
        );
        assert!(
            !paths.manifest().exists(),
            "manifest left after clean retry"
        );
        assert!(!paths.sumcp().exists(), "sumcp tree left after clean retry");
    }

    #[test]
    fn reinstall_then_uninstall_restores_original_user_skill() {
        // The backup made on FIRST install (the user's own skill) must survive
        // a reinstall and still be restored by the eventual uninstall.
        let home = temp_home();
        let skill = home.path().join(".claude/skills/debrief");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), b"MY CUSTOM SKILL").unwrap();

        let (_g, exe) = fake_exe_dir();
        let paths = Paths::new(home.path().to_path_buf());
        run_install(&paths, &exe, true).unwrap();
        run_install(&paths, &exe, true).unwrap(); // reinstall

        run_uninstall(&paths, true).unwrap();
        assert_eq!(
            fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            "MY CUSTOM SKILL",
            "user's original skill lost across reinstall + uninstall"
        );
    }
}
