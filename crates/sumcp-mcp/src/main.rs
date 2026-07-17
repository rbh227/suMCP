//! `sumcp-mcp` — MCP server over stdio (T4.1).
//!
//! Six read-only forensics tools with fail-closed session identification
//! (ADR A4). The async runtime lives only in this binary (ADR A2);
//! `sumcp-core` stays synchronous and pure.

mod identify;
mod server;
mod store;

use rmcp::ServiceExt as _;
use std::path::PathBuf;
use sumcp_core::score::Weights;

/// Load the optional weights override (ADR A6): `~/.config/sumcp/config.toml`
/// (or `$XDG_CONFIG_HOME/sumcp/config.toml`). Missing file → compiled
/// defaults. A *broken* file also falls back to defaults, but says so on
/// stderr — a bad config must never take the server down.
fn load_weights_from(path: Option<PathBuf>) -> Weights {
    let Some(path) = path else {
        return Weights::default();
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // No config file at all — the normal, silent case.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Weights::default(),
        // A config that EXISTS but can't be read (permissions, encoding) is
        // a broken config, and broken configs must say so — silence here
        // would be indistinguishable from "no config".
        Err(e) => {
            eprintln!(
                "sumcp-mcp: cannot read {} ({e}); using default weights",
                path.display()
            );
            return Weights::default();
        }
    };
    match toml::from_str::<Weights>(&raw) {
        Ok(mut w) => {
            // Transparency guardrail: payloads echo where weights came from.
            w.source = path.display().to_string();
            w
        }
        Err(e) => {
            eprintln!(
                "sumcp-mcp: ignoring malformed {} ({e}); using default weights",
                path.display()
            );
            Weights::default()
        }
    }
}

/// `$XDG_CONFIG_HOME/sumcp/config.toml`, falling back to `~/.config/…`.
fn config_path() -> Option<PathBuf> {
    config_path_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

/// The pure core of [`config_path`] (env-free, so tests can drive it).
/// Per the XDG spec, an empty or relative `XDG_CONFIG_HOME` is IGNORED —
/// honoring a relative one would resolve against our cwd, letting a
/// checked-out repo containing `./sumcp/config.toml` silently override the
/// user's weights.
fn config_path_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = match xdg {
        Some(p) if p.is_absolute() => p,
        _ => home?.join(".config"),
    };
    Some(base.join("sumcp").join("config.toml"))
}

/// `~/.claude`, overridable via `SUMCP_CLAUDE_HOME` (tests point this at a
/// fixture tree; there is no other reason to set it).
fn claude_home() -> Option<PathBuf> {
    std::env::var_os("SUMCP_CLAUDE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
}

// `current_thread`: one connection over stdio needs no thread pool.
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let Some(home) = claude_home() else {
        // No $HOME at all — nothing to scan; refuse loudly rather than serve
        // tools that can never answer.
        eprintln!("sumcp-mcp: neither SUMCP_CLAUDE_HOME nor HOME is set; exiting");
        std::process::exit(1);
    };

    let server = server::SumcpServer {
        // Claude Code launches project-scoped stdio servers with cwd = the
        // project root, so this resolves to the right transcript directory.
        project_dir: sumcp_core::locate::project_dir(&home, &cwd),
        store: store::SessionStore::new(),
        weights: load_weights_from(config_path()),
    };

    // serve() runs the MCP handshake; waiting() parks until the client
    // disconnects (Claude Code closing stdin) — then we exit cleanly.
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_yields_defaults() {
        let w = load_weights_from(Some(PathBuf::from("/nonexistent/config.toml")));
        assert_eq!(w.source, "defaults");
    }

    #[test]
    fn partial_toml_overrides_and_records_source() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "churn = 9.5\n").unwrap();
        let w = load_weights_from(Some(path.clone()));
        assert_eq!(w.churn, 9.5);
        // Unspecified fields keep their defaults (serde(default) on Weights).
        assert_eq!(w.rework, 2.0);
        assert_eq!(w.source, path.display().to_string());
    }

    #[test]
    fn empty_or_relative_xdg_config_home_is_ignored() {
        let home = Some(PathBuf::from("/home/u"));
        let expect = PathBuf::from("/home/u/.config/sumcp/config.toml");
        // Empty (common misconfiguration) and relative both fall through.
        assert_eq!(
            config_path_from(Some(PathBuf::from("")), home.clone()),
            Some(expect.clone())
        );
        assert_eq!(
            config_path_from(Some(PathBuf::from("rel/dir")), home.clone()),
            Some(expect)
        );
        // Absolute XDG wins.
        assert_eq!(
            config_path_from(Some(PathBuf::from("/xdg")), home),
            Some(PathBuf::from("/xdg/sumcp/config.toml"))
        );
        // No XDG, no HOME → no path at all.
        assert_eq!(config_path_from(None, None), None);
    }

    #[test]
    fn unreadable_config_falls_back_to_defaults() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "churn = 9.5\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let w = load_weights_from(Some(path.clone()));
        assert_eq!(w.source, "defaults");
        // restore so tempdir cleanup can delete it
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn malformed_toml_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "churn = \"not a number\"").unwrap();
        let w = load_weights_from(Some(path));
        assert_eq!(w.source, "defaults");
        assert_eq!(w.churn, 1.0);
    }
}
