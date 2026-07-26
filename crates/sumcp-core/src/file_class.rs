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

    /// The class name as it appears in payloads and reports. One definition,
    /// so the JSON serialization and every rendered surface cannot diverge:
    /// `Debug` formatting would print `WebAsset` where serde prints
    /// `web_asset`.
    pub fn as_str(self) -> &'static str {
        match self {
            FileClass::Code => "code",
            FileClass::Web => "web",
            FileClass::Notes => "notes",
            FileClass::Docs => "docs",
            FileClass::Config => "config",
            FileClass::Other => "other",
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

/// Basenames that are secrets outright, matched exactly.
const SECRET_NAMES: &[&str] = &[".netrc", ".pgpass", "credentials"];
/// Basename prefixes naming an SSH/PGP keypair's PRIVATE half. `.env` itself
/// is matched exactly by `is_secrets`; this covers the suffixed and keypair
/// forms. The public half (`*.pub`) is excluded up front in `is_secrets`,
/// before this table is ever consulted, so it never needs its own carve-out
/// here. A doc-like extension (a notes/setup file merely NAMED after a key)
/// is also exempted where this table is consulted; see `is_secrets`.
const KEYPAIR_PREFIXES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"];
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
    // Basename extension, empty for an extensionless name like `id_rsa`.
    // Computed once so every boundary check below reads the same value.
    let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");

    // A public key's entire purpose is being shared, so it is never a
    // secret. Checked FIRST, before any table below, so it also covers any
    // future key type this file learns about: `whatever.pub` is exempt no
    // matter what `whatever` would otherwise match.
    if name.ends_with(".pub") {
        return false;
    }

    if name == ".env" || name.starts_with(".env.") {
        return true;
    }
    if SECRET_NAMES.contains(&name) {
        return true;
    }
    // A doc-like extension means this is prose ABOUT a key (a setup guide, a
    // notes file), not the key material itself, so it is exempted the same
    // way `secrets.` is below. A real key has no extension, and a real
    // backup uses a non-doc one (`.bak`, `.orig`), so neither is affected.
    if KEYPAIR_PREFIXES.iter().any(|p| name.starts_with(p)) && !DOCS_EXT.contains(&ext) {
        return true;
    }
    // `secrets.` counts ONLY when the extension is not doc-like. This is the
    // INVERSE of `classify`'s `memory.` rule: there, a doc-like extension is
    // what makes a `memory.*` file matter (it is prose worth keeping as
    // notes). Here, a doc-like extension is what makes a `secrets.*` file
    // NOT matter (it is prose ABOUT secrets, e.g. a `SECRETS.md` runbook,
    // not a credential). `secrets.json`/`secrets.yaml` still count: their
    // extension isn't prose, so nothing exempts them.
    if name.starts_with("secrets.") && !DOCS_EXT.contains(&ext) {
        return true;
    }

    SECRET_EXT.contains(&ext)
}

/// Classify a path. Precedence matters and is tested:
///
/// 1. A path that [`is_secrets`] recognizes (`.env` and its suffixed
///    variants, plus the credentials/key tables) is [`FileClass::Config`],
///    checked FIRST so it wins over every other rule. `classify` and
///    `is_secrets` share this one definition of a secrets path, so the two
///    can never disagree about what counts.
/// 2. A memory or notes DIRECTORY (a path segment, not merely a prefix of
///    one) is [`FileClass::Notes`] regardless of extension, checked BEFORE
///    extensions so `memory/helper.rs` is notes even though `.rs` is
///    otherwise code, while `notesctl/main.rs` is still code.
/// 3. A basename starting with `memory.` is [`FileClass::Notes`] only when
///    its extension is doc-like (in [`DOCS_EXT`]), so `memory.md` and
///    `memory.txt` are notes but `memory.rs` and `memory.ts` are still
///    classified by the extension tables below.
/// 4. Extension tables, in the order code, docs, config, web.
/// 5. Everything else is [`FileClass::Other`].
pub fn classify(path: &str) -> FileClass {
    let lower = path.to_ascii_lowercase();
    // `rsplit('/')` always yields at least one item, so a bare filename with
    // no directory component still lands here.
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if is_secrets(path) {
        return FileClass::Config;
    }
    // Directory-based notes/memory paths win regardless of extension.
    if lower.contains("/memory/") || lower.contains("/notes/") {
        return FileClass::Notes;
    }

    // `rsplit_once` on the BASENAME, so a dot in a parent directory cannot be
    // mistaken for an extension. A leading-dot file like `.gitignore` yields
    // ("", "gitignore"), which matches no table and falls through to Other.
    let Some((_stem, ext)) = name.rsplit_once('.') else {
        return FileClass::Other;
    };

    // Unlike the directory check above, a bare `memory.` basename prefix is
    // NOT extension-blind: `memory.rs` is still source, only a doc-like
    // extension turns it into notes.
    if name.starts_with("memory.") && DOCS_EXT.contains(&ext) {
        return FileClass::Notes;
    }

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
    fn a_memory_prefixed_source_file_is_code() {
        // "memory." as a bare basename prefix also captured source files,
        // the same overreach the notes clause had.
        assert_eq!(classify("/repo/src/memory.rs"), FileClass::Code);
        assert_eq!(classify("/repo/memory.ts"), FileClass::Code);
        // Doc-like memory files are still notes.
        assert_eq!(classify("/repo/memory.md"), FileClass::Notes);
        assert_eq!(classify("/repo/memory.txt"), FileClass::Notes);
    }

    #[test]
    fn a_memory_directory_beats_the_extension_table() {
        assert_eq!(classify("/home/u/.claude/memory/plan.md"), FileClass::Notes);
        assert_eq!(
            classify("/home/u/.claude/memory/helper.rs"),
            FileClass::Notes
        );
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

    #[test]
    fn as_str_matches_the_serde_name_for_every_variant() {
        for c in [
            FileClass::Code,
            FileClass::Web,
            FileClass::Notes,
            FileClass::Docs,
            FileClass::Config,
            FileClass::Other,
        ] {
            let json = serde_json::to_value(c).unwrap();
            assert_eq!(json, serde_json::json!(c.as_str()), "{c:?}");
        }
    }

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

    #[test]
    fn a_document_about_secrets_is_not_a_secret() {
        // "secrets." with no boundary caught prose. Note this is the INVERSE
        // of the memory. rule: a memory file matters when it is prose, a
        // secrets file matters when it is not.
        assert!(is_secrets("/repo/secrets.json"));
        assert!(is_secrets("/repo/secrets.yaml"));
        assert!(!is_secrets("/repo/SECRETS.md"));
        assert!(!is_secrets("/repo/docs/secrets.txt"));
    }

    #[test]
    fn a_public_key_is_not_a_secret() {
        // Publishing a .pub file is its whole purpose. Flagging it spends the
        // reader's attention on a non-event.
        assert!(is_secrets("/home/u/.ssh/id_rsa"));
        assert!(!is_secrets("/home/u/.ssh/id_rsa.pub"));
        assert!(is_secrets("/home/u/.ssh/id_ed25519"));
        assert!(!is_secrets("/home/u/.ssh/id_ed25519.pub"));
    }

    #[test]
    fn a_document_about_a_keypair_is_not_a_secret() {
        // A third instance of the same unbounded-prefix defect class as
        // "secrets.": KEYPAIR_PREFIXES matched by `starts_with` with no
        // extension boundary, so a notes/setup file whose name happens to
        // start with a key basename was flagged too. Real key files and
        // their backups (no extension, or a non-doc extension like `.bak`)
        // still count.
        assert!(!is_secrets("/repo/docs/id_rsa_notes.md"));
        assert!(!is_secrets("/repo/id_rsa_setup.txt"));
        assert!(!is_secrets("/repo/id_dsa_history.md"));
        assert!(is_secrets("/home/u/.ssh/id_rsa"));
        assert!(is_secrets("/home/u/.ssh/id_rsa.bak"));
    }
}
