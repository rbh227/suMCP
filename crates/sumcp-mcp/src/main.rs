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
/// checked-out repo containing `./sumcp/config.toml` silently shadow the
/// real one.
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

/// ADR A6 retired (spec 2026-07-26 §2f): ranking has no weights to configure,
/// so `~/.config/sumcp/config.toml` is no longer read. A user who wrote one is
/// told rather than silently ignored.
fn warn_if_stale_config(path: Option<PathBuf>) {
    if let Some(path) = path
        && path.exists()
    {
        eprintln!(
            "sumcp-mcp: {} is no longer read (ranking weights were removed; \
             see docs/validation/2026-07-26-file-class-measurement.md)",
            path.display()
        );
    }
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

    warn_if_stale_config(config_path());
    let server = server::SumcpServer {
        // Claude Code launches project-scoped stdio servers with cwd = the
        // project root, so this resolves to the right transcript directory.
        project_dir: sumcp_core::locate::project_dir(&home, &cwd),
        store: store::SessionStore::new(),
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
}
