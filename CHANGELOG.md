# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-07-26

First release. Prebuilt archives for five targets, each built and executed in
CI before publishing.

### Added
- Six read-only MCP tools (`session_overview`, `struggle_areas`, `file_story`,
  `blind_spots`, `context_health`, `evidence`).
- Deterministic transcript ingest with subagent flat-merge and a total
  ordering contract.
- Self-contained HTML report (`sumcp --file <session> --html`), and bare
  `sumcp` analyzes the newest session for the current project.
- `install` / `uninstall`, writing only under `$HOME`, dry-run by default and
  fully reversible from a manifest.
- **Native Windows support** (x86-64). The analysis engine and MCP server were
  already portable; the installer needed a no-op for Unix mode bits, a
  `USERPROFILE` fallback for the home directory, and `EXE_SUFFIX` on the
  sibling-binary lookup. Two documented differences there: the Stop hook is a
  `/bin/sh` script so it is not installed, and installed files are not
  permission-restricted because Windows has no mode bits.
- **A secrets-file blind spot.** One read, edit, or write of a credentials or
  key file puts that file in the review queue on its own. `Config` sits in the
  last ranking tier, so ordering was the wrong instrument for a rule that
  tolerates zero occurrences.
- Statically linked `x86_64-unknown-linux-musl` archive, for distributions
  whose glibc is older than the build runner's.
- Dual license: MIT OR Apache-2.0.

### Changed
- **BREAKING: the weighted ranking score is gone.** Ranking is now four
  declared keys: edited files before never-edited ones, then file class, then
  edit count, then path. The 2026-07-22 study found the weighted ranking did
  not beat sorting by edit count, and fitting the weights to the outcomes
  themselves gained at most 4 hits out of 39, so the score was removed rather
  than retuned. See `docs/validation/2026-07-26-file-class-measurement.md`.
- **BREAKING: payload contract v0 to v1.** `struggle_areas` drops `weights`
  and per-file `score` for `ranking_rule` plus per-file `class` and `edits`;
  `session_overview.top_struggles` makes the same per-file swap.
- **BREAKING: ADR A6 retired.** `~/.config/sumcp/config.toml` set ranking
  weights, which no longer exist, so it is no longer read. A stale config gets
  a one-line notice. This also closes a divergence where the CLI used default
  weights while the MCP server loaded the config, so the two surfaces could
  rank the same session differently.
- Payload caps are enforced by construction for all six tools, with disclosure
  fields naming whatever was dropped.

### Fixed
- The Intel macOS archive was cross-compiled and never executed anywhere. It
  now runs under Rosetta 2 in CI, on every push rather than only at tag time.
- `docs/metrics.md` no longer documents weight tiers for a mechanism that does
  not exist, and records what each signal actually did on the measured corpus,
  including that the joint-highest-weighted signal fired 24 times with zero
  outcomes.

### Notes
- macOS binaries carry only Rust's ad-hoc signature. Notarization needs a paid
  Apple Developer ID this project does not have, so a browser download is
  quarantined by Gatekeeper until you clear the attribute. `curl` downloads
  are not.
- No external user has validated the ranking. That is the top post-v0.1 item.

[0.1.0]: https://github.com/rbh227/suMCP/releases/tag/v0.1.0
