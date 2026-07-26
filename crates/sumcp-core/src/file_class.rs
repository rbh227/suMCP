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
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "h", "cc", "cpp", "hpp", "rb",
    "swift", "kt", "sh", "bash", "zsh", "sql", "vue", "svelte", "cs", "php", "lua", "scala",
    "dart", "ex", "exs", "clj", "hs", "ml", "m", "mm", "r", "pl",
];
const WEB_EXT: &[&str] = &["html", "css", "scss", "sass", "less"];
const DOCS_EXT: &[&str] = &["md", "mdx", "txt", "rst", "adoc", "tex"];
const CONFIG_EXT: &[&str] = &[
    "json",
    "toml",
    "yaml",
    "yml",
    "ini",
    "cfg",
    "conf",
    "env",
    "lock",
    "properties",
    "gradle",
    "xml",
    "plist",
];

/// Classify a path. Precedence matters and is tested:
///
/// 1. A basename that is exactly `.env` or starts with `.env.` is
///    [`FileClass::Config`], so `.env.local` and `.env.production` are
///    caught alongside `.env`, but `.environment.rs` is not: the boundary
///    stops at the dot, so the extension table still gets a look.
/// 2. A memory or notes DIRECTORY (a path segment, not merely a prefix of
///    one) is [`FileClass::Notes`], checked BEFORE extensions so
///    `memory/plan.md` is notes rather than documentation, while
///    `notesctl/main.rs` is still code.
/// 3. Extension tables, in the order code, docs, config, web.
/// 4. Everything else is [`FileClass::Other`].
pub fn classify(path: &str) -> FileClass {
    let lower = path.to_ascii_lowercase();
    // `rsplit('/')` always yields at least one item, so a bare filename with
    // no directory component still lands here.
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if name == ".env" || name.starts_with(".env.") {
        return FileClass::Config;
    }
    if lower.contains("/memory/") || name.starts_with("memory.") || lower.contains("/notes/") {
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
    fn a_notes_prefixed_directory_is_not_a_notes_file() {
        // "/notes" without a trailing slash also matches "notesctl", which
        // would demote real source below documentation.
        assert_eq!(classify("/repo/notesctl/src/main.rs"), FileClass::Code);
        assert_eq!(classify("/repo/notesapp/index.ts"), FileClass::Code);
        // A file merely NAMED notes is still source if it has a code extension.
        assert_eq!(classify("/repo/notes.rs"), FileClass::Code);
        // A real notes directory still classifies as notes.
        assert_eq!(classify("/repo/notes/scratch.md"), FileClass::Notes);
    }

    #[test]
    fn dotenv_matching_stops_at_the_documented_boundary() {
        assert_eq!(classify("/repo/.env"), FileClass::Config);
        assert_eq!(classify("/repo/.env.local"), FileClass::Config);
        // Not a dotenv file: the extension table must still get a look.
        assert_eq!(classify("/repo/.environment.rs"), FileClass::Code);
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
