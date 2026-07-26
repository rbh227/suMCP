# Ceiling Verdict and Simple Ranking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the weighted score with a stated ordering rule (edited files
first, code before docs and config, then edit count, ties by path), keeping
every detector and all evidence untouched, and surface a secrets-file touch as
a blind spot.

**Architecture:** `score::rank` stops computing a weighted sum and starts
sorting on four declared keys. A new pure `file_class` module supplies the class
key. `Weights` and its TOML config are deleted outright, because nothing except
ranking ever read them. The payload contract goes from `v: 0` to `v: 1` because
`score` and `weights` are removed from two payloads.

**Tech Stack:** Rust 2024 edition, `serde`/`serde_json` only in
`sumcp-core`; python3 stdlib for dev scripts.

**Source spec:** `docs/superpowers/specs/2026-07-26-ceiling-verdict-and-simple-ranking-design.md`

## Global Constraints

- MSRV is `1.88` (`rust-version` in `Cargo.toml`), enforced by a CI job. Do not
  use language features newer than 1.88.
- `sumcp-core` stays synchronous and pure with no dependencies beyond `serde`
  and `serde_json` (ADR A2). Add no crates.
- Dev scripts are **python3 stdlib only**, matching `sanitize.py`,
  `check_payloads.py`, and `validity_sweep.py`. Do not import numpy.
- **No em dashes** in any prose, doc, comment, or commit message. The repo was
  scrubbed of 24 of them in T5.4 and the gate is enforced by review.
- No real filesystem paths, project names, or prompt text in anything committed
  under `docs/` or `fixtures/`. Projects are anonymized `proj-01..proj-NN`.
- All output must be deterministic: sort every collection before use, seed every
  RNG. Two runs on unchanged input produce byte-identical output.
- CI runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, and `python3
  scripts/check_payloads.py` on every push. All four must pass before any commit
  is pushed.
- Public items in `sumcp-core` need rustdoc: the crate has
  `#![warn(missing_docs)]` at `crates/sumcp-core/src/lib.rs:1`.
---

## File Structure

**Created:**
- `crates/sumcp-core/src/file_class.rs`: path to `FileClass` classification and
  ranking tier. Pure, no I/O, one responsibility.
- `docs/validation/2026-07-26-file-class-measurement.md`: the descriptive
  breakdown that justifies ranking code above documentation.

**Modified:**
- `crates/sumcp-core/examples/validity_dump.rs`: follow the `rank` signature
  change. No new fields.
- `crates/sumcp-core/src/lib.rs`: register `file_class`.
- `crates/sumcp-core/src/score.rs`: new ordering, `FileScore` shape, delete
  `Weights`.
- `crates/sumcp-core/src/payloads.rs`: `v: 1`, `class`/`edits` instead of
  `score`, `ranking_rule` instead of `weights`.
- `crates/sumcp-core/src/html.rs`: render class and edits, new footnote.
- `crates/sumcp-cli/src/main.rs`: call-site updates and the terminal line.
- `crates/sumcp-mcp/src/main.rs`: delete the weights loader, warn on a stale
  config.
- `crates/sumcp-mcp/src/server.rs`: delete the `weights` field.
- `crates/sumcp-mcp/src/identify.rs`, `crates/sumcp-mcp/tests/stdio.rs`: `v`
  assertions.
- `fixtures/mock-payloads/*.json` (7 files): `v: 1`, and the two ranking mocks
  reshaped.
- `scripts/check_payloads.py`: v1 rules.
- `docs/payload-schema.md`, `docs/metrics.md`, `SPEC.md`, `README.md`,
  `tasks/todo.md`.

**Not modified, deliberately:** every file under
`crates/sumcp-core/src/signals/`, `ingest.rs`, `merge.rs`, `model.rs`,
`assemble.rs`, `locate.rs`, `redact.rs`, `report.rs`, and the installer. The
detectors and the evidence chain do not change.

---

### Task 1: `file_class`

**Files:**
- Create: `crates/sumcp-core/src/file_class.rs`
- Modify: `crates/sumcp-core/src/lib.rs:9-20`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum FileClass { Code, Web, Notes, Docs, Config, Other }`
  deriving `Debug, Clone, Copy, PartialEq, Eq, Serialize` with
  `#[serde(rename_all = "snake_case")]`; `pub fn classify(path: &str) ->
  FileClass`; `pub fn tier(self) -> u8` on `FileClass`. Task 4 calls
  `classify` and `tier`.

- [ ] **Step 1: Write the failing tests**

Create `crates/sumcp-core/src/file_class.rs` containing only the test module
for now, so the first run fails to compile for the right reason:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_their_class() {
        assert_eq!(classify("/repo/src/main.rs"), FileClass::Code);
        assert_eq!(classify("/repo/app/page.tsx"), FileClass::Code);
        assert_eq!(classify("/repo/docs/guide.md"), FileClass::Docs);
        assert_eq!(classify("/repo/Cargo.toml"), FileClass::Config);
        assert_eq!(classify("/repo/site/styles.css"), FileClass::Web);
    }

    #[test]
    fn classification_is_case_insensitive() {
        // A transcript reports whatever the user typed, and READMEs are
        // routinely uppercase.
        assert_eq!(classify("/repo/README.MD"), FileClass::Docs);
        assert_eq!(classify("/repo/src/Main.RS"), FileClass::Code);
    }

    #[test]
    fn notes_beats_extension() {
        // A markdown file under a memory directory is a notes file, not
        // documentation: the two behave differently and the path says which.
        assert_eq!(classify("/home/u/.claude/memory/plan.md"), FileClass::Notes);
        assert_eq!(classify("/repo/notes/scratch.md"), FileClass::Notes);
        assert_eq!(classify("/repo/memory.md"), FileClass::Notes);
    }

    #[test]
    fn dotenv_is_config_including_suffixed_variants() {
        assert_eq!(classify("/repo/.env"), FileClass::Config);
        assert_eq!(classify("/repo/.env.local"), FileClass::Config);
        assert_eq!(classify("/repo/.env.production"), FileClass::Config);
    }

    #[test]
    fn extensionless_and_unknown_are_other() {
        assert_eq!(classify("/repo/Makefile"), FileClass::Other);
        assert_eq!(classify("/repo/LICENSE"), FileClass::Other);
        assert_eq!(classify("/repo/assets/hero.jpg"), FileClass::Other);
        assert_eq!(classify("/repo/.gitignore"), FileClass::Other);
    }

    #[test]
    fn bare_filename_without_a_directory_still_classifies() {
        assert_eq!(classify("main.rs"), FileClass::Code);
        assert_eq!(classify("notes.md"), FileClass::Docs);
    }

    #[test]
    fn tiers_order_code_above_notes_above_docs_above_config() {
        assert!(FileClass::Code.tier() < FileClass::Notes.tier());
        assert!(FileClass::Notes.tier() < FileClass::Docs.tier());
        assert!(FileClass::Docs.tier() < FileClass::Config.tier());
        // Web ranks with code; Other ranks with config.
        assert_eq!(FileClass::Web.tier(), FileClass::Code.tier());
        assert_eq!(FileClass::Other.tier(), FileClass::Config.tier());
    }

    #[test]
    fn serializes_as_snake_case() {
        let j = serde_json::to_value(FileClass::Code).unwrap();
        assert_eq!(j, serde_json::json!("code"));
        let j = serde_json::to_value(FileClass::Other).unwrap();
        assert_eq!(j, serde_json::json!("other"));
    }
}
```

Register the module by adding `pub mod file_class;` to
`crates/sumcp-core/src/lib.rs`, in alphabetical position between
`pub mod assemble;` and `pub mod html;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p sumcp-core file_class`
Expected: FAIL to compile, with errors like
`cannot find type FileClass in this scope` and
`cannot find function classify in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `crates/sumcp-core/src/file_class.rs`:

```rust
//! File classification for ranking (spec 2026-07-26 §2a).
//!
//! Pure: classification reads the path string only, never the filesystem
//! (ADR A9). Every input is an untrusted path out of a transcript.
//!
//! **Why classes exist.** On the 2026-07-26 tune split of the author's own
//! corpus, documentation files were 192 of 552 (session, file) pairs and
//! carried 1 of 39 recurrence outcomes; config files were 37 pairs and
//! carried none. Code files were 285 pairs and carried 34. Ranking code above
//! documentation cut flagged files from 65 to 52 with an identical hit count.
//!
//! **What the tiers are and are not.** Only the code-versus-docs-and-config
//! boundary rests on adequate data. `Notes` (19 pairs, 3 outcomes) showed a
//! HIGHER outcome rate than code, 0.158 against 0.119, which on three
//! outcomes is far too thin to promote it above code, so it sits directly
//! below code rather than beside documentation. `Web` (7 pairs) is grouped
//! with code because web files are code-like, not because 7 pairs measured
//! anything. Read the tier order as a declared judgment on thin data
//! everywhere except that one boundary.

use serde::Serialize;

/// What kind of file a path names, for ranking purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    /// Source code.
    Code,
    /// Markup and stylesheets. Ranks with [`FileClass::Code`].
    Web,
    /// Running notes and agent memory files.
    Notes,
    /// Prose documentation.
    Docs,
    /// Configuration, lockfiles, and environment files.
    Config,
    /// Anything unrecognized, including extensionless files and binaries.
    Other,
}

impl FileClass {
    /// Ranking tier, lower sorts first. Not the enum's declaration order:
    /// tiers are deliberately coarse so that two classes can tie.
    pub fn tier(self) -> u8 {
        match self {
            FileClass::Code | FileClass::Web => 0,
            FileClass::Notes => 1,
            FileClass::Docs => 2,
            FileClass::Config | FileClass::Other => 3,
        }
    }
}

const CODE_EXT: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "h", "cc", "cpp",
    "hpp", "rb", "swift", "kt", "sh", "bash", "zsh", "sql", "vue", "svelte",
    "cs", "php", "lua", "scala", "dart", "ex", "exs", "clj", "hs", "ml", "m",
    "mm", "r", "pl",
];
const WEB_EXT: &[&str] = &["html", "css", "scss", "sass", "less"];
const DOCS_EXT: &[&str] = &["md", "mdx", "txt", "rst", "adoc", "tex"];
const CONFIG_EXT: &[&str] = &[
    "json", "toml", "yaml", "yml", "ini", "cfg", "conf", "env", "lock",
    "properties", "gradle", "xml", "plist",
];

/// Classify a path. Precedence matters and is tested:
///
/// 1. A basename starting with `.env` is [`FileClass::Config`], so
///    `.env.local` and `.env.production` are caught alongside `.env`.
/// 2. A memory or notes path is [`FileClass::Notes`], checked BEFORE
///    extensions so `memory/plan.md` is notes rather than documentation.
/// 3. Extension tables, in the order code, docs, config, web.
/// 4. Everything else is [`FileClass::Other`].
pub fn classify(path: &str) -> FileClass {
    let lower = path.to_ascii_lowercase();
    // `rsplit('/')` always yields at least one item, so a bare filename with
    // no directory component still lands here.
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if name.starts_with(".env") {
        return FileClass::Config;
    }
    if lower.contains("/memory/") || name.starts_with("memory.") || lower.contains("/notes") {
        return FileClass::Notes;
    }

    // `rsplit_once` on the BASENAME, so a dot in a parent directory cannot be
    // mistaken for an extension. A leading-dot file like `.gitignore` yields
    // ("", "gitignore"), which matches no table and falls through to Other.
    let Some((_stem, ext)) = name.rsplit_once('.') else {
        return FileClass::Other;
    };
    if CODE_EXT.contains(&ext) {
        FileClass::Code
    } else if DOCS_EXT.contains(&ext) {
        FileClass::Docs
    } else if CONFIG_EXT.contains(&ext) {
        FileClass::Config
    } else if WEB_EXT.contains(&ext) {
        FileClass::Web
    } else {
        FileClass::Other
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p sumcp-core file_class`
Expected: PASS, 8 tests.

- [ ] **Step 5: Verify the whole workspace and the lints**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all three clean. `file_class` is not yet used by anything, which is
intentional: it lands reviewable on its own.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/file_class.rs crates/sumcp-core/src/lib.rs
git commit -m "core: file_class, path to Code/Web/Notes/Docs/Config/Other

Pure path classification, no filesystem access (ADR A9). Used by nothing yet.

Motivated by the 2026-07-26 tune split: documentation was 192 of 552
(session, file) pairs and carried 1 of 39 recurrence outcomes, config 37 pairs
and none, code 285 pairs and 34. The rustdoc records which tier boundaries the
data actually supports (code versus docs and config) and which are declared
judgments on thin cells (notes at 19 pairs, web at 7)."
```

---

### Task 2: The new ordering

**Files:**
- Modify: `crates/sumcp-core/src/score.rs:78-89` (`FileScore`), `:143-186`
  (`rank`)
- Test: `crates/sumcp-core/src/score.rs` test module

**Interfaces:**
- Consumes: `file_class::{FileClass, classify}` from Task 1.
- Produces: `FileScore` gains `pub class: FileClass` and `pub edits: u64` and
  keeps `score: f64` for now, so every existing caller still compiles. `rank`
  keeps its `&Weights` parameter for now. Task 5 removes both.

This task changes ordering only. Keeping `score` and `Weights` for one more
task is what makes it independently compilable and testable.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/sumcp-core/src/score.rs`:

```rust
    /// Read-only files carry ReRead findings and so enter the ranking with
    /// zero edits. On the demo fixture a never-edited `.jpg` ranked FOURTH,
    /// above a `.py` file whose commands were failing, purely for having been
    /// read four times. A review queue is about changes, so anything unedited
    /// sorts last regardless of class.
    #[test]
    fn edited_files_outrank_unedited_ones() {
        let read = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
            )
        };
        let mut lines: Vec<String> = (0..4)
            .map(|i| read(&format!("r{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hero.jpg"))
            .collect();
        // One edited code file, fewer signals than the read-thrashed image.
        for i in 0..2 {
            lines.push(edit(
                &format!("e{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].file, "/a/main.rs", "edited file first");
        assert_eq!(ranked[0].edits, 2);
        assert_eq!(ranked.last().unwrap().file, "/a/hero.jpg");
        assert_eq!(ranked.last().unwrap().edits, 0, "never edited");
    }

    #[test]
    fn code_outranks_docs_even_with_fewer_edits() {
        let mut lines = Vec::new();
        // Docs edited 5x, code edited 2x. Code still wins on class.
        for i in 0..5 {
            lines.push(edit(
                &format!("d{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/a/NOTES-FOR-RELEASE.md",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("c{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].file, "/a/main.rs");
        assert_eq!(ranked[0].class, crate::file_class::FileClass::Code);
        assert_eq!(ranked[1].file, "/a/NOTES-FOR-RELEASE.md");
        assert_eq!(ranked[1].class, crate::file_class::FileClass::Docs);
    }

    #[test]
    fn within_a_class_more_edits_ranks_first_and_path_breaks_ties() {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(&format!("h{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hot.rs"));
        }
        for i in 0..2 {
            lines.push(edit(&format!("m{i}"), &format!("2026-01-01T00:01:0{i}Z"), "/a/mid.rs"));
        }
        // Same edit count as mid.rs, so only the path can separate them.
        for i in 0..2 {
            lines.push(edit(&format!("z{i}"), &format!("2026-01-01T00:02:0{i}Z"), "/a/also.rs"));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let files: Vec<&str> = rank(&s, &Weights::default())
            .iter()
            .map(|f| f.file.as_str())
            .collect();
        assert_eq!(files, vec!["/a/hot.rs", "/a/also.rs", "/a/mid.rs"]);
    }

    #[test]
    fn edits_counts_writes_as_well_as_edits() {
        let write = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"{file}","content":"x"}}}}]}}}}"#
            )
        };
        let raw = format!(
            "{}\n{}\n{}",
            write("w1", "2026-01-01T00:00:01Z", "/a/main.rs"),
            edit("e1", "2026-01-01T00:00:02Z", "/a/main.rs"),
            edit("e2", "2026-01-01T00:00:03Z", "/a/main.rs"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].edits, 3, "Write counts toward edits");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p sumcp-core score::tests`
Expected: FAIL to compile with `no field edits on type FileScore` and
`no field class on type FileScore`.

- [ ] **Step 3: Add the two fields to `FileScore`**

Replace `crates/sumcp-core/src/score.rs:78-89` with:

```rust
/// One file's place in the ranking, with the evidence that explains it.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// The file path.
    pub file: String,
    /// What kind of file this is. First ranking key after edited-ness.
    pub class: crate::file_class::FileClass,
    /// How many Edit or Write actions targeted this file. Second ranking key.
    pub edits: u64,
    /// The weighted score. Retained for one task only; the ranking no longer
    /// consults it (spec 2026-07-26 §2b removes it next).
    pub score: f64,
    /// Per-category magnitudes (churn/rework/failure_loops/re_read/fumbles/action_loops).
    pub breakdown: BTreeMap<String, u64>,
    /// The findings backing this file, in a stable order.
    pub findings: Vec<Finding>,
}
```

- [ ] **Step 4: Count edits and change the sort**

Add above `rank` in `crates/sumcp-core/src/score.rs`:

```rust
/// Edit/Write actions per file. Not a signal: the ranking's second key and a
/// displayed number, so it counts ATTEMPTS exactly as `Overview::edits` does
/// rather than only confirmed successes.
fn edit_counts(s: &Session) -> BTreeMap<&str, u64> {
    let mut out: BTreeMap<&str, u64> = BTreeMap::new();
    for a in &s.actions {
        if matches!(a.kind, ActionKind::Edit | ActionKind::Write)
            && let Some(f) = a.file_path.as_deref()
        {
            *out.entry(f).or_insert(0) += 1;
        }
    }
    out
}
```

Add `ActionKind` to the `use crate::model::{...}` list at
`crates/sumcp-core/src/score.rs:18`.

Then in `rank`, immediately after the `let mut acc: BTreeMap<String, Acc> =
BTreeMap::new();` line, add:

```rust
    let edits = edit_counts(s);
```

Replace the `.map(...)` closure that builds each `FileScore` with:

```rust
        .map(|(file, (score, breakdown, findings))| {
            let edits = edits.get(file.as_str()).copied().unwrap_or(0);
            FileScore {
                class: crate::file_class::classify(&file),
                edits,
                file,
                score,
                breakdown,
                findings,
            }
        })
```

Replace the `scores.sort_by(...)` call and its comment with:

```rust
    // The ranking rule, in full. Four keys, each one checkable by hand
    // against the rendered report (spec 2026-07-26 §2b):
    //   1. edited files before never-edited ones, because a file with no
    //      change has nothing to review;
    //   2. class tier, because documentation and config churn does not
    //      predict recurrence (see file_class's module doc);
    //   3. edit count, descending;
    //   4. path, so the order is total and stable.
    // Deliberately NOT a weighted sum: fitting weights to maximize hits with
    // the outcomes in hand bought at most 4 hits out of 39 on the only corpus
    // this has been measured against, and the fit put maximum weight on edit
    // count anyway (docs/validation/2026-07-26-ceiling-analysis.md).
    scores.sort_by(|a, b| {
        (a.edits == 0)
            .cmp(&(b.edits == 0))
            .then_with(|| a.class.tier().cmp(&b.class.tier()))
            .then_with(|| b.edits.cmp(&a.edits))
            .then_with(|| a.file.cmp(&b.file))
    });
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test -p sumcp-core score::tests`
Expected: PASS. The four new tests pass, and the pre-existing tests
`ranking_is_transparent_and_ordered`,
`tiny_relative_churn_halves_the_churn_contribution`, and
`action_loop_contributes_at_half_weight` still pass, because `score` is still
computed and their fixtures are all same-class files ordered by edit count.

- [ ] **Step 6: Run the whole workspace**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all clean. If an `html.rs` or `payloads.rs` test fails on ordering,
read the failure: it is telling you a fixture's expected order changed, which
is the intended behavior, and the assertion should be updated to the new order.

- [ ] **Step 7: Eyeball the fixture, which is the whole point**

```bash
cargo build --release -p sumcp-cli
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl 2>&1 \
  | sed -n '/struggle areas/,/^$/p'
```

Expected: the `.py` files now rank above the `.md` files, and the `.jpg` is
last. Before this change the order was `.py`, `.md`, `.md`, `.jpg`, `.py`.

- [ ] **Step 8: Commit**

```bash
git add crates/sumcp-core/src/score.rs
git commit -m "core: rank by edited-first, class, edit count, path

Replaces the weighted sum as the ORDERING key. The score field and Weights
survive one more commit so every caller still compiles; the next commit
removes both.

On the demo fixture the old order put two markdown files and a never-edited
JPEG above a .py file whose commands were failing. The JPEG ranked on
re-reads alone. Four declared keys fix that and can be checked by hand
against the report."
```

---

### Task 3: Remove the score and the weights, land payload contract v1

**Files:**
- Modify: `crates/sumcp-core/src/score.rs` (delete `Weights`,
  `REL_CHURN_CLAMP`, `category_weight`, `finding_multiplier`; drop the `score`
  field and the `&Weights` parameter)
- Modify: `crates/sumcp-core/src/payloads.rs:236`, `:326-390`, six `"v": 0`
  sites at `:273`, `:384`, `:459`, `:509`, `:550`, `:620`, test at `:784`, and
  the `struggle_areas` call sites at `:773`, `:833`, `:940`, `:983`, `:1076`
- Modify: `crates/sumcp-core/src/html.rs:46-50`, `:69`, `:511-578`, `:825`,
  `:974-975`, test at `:1226`
- Modify: `crates/sumcp-cli/src/main.rs:14`, `:220`, `:229`, `:248-261`
- Modify: `crates/sumcp-mcp/src/main.rs:13-52`, `:99`, tests at `:113-129`
- Modify: `crates/sumcp-mcp/src/server.rs:19`, `:44-46`, `:300`, `:306`, `:346`
- Modify: `crates/sumcp-mcp/src/identify.rs:296`,
  `crates/sumcp-mcp/tests/stdio.rs:218`, `:282`
- Modify: `crates/sumcp-core/examples/validity_dump.rs:20`, `:41`
- Modify: all 7 files in `fixtures/mock-payloads/*.json`
- Modify: `scripts/check_payloads.py:29-30`, `:85`, `:94-98`
- Modify: `docs/payload-schema.md:54-55`, and append a v1 section

**Interfaces:**
- Consumes: `FileScore` with `class` and `edits` from Task 4.
- Produces: `pub const RANKING_RULE: &str` in `score.rs`; `rank(s: &Session) ->
  Vec<FileScore>`; `render_html(s: &Session, ranked: &[FileScore], meta:
  &SessionMeta) -> String`; `struggle_areas(ranked: &[FileScore], meta:
  &SessionMeta, n: usize) -> Value`. `Weights` no longer exists.

This task is deliberately atomic. CI runs `check_payloads.py`, so the Rust
builders, the mocks, the checker, and the schema doc must move together or the
build is red.

- [ ] **Step 1: Write the failing payload tests**

In `crates/sumcp-core/src/payloads.rs`, replace the test
`struggle_areas_echoes_weights_and_breakdown` (at line 830) with:

```rust
    #[test]
    fn struggle_areas_echoes_the_ranking_rule_and_breakdown() {
        let s = churny_session();
        let p = struggle_areas(&rank(&s), &meta(), 5);
        assert_eq!(p["v"], 1);
        // SPEC §7: ranking output is never an opaque number. The rule that
        // produced the order ships with the order.
        assert_eq!(p["ranking_rule"], crate::score::RANKING_RULE);
        assert!(p["files"][0]["breakdown"].is_object());
        assert!(p["files"][0]["class"].is_string());
        assert!(p["files"][0]["edits"].is_u64());
        assert!(
            p["files"][0].get("score").is_none(),
            "the weighted score is gone, not renamed"
        );
        assert!(p.get("weights").is_none(), "weights are gone");
    }

    #[test]
    fn session_overview_top_struggles_carry_class_and_edits() {
        let s = churny_session();
        let ranked = rank(&s);
        let p = session_overview(&s, &ranked, &meta());
        assert_eq!(p["v"], 1);
        let top = &p["top_struggles"][0];
        assert!(top["class"].is_string());
        assert!(top["edits"].is_u64());
        assert!(top.get("score").is_none());
    }
```

If a helper named `churny_session()` does not already exist in that test
module, add it, reusing the module's existing `edit` helper:

```rust
    /// Two same-class files with different edit counts: enough to rank.
    fn churny_session() -> crate::model::Session {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(&format!("h{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hot.rs"));
        }
        for i in 0..2 {
            lines.push(edit(&format!("w{i}"), &format!("2026-01-01T00:01:0{i}Z"), "/a/warm.rs"));
        }
        crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main)
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p sumcp-core payloads::tests::struggle_areas_echoes_the_ranking_rule_and_breakdown`
Expected: FAIL to compile, because `struggle_areas` still takes four arguments
and `RANKING_RULE` does not exist.

- [ ] **Step 3: Strip `score.rs`**

In `crates/sumcp-core/src/score.rs`:

1. Delete the `Weights` struct, its `impl Default`, `REL_CHURN_CLAMP`,
   `category_weight`, and `finding_multiplier` entirely.
2. Delete `use serde::{Deserialize, Serialize};` and replace with
   `use serde::Serialize;` (`FileScore` still derives `Serialize`).
3. Remove `Confidence` from the `use crate::model::{...}` list if nothing else
   in the file uses it after the weighted sum is gone.
4. Replace the module doc at lines 1-16 with:

```rust
//! Ranking: a stated rule, not a score (spec 2026-07-26 §2b).
//!
//! Files are ordered by four keys, in this order:
//!
//! 1. edited files before never-edited ones, because a file with no change
//!    has nothing to review;
//! 2. [`crate::file_class::FileClass::tier`], because documentation and
//!    config churn does not predict recurrence;
//! 3. edit count, descending;
//! 4. path, so the order is total and stable.
//!
//! **Why there is no weighted score.** Until 2026-07-26 this module summed
//! `weight[category] * magnitude * confidence_factor` per file. Fitting those
//! weights to maximize hits WITH THE OUTCOMES IN HAND bought at most 4 hits
//! out of 39 on the only corpus it has been measured against, and the fit
//! assigned maximum weight to edit count regardless. A tuned sum that cannot
//! beat counting edits even while cheating is not worth the opacity it costs,
//! so the sum, the `Weights` type, and its TOML override (ADR A6) are gone.
//! See `docs/validation/2026-07-26-ceiling-analysis.md`.
//!
//! The findings stay. They are the explanation and the citations, and every
//! one still carries its tier, its exact-versus-heuristic flag, its
//! confidence, and the action indices that prove it.

/// The ranking rule, as one sentence. A constant so the payload, the HTML
/// report, and the terminal output cannot drift apart.
pub const RANKING_RULE: &str =
    "edited files first, then code before docs and config, then by edit count, ties by path";
```

5. Change the signature to `pub fn rank(s: &Session) -> Vec<FileScore>` and
   delete the `factor`, `contribution`, and `entry.0 += contribution` lines,
   the `Acc` tuple's `f64` slot, and the `score` field from the `FileScore`
   construction. `Acc` becomes
   `type Acc = (BTreeMap<String, u64>, Vec<Finding>);`.
6. Delete the `score: f64` field from `FileScore` and its doc comment.
7. Update every `rank(&s, &Weights::default())` and `rank(&s, &w)` inside this
   file's own tests to `rank(&s)`, and delete the tests
   `default_weight_order_matches_evidence_strength`,
   `tiny_relative_churn_halves_the_churn_contribution`, and
   `action_loop_contributes_at_half_weight`, which assert weighted-sum
   behavior that no longer exists. Change
   `ranking_is_transparent_and_ordered`'s final assertion from
   `assert!(ranked[0].score > ranked[1].score);` to
   `assert!(ranked[0].edits > ranked[1].edits);`.

- [ ] **Step 4: Update `payloads.rs`**

1. Line 10: `use crate::score::{FileScore, Weights};` becomes
   `use crate::score::FileScore;`.
2. Line 236, in `session_overview`'s `top` builder:

```rust
            json!({
                "file": elide_middle(&f.file, PATH_MAX),
                "class": f.class, "edits": f.edits,
                "breakdown": f.breakdown
            })
```

3. Replace the `struggle_areas` doc line 326 and signature at 335-353 with:

```rust
/// `struggle_areas(n)`: ranked files with breakdown, ranking rule, findings.
///
/// Three caps stack, in the order the schema advertises (tail-first):
/// `n` is clamped to `STRUGGLE_FILES_MAX`, findings per file are capped and
/// chosen to represent the breakdown (`representative_findings`), and then
/// the payload is rebuilt smaller until it fits `CAP_STRUGGLE`: lowest-ranked
/// files dropped first, and only once a single file is left do its findings
/// start going. Measured before this existed: `n=99` on an ordinary 12-file
/// session produced 2827 tokens, and a 200-file session 1.6M.
pub fn struggle_areas(ranked: &[FileScore], meta: &SessionMeta, n: usize) -> Value {
    // `n` arrives straight from an MCP caller and was honored verbatim.
    let n = n.min(STRUGGLE_FILES_MAX);
    let (session, id_cut) = session_block(meta);
```

The `weights_json` block and its `long_source` elision go away entirely:
`RANKING_RULE` is a compile-time constant, so there is no longer a
caller-controlled string in this payload to truncate.

4. Line 364, in the per-file entry:

```rust
                let mut entry = json!({
                    "rank": i + 1, "file": elide_middle(&f.file, PATH_MAX),
                    "class": f.class, "edits": f.edits,
                    "breakdown": f.breakdown,
                    "findings": kept.iter().map(|f| compact_finding(f)).collect::<Vec<_>>()
                });
```

5. Line 386: `"weights": weights_json,` becomes
   `"ranking_rule": crate::score::RANKING_RULE,`.
6. All six `"v": 0,` sites become `"v": 1,`.
7. Line 784: `assert_eq!(payload["v"], 0);` becomes `assert_eq!(payload["v"], 1);`.
8. Every `struggle_areas(&r, &w, &m, 10)`-shaped call in the test module drops
   the weights argument, and every `rank(&s, &w)` becomes `rank(&s)`. Delete
   any now-unused `let w = Weights::default();` bindings.

- [ ] **Step 5: Update `html.rs`**

1. Line 46-50: drop the `weights: &Weights` parameter, leaving
   `pub fn render_html(s: &Session, ranked: &[FileScore], meta: &SessionMeta) -> String`.
   Remove `Weights` from the file's `use` list.
2. Line 69: `h.push_str(&struggles_section(ranked, &review));`
3. Line 515: `struggles_section` drops its `weights: &Weights` parameter.
4. Lines 539-547, the row: replace the score cell with class and edits.

```rust
        let _ = write!(
            rows,
            "<tr{top}><td class=\"r\">{rank}</td><td>{file_cell}</td>\
             <td>{class}</td><td class=\"r\">{edits}</td><td>{phrases}</td></tr>",
            top = if i < 3 { " class=\"top\"" } else { "" },
            rank = i + 1,
            class = esc(&format!("{:?}", f.class).to_lowercase()),
            edits = f.edits,
            phrases = esc(&phrases.join(", ")),
        );
```

5. Lines 559-572, the footnote, becomes the rule itself:

```rust
    let footnote = format!(
        "<p class=\"foot\">ranked by: {rule}. No weighted score: on the only \
         corpus this has been measured against, no weighting over the \
         observable signals beat counting edits.</p>",
        rule = esc(crate::score::RANKING_RULE),
    );
```

6. Lines 573-578, the table header gains two columns:

```rust
    format!(
        "<section class=\"sec\"><h2>Struggle areas</h2>\
         <table class=\"tbl\"><thead><tr><th>#</th><th>file</th>\
         <th>class</th><th>edits</th><th>signals</th></tr></thead>\
         <tbody>{rows}</tbody></table>{overflow}{footnote}</section>"
    )
```

7. Line 824-827, the story `why_line`:

```rust
        let why_line = match c.ranked {
            Some(fs) => format!(
                "{} · edited {}x · {}",
                esc(&format!("{:?}", fs.class).to_lowercase()),
                fs.edits,
                esc(&why)
            ),
            None => esc(&why),
        };
```

8. Lines 974-975 in the test module: `rank(&s)` and
   `render_html(&s, &r, &meta())`.
9. Rename the test at line 1226 to
   `struggle_breakdown_is_plain_language_with_ranking_rule_footnote` and
   replace its `assert!(html.contains("rework 3"), "actual weights echoed");`
   with `assert!(html.contains(crate::score::RANKING_RULE), "rule echoed");`.

- [ ] **Step 6: Update the two binaries**

`crates/sumcp-cli/src/main.rs`:

- Line 14: `use sumcp_core::score::rank;`
- Line 220: `let ranked = rank(&session);`
- Line 229: `sumcp_core::html::render_html(&session, &ranked, &meta)`
- Lines 254-260, the terminal line:

```rust
            println!(
                "{}. {}  ({}, edited {}x: {})",
                i + 1,
                f.file,
                format!("{:?}", f.class).to_lowercase(),
                f.edits,
                cats.join(", ")
            );
```

`crates/sumcp-mcp/src/server.rs`:

- Line 19: `use sumcp_core::score::rank;`
- Lines 44-46: delete the `pub weights: Weights,` field and its doc comment,
  and change the struct doc at line 38 to
  `/// The server: project directory to scan and parsed-session cache.`
- Line 300: `let ranked = rank(&session);`
- Line 306: `payloads::struggle_areas(&ranked, &meta, n)`
- Line 346: delete `weights: Weights::default(),` from the test helper.

`crates/sumcp-mcp/src/main.rs`:

- Delete `load_weights_from` (lines 15-52), the `use sumcp_core::score::Weights;`
  at line 13, and the tests `missing_config_yields_defaults` and
  `partial_toml_overrides_and_records_source`.
- Keep `config_path`, `config_path_from`, and the
  `empty_or_relative_xdg_config_home_is_ignored` test: they now serve the
  notice below, and the XDG hardening they encode is still worth keeping.
- Add, replacing the deleted loader:

```rust
/// ADR A6 retired (spec 2026-07-26 §2f): ranking has no weights to configure,
/// so `~/.config/sumcp/config.toml` is no longer read. A user who wrote one is
/// told rather than silently ignored.
fn warn_if_stale_config(path: Option<PathBuf>) {
    if let Some(path) = path
        && path.exists()
    {
        eprintln!(
            "sumcp-mcp: {} is no longer read (ranking weights were removed; \
             see docs/validation/2026-07-26-ceiling-analysis.md)",
            path.display()
        );
    }
}
```

- Line 94-100, the server construction:

```rust
    warn_if_stale_config(config_path());
    let server = server::SumcpServer {
        // Claude Code launches project-scoped stdio servers with cwd = the
        // project root, so this resolves to the right transcript directory.
        project_dir: sumcp_core::locate::project_dir(&home, &cwd),
        store: store::SessionStore::new(),
    };
```

- If `toml` is now unused in `sumcp-mcp`, remove it from that crate's
  `Cargo.toml` dependencies and run `cargo update -p sumcp-mcp` so
  `Cargo.lock` stays in step. Verify with
  `grep -rn 'toml::' crates/sumcp-mcp/src` returning nothing first.

`crates/sumcp-core/examples/validity_dump.rs`:

- Line 20: `use sumcp_core::score::{all_findings, rank};`
- Line 41: `let ranked = rank(&session);`

- [ ] **Step 7: Update the remaining `v` assertions**

- `crates/sumcp-mcp/src/identify.rs:296`: `assert_eq!(p["v"], 1);`
- `crates/sumcp-mcp/tests/stdio.rs:218` and `:282`:
  `assert_eq!(overview["v"], 1);`

- [ ] **Step 8: Update the mock payloads**

In all 7 files under `fixtures/mock-payloads/`, change `"v":0` to `"v":1`.

In `fixtures/mock-payloads/struggle_areas.json`: delete the whole `"weights"`
line, add
`"ranking_rule":"edited files first, then code before docs and config, then by edit count, ties by path",`
in its place, and in each of the three file entries replace
`"score":63.5,` with `"class":"code","edits":24,`, `"score":38.0,` with
`"class":"code","edits":17,`, and `"score":22.5,` with
`"class":"code","edits":10,`. The edit counts match each entry's own
`breakdown.churn`, which is what churn magnitude counts, so the mock stays
internally consistent.

In `fixtures/mock-payloads/session_overview.json`, make the same
`score` to `class`/`edits` replacement in all three `top_struggles` entries.

- [ ] **Step 9: Update the payload checker**

In `scripts/check_payloads.py`:

- Line 29-30:

```python
# payloads whose top-level content is ranked: they must echo the RULE that
# produced the order, never an opaque score (SPEC §7)
RANKING_PAYLOADS = {"struggle_areas"}
```

- Line 85: `if payload.get("v") != 1:` and the message becomes
  `"missing/wrong schema version 'v' (expected 1)"`.
- Lines 94-98:

```python
    if name in RANKING_PAYLOADS:
        rule = payload.get("ranking_rule")
        if not (isinstance(rule, str) and rule.strip()):
            errors.append("ranking payload must echo a non-empty 'ranking_rule'"
                          " (SPEC §7, never opaque)")
        if "weights" in payload:
            errors.append("'weights' was removed in v1; ranking has no weights")
        files = payload.get("files", [])
        if not any("breakdown" in f for f in files):
            errors.append("ranking payload must show per-file 'breakdown'")
        for f in files:
            if "score" in f:
                errors.append("v1 removed the opaque per-file 'score'")
            for field in ("class", "edits"):
                if field not in f:
                    errors.append(f"ranked file missing '{field}'")
```

- In `check_error`, line 106: `if payload.get("v") != 1:`.
- Add, so the overview's ranked entries are held to the same rule:

```python
def check_overview_top_struggles(payload) -> list[str]:
    """session_overview embeds ranked entries too, and the same v1 rule
    applies to them: class and edits, never an opaque score."""
    errors = []
    for entry in payload.get("top_struggles", []):
        if "score" in entry:
            errors.append("v1 removed the opaque 'score' from top_struggles")
        for field in ("class", "edits", "breakdown"):
            if field not in entry:
                errors.append(f"top_struggles entry missing '{field}'")
    return errors
```

and call it from `check_success` when `name == "session_overview"`.

- [ ] **Step 10: Update the schema doc**

In `docs/payload-schema.md`, replace lines 54-55 with:

```markdown
All ranking output shows the per-category `breakdown` and the `ranking_rule`
that produced the order. There is no score: see the v1 section below.
```

Append at the end of the file:

```markdown
## 2026-07-26 BREAKING: `v` goes 0 to 1 (spec 2026-07-26)

The weighted score is gone, so two payloads change shape. Every payload's `v`
becomes `1`.

| payload | removed | added |
|---|---|---|
| `struggle_areas` | `weights` object, per-file `score` | `ranking_rule` string, per-file `class` and `edits` |
| `session_overview` | `top_struggles[].score` | `top_struggles[].class`, `top_struggles[].edits` |

`class` is one of `code`, `web`, `notes`, `docs`, `config`, `other`. `edits`
counts Edit and Write attempts against that file.

Why: fitting ranking weights to maximize hits with the outcomes in hand bought
at most 4 hits out of 39 on the only corpus this has been measured against, and
the fit assigned maximum weight to edit count anyway. The order is now four
declared keys a reader can check by hand, and `ranking_rule` ships alongside
the order so SPEC §7's "never an opaque number" holds more strongly than
before. Full method and tables in
`docs/validation/2026-07-26-ceiling-analysis.md`.
```

- [ ] **Step 11: Run everything**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_payloads.py
python3 scripts/check_narration.py
```

Expected: all five clean. `check_payloads.py` prints no errors. If
`check_narration.py` fails, the debrief mock references a removed field; update
`fixtures/mock-payloads/sample-debrief.md` so it cites `class` and `edits`
rather than a score.

- [ ] **Step 12: Confirm the real binaries still work end to end**

```bash
cargo build --release -p sumcp-cli -p sumcp-mcp
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl --json \
  | python3 -c "
import json,sys
d=json.load(sys.stdin)
assert d['v']==1, d['v']
t=d['top_struggles'][0]
assert 'score' not in t and 'class' in t and 'edits' in t, t
print('overview v1 OK:', json.dumps(t)[:120])
"
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl --html \
  | grep -c "edited files first, then code before docs" \
  && echo "html rule footnote OK"
```

Expected: `overview v1 OK` with a class and an edits count, and a non-zero
grep count for the footnote.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "core: remove the weighted score, payload contract v1

Deletes the weighted sum, the Weights type, and its TOML override (ADR A6
retired). Nothing but ranking ever read them: the detectors in signals/ never
consulted weights, so leaving a public configurable type would have advertised
a knob that changes nothing.

Payloads go v0 to v1: struggle_areas drops weights and per-file score for
ranking_rule plus class and edits, and session_overview's top_struggles does
the same. SPEC 7 holds more strongly than before, since the rule that produced
the order now ships with the order instead of six decimals that did not
explain it.

This also closes the CLI-versus-MCP divergence from the codex review: the CLI
always used Weights::default() while the server loaded the config, so the two
surfaces could rank the same session differently. There is now one rule and no
configuration to diverge on. A user with a stale config gets a notice."
```

---

### Task 4: Surface a secrets-file touch as a blind spot

**Files:**
- Modify: `crates/sumcp-core/src/file_class.rs` (add `is_secrets`)
- Modify: `crates/sumcp-core/src/model.rs` (`FindingKind` gains a variant)
- Create: `crates/sumcp-core/src/signals/secrets.rs`
- Modify: `crates/sumcp-core/src/signals.rs` (register the module)
- Modify: `crates/sumcp-core/src/score.rs` (`all_findings`, `ranked_category`)
- Modify: `crates/sumcp-core/src/review.rs` (`is_solo_qualifying`, `reason_sentence`)
- Modify: `crates/sumcp-core/src/payloads.rs` (`blind_spots`)
- Modify: `fixtures/mock-payloads/blind_spots.json`, `scripts/check_payloads.py`,
  `docs/payload-schema.md`, `docs/metrics.md`

**Why this task exists.** The user's rule is that a `.env` file should never be
edited or even read. The ranking change puts `Config` in the LAST tier, so a
secrets file the agent touched would be buried at the bottom of the queue,
which is backwards: if it must never be touched, a touch is the single most
important thing in the report. Ranking is the wrong instrument for a
zero-tolerance rule, so this surfaces it through `blind_spots` instead, where
one occurrence is enough. The class stays `Config` so ordinary config churn
keeps ranking low.

Secret VALUES are already handled: `redact.rs` scrubs excerpt text on the
`evidence()` path, so citing these actions cannot print a key.

**Interfaces:**
- Consumes: `file_class` from Task 1; the payload v1 shape from Task 5.
- Produces: `file_class::is_secrets(path: &str) -> bool`;
  `FindingKind::SecretsFileTouched` serializing as `secrets_file_touched`;
  `signals::secrets(s: &Session) -> Vec<Finding>`; a
  `blind_spots.secrets_file_touched` list plus its `totals` entry.

- [ ] **Step 1: Write the failing tests**

In `crates/sumcp-core/src/file_class.rs`, add:

```rust
    #[test]
    fn secrets_paths_are_recognized_and_ordinary_config_is_not() {
        assert!(is_secrets("/repo/.env"));
        assert!(is_secrets("/repo/.env.production"));
        assert!(is_secrets("/home/u/.ssh/id_rsa"));
        assert!(is_secrets("/repo/certs/server.pem"));
        assert!(is_secrets("/repo/private.key"));
        assert!(is_secrets("/home/u/.netrc"));
        // Ordinary config is NOT a secret: it must not trip the blind spot.
        assert!(!is_secrets("/repo/Cargo.toml"));
        assert!(!is_secrets("/repo/package.json"));
        assert!(!is_secrets("/repo/src/main.rs"));
    }

    #[test]
    fn secrets_files_still_classify_as_config() {
        // The blind spot is the instrument for a secrets touch, not the
        // ranking: a secrets file keeps the low Config tier.
        assert_eq!(classify("/repo/.env"), FileClass::Config);
        assert_eq!(classify("/repo/certs/server.pem"), FileClass::Config);
    }
```

Create `crates/sumcp-core/src/signals/secrets.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::{FindingKind, Lane};

    fn tool(id: &str, ts: &str, name: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"{name}","input":{{"file_path":"{file}"}}}}]}}}}"#
        )
    }

    #[test]
    fn a_read_of_a_secrets_file_is_a_finding() {
        let raw = tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/.env");
        let s = ingest_str(&raw, Lane::Main);
        let f = secrets(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].kind, FindingKind::SecretsFileTouched);
        assert_eq!(f[0].file.as_deref(), Some("/repo/.env"));
        assert_eq!(f[0].nums.get("reads"), Some(&1.0));
        assert_eq!(f[0].nums.get("edits"), Some(&0.0));
        assert_eq!(f[0].idxs.len(), 1);
    }

    #[test]
    fn reads_and_edits_of_one_file_collapse_into_one_finding() {
        let raw = format!(
            "{}\n{}\n{}",
            tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/.env"),
            tool("e1", "2026-01-01T00:00:02Z", "Edit", "/repo/.env"),
            tool("r2", "2026-01-01T00:00:03Z", "Read", "/repo/.env"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let f = secrets(&s);
        assert_eq!(f.len(), 1, "one finding per file, not per action");
        assert_eq!(f[0].nums.get("reads"), Some(&2.0));
        assert_eq!(f[0].nums.get("edits"), Some(&1.0));
        assert_eq!(f[0].idxs.len(), 3, "every touching action is cited");
    }

    #[test]
    fn ordinary_files_produce_nothing() {
        let raw = format!(
            "{}\n{}",
            tool("e1", "2026-01-01T00:00:01Z", "Edit", "/repo/src/main.rs"),
            tool("e2", "2026-01-01T00:00:02Z", "Edit", "/repo/Cargo.toml"),
        );
        let s = ingest_str(&raw, Lane::Main);
        assert!(secrets(&s).is_empty());
    }

    #[test]
    fn findings_are_ordered_by_path_for_determinism() {
        let raw = format!(
            "{}\n{}",
            tool("r1", "2026-01-01T00:00:01Z", "Read", "/repo/z.pem"),
            tool("r2", "2026-01-01T00:00:02Z", "Read", "/repo/a.pem"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let files: Vec<&str> = secrets(&s).iter().filter_map(|f| f.file.as_deref()).collect();
        assert_eq!(files, vec!["/repo/a.pem", "/repo/z.pem"]);
    }
}
```

In `crates/sumcp-core/src/review.rs`, add:

```rust
    #[test]
    fn a_single_secrets_touch_qualifies_alone() {
        // Zero-tolerance rule: one occurrence is the whole signal, so it must
        // not need a second finding to clear the floor.
        let all = vec![finding(FindingKind::SecretsFileTouched, "/repo/.env")];
        let picked = needs_review(&[], &all);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].file, "/repo/.env");
        assert!(reason_sentence(&picked[0]).contains("secrets file"));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p sumcp-core secrets`
Expected: FAIL to compile, `cannot find function is_secrets`, `no variant
SecretsFileTouched`, `cannot find function secrets`.

- [ ] **Step 3: Add `is_secrets` and route `classify` through it**

In `crates/sumcp-core/src/file_class.rs`, add the table and function, then make
the existing dotenv branch in `classify` call `is_secrets` so there is exactly
one definition of what a secrets path is:

```rust
/// Basenames that are secrets outright, matched exactly.
const SECRET_NAMES: &[&str] = &[".netrc", ".pgpass", "credentials"];
/// Basename prefixes that mark a secret. `.env` itself is matched exactly by
/// `is_secrets`; these cover the suffixed and keypair forms.
const SECRET_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "secrets."];
/// Extensions that carry key material.
const SECRET_EXT: &[&str] = &["pem", "key", "p12", "pfx"];

/// Whether a path names a credentials or key file.
///
/// Deliberately NARROW and deny-list shaped: a false positive here puts a file
/// in the review queue that does not belong there, which trains the reader to
/// ignore the signal. Extend the tables rather than loosening the matching.
///
/// This is the ONLY definition of a secrets path. [`classify`] calls it, so a
/// path recognized here always classifies as [`FileClass::Config`] and the two
/// cannot disagree.
pub fn is_secrets(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    if name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if SECRET_NAMES.contains(&name) {
        return true;
    }
    if SECRET_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    match name.rsplit_once('.') {
        Some((_, ext)) => SECRET_EXT.contains(&ext),
        None => false,
    }
}
```

Replace `classify`'s dotenv branch with a call to `is_secrets`, keeping it as
the FIRST check so it still wins over every other rule.

- [ ] **Step 4: Add the finding kind**

In `crates/sumcp-core/src/model.rs`, add to `FindingKind`, after `ReviewBurden`:

```rust
    /// A credentials or key file was read, edited, or written. Zero-tolerance
    /// by design: one occurrence is the entire signal, so it solo-qualifies
    /// for review rather than needing a second finding. Surfaced through
    /// `blind_spots`, not through the ranking, because the ranking puts
    /// `Config` last and burying this would defeat the point.
    SecretsFileTouched,
```

Confirm the enum's serde attribute renders it as `secrets_file_touched`; the
enum already serializes snake_case, so no per-variant attribute is needed.

- [ ] **Step 5: Write the detector**

Insert above the test module in `crates/sumcp-core/src/signals/secrets.rs`:

```rust
//! Secrets-file touches (spec 2026-07-26, added during execution).
//!
//! One finding per secrets-class file that the session read, edited, or wrote.
//! Exact and high-confidence: this is a literal fact about the action log, not
//! an inference. Per file rather than per action so a file read ten times
//! produces one review item carrying ten citations.

use crate::file_class::is_secrets;
use crate::model::{ActionKind, Confidence, Finding, FindingKind, Idx, Session, Tier};
use std::collections::BTreeMap;

/// Findings for every secrets-class file the session touched, ordered by path.
pub fn secrets(s: &Session) -> Vec<Finding> {
    // BTreeMap so the output order is path order: deterministic without a
    // separate sort.
    let mut per_file: BTreeMap<&str, (u64, u64, Vec<Idx>)> = BTreeMap::new();
    for a in &s.actions {
        let Some(file) = a.file_path.as_deref() else {
            continue;
        };
        let is_read = matches!(a.kind, ActionKind::Read);
        let is_write = matches!(a.kind, ActionKind::Edit | ActionKind::Write);
        if !(is_read || is_write) || !is_secrets(file) {
            continue;
        }
        let entry = per_file.entry(file).or_insert((0, 0, Vec::new()));
        if is_read {
            entry.0 += 1;
        } else {
            entry.1 += 1;
        }
        entry.2.push(a.idx);
    }

    per_file
        .into_iter()
        .map(|(file, (reads, edits, idxs))| {
            let mut nums = BTreeMap::new();
            nums.insert("reads".to_string(), reads as f64);
            nums.insert("edits".to_string(), edits as f64);
            Finding {
                kind: FindingKind::SecretsFileTouched,
                tier: Tier::T1,
                exact: true,
                confidence: Confidence::High,
                idxs,
                file: Some(file.to_string()),
                note: Some(format!(
                    "credentials or key file: {reads} read(s), {edits} write(s)"
                )),
                nums,
            }
        })
        .collect()
}
```

Register it in `crates/sumcp-core/src/signals.rs`, alphabetically:
`pub mod secrets;` after `pub mod failures;`, and
`pub use secrets::secrets;` after `pub use failures::failures;`.

Add it to `all_findings` in `crates/sumcp-core/src/score.rs`:
`f.extend(signals::secrets(s));` after the `comprehension` line.

Leave `ranked_category` returning `None` for the new kind, which the existing
`_ => None` arm already does. It is not a struggle category and must not
contribute to the ordering.

- [ ] **Step 6: Make it solo-qualify and give it a phrase**

In `crates/sumcp-core/src/review.rs`, add `FindingKind::SecretsFileTouched` to
the `matches!` list in `is_solo_qualifying`, and add to `reason_sentence`,
placed FIRST among the non-ranking phrases so it leads the sentence:

```rust
    let sec_n = count_of(&FindingKind::SecretsFileTouched);
    if sec_n > 0 {
        parts.push("secrets file touched".into());
    }
```

Place this block before the existing `flip_n` block so the phrase order puts it
ahead of the others.

- [ ] **Step 7: Add it to `blind_spots`**

In `crates/sumcp-core/src/payloads.rs`, inside `blind_spots`: add
`let secrets = of_kind(FindingKind::SecretsFileTouched);`, include `&secrets`
in the `findings_cut` chain and in the `longest` maximum, add
`"secrets_file_touched": list(&secrets),` as the FIRST list in the JSON object,
and add `"secrets_file_touched": secrets.len(),` to `totals`.

Add this test:

```rust
    #[test]
    fn blind_spots_reports_a_secrets_touch() {
        let raw = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"tool_use","id":"r1","name":"Read","input":{{"file_path":"/repo/.env"}}}}]}}}}"#
        );
        let s = crate::ingest::ingest_str(&raw, crate::model::Lane::Main);
        let p = blind_spots(&s, &meta());
        assert_eq!(p["totals"]["secrets_file_touched"], 1);
        assert_eq!(p["secrets_file_touched"][0]["kind"], "secrets_file_touched");
    }
```

- [ ] **Step 8: Update the contract files**

- `scripts/check_payloads.py`: add `"secrets_file_touched"` to the `KINDS` set.
- `fixtures/mock-payloads/blind_spots.json`: add a
  `"secrets_file_touched": []` list and a `"secrets_file_touched": 0` total, so
  the mock shows the field's shape. Keep every existing value unchanged.
- `docs/payload-schema.md`: extend the v1 section with a row for the new
  `blind_spots` list and the new finding kind.
- `docs/metrics.md`: add a row for `secrets_file_touched`: T1, exact,
  high confidence, solo-qualifying, surfaced in `blind_spots`. State that it
  has NO predictive validation, because it did not exist when the corpus was
  measured, and that it is a policy signal rather than a measured one.

- [ ] **Step 9: Run everything**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_payloads.py
python3 scripts/check_narration.py
```

Expected: all five clean.

- [ ] **Step 10: Confirm it fires on a real touch**

```bash
cargo build --release -p sumcp-cli
printf '%s\n' '{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/tmp/demo/.env"}}]}}' > /tmp/secrets-probe.jsonl
./target/release/sumcp --file /tmp/secrets-probe.jsonl 2>&1 | head -20
```

Expected: the run completes and reports the session. A one-action transcript is
below most detector thresholds, so the value of this probe is that the binary
does not crash on the new kind; the unit tests are what prove the finding
fires.

- [ ] **Step 11: Commit**

```bash
git add -A
git commit -m "core: surface a secrets-file touch as a blind spot

A .env file should never be read or edited, so a touch is the most important
thing a report can say. Ranking is the wrong instrument for a zero-tolerance
rule: Config sits in the last tier, so the ranking would have buried it.

One finding per secrets-class file that was read, edited, or written, exact
and high-confidence because it is a literal fact about the action log. It
solo-qualifies for review, so one occurrence is enough, and it is surfaced
through blind_spots rather than the ranking. Ordinary config churn keeps its
low rank.

file_class::is_secrets is the single definition of a secrets path and classify
calls it, so the classifier and the detector cannot disagree. The table is
deliberately narrow: a false positive here trains the reader to ignore the
signal. Secret values were already safe, since redact.rs scrubs excerpts on
the evidence path.

No predictive validation: this kind did not exist when the corpus was
measured. It is a policy signal, not a measured one, and metrics.md says so."
```

---

### Task 5: Publish the measurement note

**Files:**
- Create: `docs/validation/2026-07-26-ceiling-analysis.md`
- Modify: `docs/validation/2026-07-22-predictive-validity.md` (pointer only)
- Modify: `docs/assets/report-hero.png`

**Interfaces:**
- Consumes: the Task 3 output.

- [ ] **Step 1: Write the measurement note**

Create `docs/validation/2026-07-26-file-class-measurement.md`. This is a short
note, not a study: it records the one measurement that justifies the ranking
change, so the change is not asserted from taste.

State up front what it is and is not: a descriptive breakdown of the SAME tune
split the 2026-07-22 study reports (552 (session, file) pairs, 39 strong
recurrence outcomes, held-out project excluded, outcome and window definitions
unchanged), computed on 2026-07-26. No model is fitted and no threshold is
swept, so there is nothing here to overfit. It is not a second predictive study
and makes no accuracy claim.

The figures, by file class, over all 552 tune pairs:

| class | pairs | outcomes | rate |
|---|---|---|---|
| code | 285 | 34 | 0.119 |
| docs | 192 | 1 | 0.005 |
| config | 37 | 0 | 0.000 |
| notes | 19 | 3 | 0.158 |
| other | 12 | 0 | 0.000 |
| web | 7 | 1 | 0.143 |

And the two observations that follow, stated no more strongly than the cells
support:

- Documentation is 35% of the pair population and carries 1 of 39 outcomes.
  Config is 37 pairs and carries none. Ranking these below code is supported.
- `notes` shows a HIGHER rate than code on 19 pairs and 3 outcomes, and `web`
  on 7 pairs and 1. Both are far too thin to order confidently, which is why
  `notes` sits directly below code rather than beside documentation and `web`
  is grouped with code. Say plainly that only the code-versus-docs-and-config
  boundary rests on adequate data.

Add one paragraph recording the concrete defect this fixes: on the demo fixture
the previous weighted ranking placed two markdown files and a never-edited
image above a `.py` file that had a failure loop, the image scoring purely on
re-reads.

Add a caveats section: single-author single-machine corpus; descriptive only;
thin cells in every class except code; and that the corpus is a rolling window
under a 30-day cleanup, so this exact population cannot be recomputed from the
live transcript directory.

Do NOT invent numbers. Every figure above is given; if you believe one is
wrong, stop and report rather than substituting your own.

- [ ] **Step 2: Add the forward pointer**

At the top of `docs/validation/2026-07-22-predictive-validity.md`, under the
existing `Status:` block, add:

```markdown
See also `docs/validation/2026-07-26-file-class-measurement.md`, a descriptive
breakdown of this same tune split by file class. It is what motivated replacing
the weighted score with a stated ordering rule. The numbers below stand as
recorded and were not recomputed.
```

- [ ] **Step 2b: Give the demo fixture readable paths**

The hero screenshot is the README's main image and currently shows paths like
`/work/proj/f_d5e05a18.py`, an artifact of sanitizing the fixture from a real
session. A reader cannot tell what they are looking at, which the Codex product
review named directly: the screenshot "reads more as instrumentation than
insight" and "sanitized hashed filenames make the value difficult to understand
immediately."

Nothing in the repo references these names. Verify that yourself before editing,
since it is the whole basis for this step being safe:

```bash
grep -rln "f_d5e05a18\|f_04877e9f\|f_ea8148c7" --include='*.rs' --include='*.py' \
  --include='*.json' --include='*.md' . | grep -v '^./target'
```

Expected: no output. If anything IS listed, stop and report rather than
renaming.

Then rewrite the paths in `fixtures/demo/demo-session.jsonl` to plausible
INVENTED ones. They must stay entirely synthetic: no real project name, no real
directory from this machine. Keep the same file extensions, because the classes
drive the ranking and the point of the image is that code outranks docs. A
mapping that preserves the current ranking shape:

| current | replace with |
|---|---|
| the 8-edit `.py` | `src/store/data_store.py` |
| the 2-edit `.py` with failure loops | `src/api/routes.py` |
| the 4-edit `.md` | `docs/architecture.md` |
| the 3-edit `.md` | `docs/api-notes.md` |
| the 2-edit `.md` | `README.md` |
| the never-edited `.jpg` | `assets/diagram.jpg` |

Replace every occurrence of each old path, including inside tool inputs, tool
results, and any `structuredPatch` content, so the transcript stays internally
consistent. A path that appears in an edit but not in its matching result would
change what the detectors see.

Drop the `/work/proj/` prefix: relative paths read better in the report and the
tool does not require absolute ones.

AFTER editing, confirm the ranking shape is unchanged apart from the names:

```bash
cargo build --release -p sumcp-cli
./target/release/sumcp --file fixtures/demo/demo-session.jsonl 2>/dev/null \
  | sed -n '/struggle areas/,$p' | head -8
```

Expected: both `.py` files first with the same edit counts (8, then 2), then the
`.md` files (4, 3, 2), and the `.jpg` last. If any edit count or the order
changed, a replacement was incomplete: fix it rather than accepting the new
output.

Run `cargo test --workspace` and `python3 scripts/check_payloads.py` again after
this edit, since a fixture change can move a test that parses it.

- [ ] **Step 3: Regenerate the hero screenshot**

The ranking order changed, so `docs/assets/report-hero.png` is stale and the
README's main image would misrepresent the tool.

```bash
cargo build --release -p sumcp-cli
./target/release/sumcp --file fixtures/demo/*.jsonl --html > /tmp/hero.html
open /tmp/hero.html
```

This step needs a human: screenshot the report at 820px wide to match the
README's `width="820"`, and save over `docs/assets/report-hero.png`. Confirm
the new image shows code files above documentation. If no demo fixture exists
at that glob, use `fixtures/session-2_1_210-subagents.jsonl`.

- [ ] **Step 4: Verify the report is committable**

```bash
F=docs/validation/2026-07-26-ceiling-analysis.md
echo "em dashes: $(grep -c '—' $F)"
grep -cE '/Users/|raphaelhaytene' $F
grep -oE 'proj-[0-9]+' $F | sort -u
```

Expected: zero em dashes, zero real-path matches, and only anonymized
`proj-NN` identifiers.

- [ ] **Step 5: Commit**

```bash
git add docs/validation/2026-07-26-ceiling-analysis.md \
        docs/validation/2026-07-22-predictive-validity.md \
        docs/assets/report-hero.png
git commit -m "validation: publish the ceiling analysis, refresh the hero

The negative result, with the gate quoted as it was written before the run.
Records all five caveats, including that leave-one-project-out has three
coarse folds and non-monotonic numbers so it is a direction rather than a
measurement, and that the corpus was a 30-day rolling window actively losing
sessions until it was archived.

Hero screenshot regenerated: the old one showed two markdown files and a
never-edited JPEG above a .py file whose commands were failing."
```

- [ ] **Step 6: Final full verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_payloads.py
python3 scripts/check_narration.py
git status --short
```

Expected: all five clean, and a clean tree apart from untracked scratch. Report
the test count so the change in totals from the deleted weight tests is
visible.

---

## Self-Review

**Spec coverage, after the 2026-07-26 descoping.** Spec sections 2a to Task 1;
2b to Tasks 2 and 3; 2c to Task 6; 2d to Task 3; 2e to Task 3; 2f to Task 3;
Part 3 to Tasks 5 and 6. Task 4 is additional scope the user requested during
execution and is not in the original spec.

**Dropped from the spec, deliberately.** Part 1 in full (the dump-field
additions, `scripts/ceiling_analysis.py`, the corpus archive plumbing and pin,
and the confirmation gate) and Part 3's held-out release-eval run. Reason: they
were insurance on a decision the already-published 2026-07-22 study plus the
Task 5 measurement note already support, and the corpus pinning had twice
blocked execution on questions that do not change what ships. No accuracy
claim is being made, so there is no claim for a held-out gate to gate.

**Type consistency.** `FileClass`, `classify`, and `tier` are named identically
in Tasks 1, 4, and 5. `RANKING_RULE` is defined in Task 5 step 3 and consumed in
steps 4, 5, and 6 and in the Task 5 step 1 test. `rank(s: &Session)` and
`struggle_areas(ranked, meta, n)` are used consistently after Task 5. There is exactly ONE
file-class table, in Rust, in `file_class.rs`. Nothing outside that module
classifies a path.

**Ordering hazard.** Task 4 keeps `score` and `Weights` alive so that task
compiles on its own. Task 5 removes them. Do not merge the two tasks: the point
of the split is that Task 4's ordering change is reviewable without the 15-file
contract break attached to it.

---

## Execution Handoff

Plan complete and saved to
`docs/superpowers/plans/2026-07-26-ceiling-verdict-and-simple-ranking.md`.
### Task 6: Documentation claims

**Files:**
- Modify: `docs/metrics.md`, `SPEC.md`, `README.md`, `tasks/todo.md`

**Interfaces:**
- Consumes: `RANKING_RULE` and the Task 3 script output.
- Produces: no code interface.

- [ ] **Step 1: Rewrite the weight column in `docs/metrics.md`**

Remove the weight column from the signal table, since no weights exist.

Cite `docs/validation/2026-07-26-file-class-measurement.md` (written in Task 5)
for the file-class figures, and add a `class` row documenting `file_class`
reproducing the honesty note from that module's rustdoc: only the
code-versus-docs-and-config boundary rests on adequate data.

Then state these three things in prose about the signals whose weights are
gone, and no more than these. The figures are from the same tune split as the
measurement note and are given here; do not invent others:

- `blind_write_attempt` was weighted joint-highest on the strength of
  IDE-Bench's 63% figure, and on this corpus it fired on 24 pairs with **zero**
  outcomes. State the limit precisely: zero in 24 rules out a large positive
  effect, it does not establish the signal is harmful, and it does not refute
  IDE-Bench, whose population is autonomous benchmark trajectories rather than
  interactive sessions.
- `failure_loop` (4 pairs) and `true_revert` (2 pairs) are too rare here to
  characterise, because failures themselves are rare: 58 confirmed failed
  commands across the tune sessions, a median of 1 per session. That is a
  property of this corpus, not of the detector.
- `re_read` had the best rate of the frequent kinds (97 pairs, 21 outcomes,
  0.216) while being weighted BELOW both `rework` (94 pairs, 19, 0.202) and
  `fumble`. `churn` was 242 pairs, 33 outcomes, 0.136. So the
  literature-derived weight ordering did not reproduce on the only corpus it
  has been measured against.

Add one sentence stating that no weighting was retuned, because the weights
were removed rather than adjusted, and that no accuracy claim is made anywhere.

- [ ] **Step 2: Amend `SPEC.md`**

Following the file's existing amendment style, amend decision 6 (transparent
weighted ranking) and ADR A6 (TOML-optional weights). Decision 6 becomes the
four-key rule with `RANKING_RULE` quoted. ADR A6 is marked retired with the
date, the reason, and a pointer to the ceiling analysis. Do not delete the
original text; amend it, so the record of what was decided and why it changed
both survive.

- [ ] **Step 3: Rewrite "The numbers" in `README.md`**

Replace the section with a claim that is exactly what the evidence supports.
It must say all of:

- Flagged files really do recur more than unflagged ones. Every product row in
  the 2026-07-22 study had a relative risk well above 1 with an interval
  excluding 1.
- The ranking is a four-key rule a reader can verify by hand, not a score. The
  2026-07-22 study found the weighted ranking did not beat sorting by edit
  count, so the score was removed rather than retuned.
- Documentation was 35% of the measured file-sessions and carried 1 of 39
  outcomes; restricting the queue to code cut flagged files from 65 to 52 for
  an identical hit count, a 20% reduction in false alarms at no cost to recall.
- Every entry carries deterministic evidence: the exact actions, cited.

It must NOT claim the ranking is more accurate than any alternative. Keep the
existing token-reduction paragraph and the `Limitations` section, and add the
single-author-corpus and 30-day-cleanup caveats to `Limitations`.

- [ ] **Step 4: Close the decision in `tasks/todo.md`**

Tick "Decide what v0.1 claims" and record: option (b) was chosen, a feasibility
pass measured that the goal is unreachable on this corpus, the score was
demoted rather than retuned, and the evidence is in
`docs/validation/2026-07-26-ceiling-analysis.md`. Add a new unticked item for
refreshing the corpus archive before any future validation run, and note that
`cleanupPeriodDays` is still unset.

- [ ] **Step 5: Check the prose gates**

```bash
for f in docs/metrics.md SPEC.md README.md tasks/todo.md; do
  echo "$f em-dashes: $(grep -c '—' $f)"
done
grep -rn "/Users/" README.md docs/metrics.md SPEC.md | head
```

Expected: zero em dashes in every file, and no output from the path grep.

- [ ] **Step 6: Commit**

```bash
git add docs/metrics.md SPEC.md README.md tasks/todo.md
git commit -m "docs: signal evidence instead of weight tiers, close the v0.1 claim

metrics.md drops the weight column, which describes a mechanism that no
longer exists, and records what each signal actually did on the 2026-07-26
tune split. The blind-write row states the limit of its own evidence: zero
outcomes in 24 pairs rules out a large positive effect and does not refute
IDE-Bench, whose population is autonomous trajectories rather than
interactive sessions.

SPEC decision 6 amended to the four-key rule; ADR A6 marked retired. README
'The numbers' now claims what is supported: the flags are predictive, the
order is a rule you can check by hand, restricting to code cut flags 65 to 52
for the same hits, and every entry is cited. No accuracy claim."
```

---

