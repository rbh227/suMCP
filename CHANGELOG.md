# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project aims
to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-07-28

The measurement-fidelity release: reports now cover the whole stretch of work,
not one transcript file, and an independent recount holds every countable
quantity to exact agreement. Motivated by a measured defect: one stretch of
work spanned 7 transcripts and 275 file operations, and the v0.1 reporting
unit showed at most 88 of them. See
`docs/superpowers/specs/2026-07-28-v02-measurement-fidelity-design.md`.

### Added
- **Work units.** Transcripts in one project directory that overlap in time or
  sit within 30 minutes of each other merge into one report: resumed sessions,
  `/clear`s, and concurrent Claude Code instances (which become parallel
  lanes, disclosed as negative gaps). The threshold is declared, not fitted;
  grouping was measured to be almost insensitive to it across a 5-to-120
  minute range. Bare `sumcp`, `--work-unit <transcript>`, and the MCP tools
  all report the unit; `--file` stays single-transcript by design and prints
  a stderr note when the named transcript is part of a larger stretch.
- **A differential recount gate** (`scripts/recount.py`): a second,
  deliberately naive counter over raw JSONL that must agree exactly with the
  analyzer on edits, writes, reads, bash calls, and files touched, per
  transcript and per disclosed work unit. Exists because the undercount above
  survived 271 green tests whose fixtures were produced by the same code path
  being tested. CI runs it against committed fixtures; the full-archive run
  is local. Measured on the archive: 85 transcripts, 32 units, exact
  agreement.
- **`mode` and `origin` events adopted.** The parser was counting both as
  unknown. `mode` makes auto-accept suppression per action instead of per
  session, so normal-mode stretches keep their latency signals. `origin`
  distinguishes human turns from harness-injected task notifications, which
  no longer truncate the review-burden window. Absent fields default to the
  old behaviour on pre-existing transcripts.
- A performance guard test (16 members, 6400 actions, generous 10 s ceiling,
  aimed at algorithmic regressions only) and measured numbers in the spec:
  the real worst-case unit (14 transcripts, 23.8 MB) analyzes in about
  0.25 s with under 35 MB peak RSS.

### Changed
- **BREAKING: payload contract v1 to v2.** Every payload's `v` is now `2`.
  `session_overview` carries a `work_unit` block (rule, member ids, gaps,
  span, and disclosure counts: `dropped` for the size cap,
  `members_unreadable` for discovered members that failed to load,
  `siblings_unplaced` for transcripts that could not be placed in time), and
  findings in `struggle_areas` and `blind_spots` carry a `session` key
  resolving them to their originating transcript. Every count in the block
  describes the analyzed members, so its internal invariants hold even when
  a member could not be read.
- Adjacency findings (rework, reverts, loops, failure attribution) key on
  (transcript, lane) rather than lane alone, so a merged unit can never pair
  two actions from different transcripts as if they were consecutive.

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

[0.2.0]: https://github.com/rbh227/backstory-mcp/releases/tag/v0.2.0
[0.1.0]: https://github.com/rbh227/backstory-mcp/releases/tag/v0.1.0
