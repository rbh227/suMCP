# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- v0.1 pre-release: six read-only MCP tools (`session_overview`,
  `struggle_areas`, `file_story`, `blind_spots`, `context_health`, `evidence`).
- Deterministic transcript ingest with subagent flat-merge and a total
  ordering contract.
- Self-contained HTML report (`sumcp --file <session> --html`).
- `install` / `uninstall` with a Stop-hook debrief nudge, writing only under
  `$HOME` and fully reversible.
- Dual license: MIT OR Apache-2.0.
