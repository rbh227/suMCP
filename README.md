<p align="center">
  <img src="docs/assets/wordmark.svg" alt="suMCP" width="420">
</p>

<p align="center"><b>One agent writes the code. Another reviews it. suMCP hands the reviewer the context only the session had.</b></p>

<p align="center">
  <img alt="license: MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-edition%202024-orange">
  <img alt="deterministic, no LLM, no network" src="https://img.shields.io/badge/deterministic-no%20LLM%20%C2%B7%20no%20network-2ea44f">
  <img alt="release: v0.2.0" src="https://img.shields.io/badge/release-v0.2.0-000080">
  <img alt="platforms: macOS, Linux, Windows" src="https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555555">
</p>

> **v0.2.0 is released.** Prebuilt archives for macOS (Apple Silicon and Intel),
> Linux (glibc and static musl), and Windows are on the
> [releases page](https://github.com/rbh227/suMCP/releases/latest), each with a
> SHA-256 checksum.
>
> **The v0.3 line (in this repository, not yet in a release archive) changes
> who suMCP is for.** The consumer is no longer a human deciding what to read.
> It is the *reviewing agent* you run after your coding agent commits, and the
> goal is precision: fewer false findings, not more places to look. See
> [What is and is not established](#what-is-and-is-not-established) before
> trusting any claim on this page.

---

## The problem, measured

A common workflow now: one agent (say, Claude Code) writes and commits, a
second agent (say, Codex) reviews the commit. The reviewer holds the diff and
nothing else. It can check whether the code is internally consistent, but not
whether it does what was *wanted*, because it infers the intent from the very
code it is reviewing.

The measured cost of that blindness is not missed bugs. It is noise. In a
study of 31,073 agentic review comments across 10,191 pull requests,
**56.3% were rejected** by developers, mostly for being *out of scope* or
*misaligned with what the developer intended*
([arXiv:2607.03316](https://arxiv.org/abs/2607.03316)). Scope and intent are
not properties of code. They are properties of the conversation, and the
conversation is sitting on disk in `~/.claude/projects/`, deleted after 30
days, read by nobody.

suMCP reads it. Deterministically, in Rust, no LLM, no network. It hands the
reviewing agent, over MCP or the CLI, the recorded facts a diff cannot carry:

- **what was asked**, the human's requests, quoted verbatim, never summarized
- **what was decided**, each question the agent put to the human, the answer
  chosen, and the other options that were offered
- **what was left unfinished**, tasks created and never completed, and
  validation commands still failing when the session ended
- **what the agent claimed it did**, the summary it wrote before handing back,
  for the reviewer to check against the diff

Every item carries a citation into the transcript. Every list is a capped
sample with an unconditional total, so "3 shown" is never readable as
"3 happened". And when coverage is incomplete (turns whose origin could not
be attributed, subagent prose that was excluded), the payload says so in a
top-level `coverage` block instead of implying completeness.

**The invariant: suMCP retrieves, the agent judges.** It never says "this is
fine" or "this is risky", only "here is what was recorded, at this index".
That is why it needs no LLM, has nothing to calibrate, and can be wrong about
nothing except whether it found the right passage.

### Why quoting beats inferring

Meta's ARCTIC ([arXiv:2607.29516](https://arxiv.org/html/2607.29516v1))
validated this thesis at production scale: intent derived from developer-agent
conversation logs, over a million requests, 90.2% engineer approval. It is
also Meta-internal and legally unreleasable, and it *infers* intent with an
LLM at 0.86 F1, roughly one intent in seven wrong. suMCP is the local, open,
transcript-native complement: it *quotes*. A quote is right always, which is
the first time this project's no-LLM rule has been an advantage rather than
a ceiling.

One research finding shapes the tool split below: pushing a full set of
requirements at an LLM reviewer and asking it to check conformance induces
*overcorrection*, where the model starts flagging correct code
([arXiv:2603.00539](https://arxiv.org/pdf/2603.00539)). So the pushed payload
is small and framed as facts that rule findings *out*, and the full verbatim
intent lives behind a second tool the reviewer must deliberately pull.

---

## Install

**Supported platforms:** macOS (Apple Silicon and Intel), Linux (x86-64), and
Windows (x86-64). Every one of them is built AND executed in CI on every push,
and the full test suite runs on Linux, macOS, and Windows.

Two things differ on Windows, both stated rather than discovered:

- **No end-of-session nudge.** The Stop hook is a `/bin/sh` script, so it is not
  installed on Windows. Everything else `install` does works: the MCP server is
  registered and the debrief skill is placed. Run the debrief yourself instead
  of waiting to be prompted.
- **Installed files are not permission-restricted.** On Unix the installer
  chmods what it writes (0700 directories, 0600 data) so another account cannot
  read your Claude Code config. Windows has no mode bits, and the ACL
  equivalent would mean a new dependency, so files inherit whatever your user
  profile directory already grants. That is normally private to you, but it is
  not explicitly locked down the way the Unix install is.

Running inside WSL is also fine, and is the better option if you want the hook:
WSL is Linux, so use the Linux archive there.

Every archive contains **both** binaries, `sumcp` and `sumcp-mcp`. `install`
registers the MCP server by looking for `sumcp-mcp` as a sibling of `sumcp`, so
keep the two together. Installing one without the other leaves a broken
registration.

### From a release archive

Download for your platform from the
[latest release](https://github.com/rbh227/suMCP/releases/latest):

```bash
# macOS, Apple Silicon. Swap the target for x86_64-apple-darwin (Intel Mac),
# x86_64-unknown-linux-gnu (Linux), or x86_64-pc-windows-msvc (Windows).
V=v0.2.0; T=aarch64-apple-darwin
curl -LO https://github.com/rbh227/suMCP/releases/download/$V/sumcp-$V-$T.tar.gz
curl -LO https://github.com/rbh227/suMCP/releases/download/$V/sumcp-$V-$T.tar.gz.sha256

# Verify before running it. Expect "sumcp-...tar.gz: OK".
shasum -a 256 -c sumcp-$V-$T.tar.gz.sha256   # Linux: sha256sum -c

tar -xzf sumcp-$V-$T.tar.gz && cd sumcp-$V-$T
./sumcp install          # dry-run: prints exactly what it will write
./sumcp install --apply  # register the MCP server, debrief skill, and Stop hook (Unix)
```

The review-context tools described on this page are in the v0.3 line: build
from source until a v0.3 archive is published.

**macOS signing.** The binaries carry only the ad-hoc signature Rust applies at
link time. That is enough for them to run, but it carries no developer identity,
so they are **not notarized**: this project has no Apple Developer ID. A `curl`
download is not quarantined and will just work. A browser download is
quarantined, and Gatekeeper will refuse it until you clear the attribute:

```bash
xattr -dr com.apple.quarantine sumcp sumcp-mcp
```

Verify what you got, rather than taking the above on trust:

```bash
codesign -dvv ./sumcp   # expect "Signature=adhoc" until a Developer ID is set up
```

**Both platforms are exercised before publishing.** Every archive's `sumcp` is
executed in CI and made to analyze a real fixture, including the Intel macOS
build, which is cross-compiled on an arm64 runner and run through Rosetta 2. A
binary that links but cannot start, or starts but cannot parse a transcript,
fails the build rather than reaching you.

Two Linux archives are published. `x86_64-unknown-linux-gnu` is the default.
If it fails with a `GLIBC_2.3x not found` error, which happens on older
distributions such as Ubuntu 22.04 and Debian 12, use
`x86_64-unknown-linux-musl` instead: it is statically linked and depends on no
system libc at all.

### From source

**Minimum Rust version:** 1.88. This is enforced by a CI job, not just
declared: the code uses let-chains, which stabilized in 1.88, so 1.87 fails to
compile. Building from source is also the path for `aarch64` Linux, which has
no published archive.

```bash
git clone https://github.com/rbh227/suMCP && cd suMCP
cargo build --release
./target/release/sumcp install          # dry-run: prints exactly what it will write
./target/release/sumcp install --apply  # register the MCP server, debrief skill, and Stop hook (Unix)
```

`install` writes only under `$HOME` (everything self-contained in
`~/.claude/sumcp/`), backs up any file it touches, and is fully reversible:

```bash
sumcp uninstall --apply   # removes exactly what install added; restores backups
```

Restart Claude Code so it picks up the new user-scope server. See
[docs/](docs/) for the write contract (ADR A8).

### Wiring the reviewing agent

Any MCP-capable reviewer can call the tools. For Codex, register the server in
`~/.codex/config.toml` (or a project's `.codex/config.toml`):

```toml
[mcp_servers.sumcp]
command = "/path/to/sumcp-mcp"
```

A reviewer with no MCP wiring at all uses the CLI instead and reads JSON:

```bash
sumcp context                       # review context for the current session
sumcp context --intent              # the full verbatim requests (large, on purpose)
sumcp context --range HEAD~3..HEAD  # assert the session overlaps those commits
```

`--range` is a scope guard, not a filter: if the analyzed session does not
overlap the window in which those commits were made, the command exits
non-zero and prints nothing to stdout, because context from the wrong session
is worse than none. When it does overlap, the payload still covers the whole
session and the stderr note says exactly that.

---

## Quickstart (the forensic layer)

Run it with no arguments in a project you have worked in. It finds that
project's most recent session and debriefs the **whole stretch of work** it
belongs to: every transcript in the same continuous sitting (resumed
sessions, `/clear`s, concurrent instances), merged into one report. The
payload discloses the grouping in a `work_unit` block so you can verify it.

```bash
cd ~/code/your-project
sumcp
```

It prints which session it picked to stderr, so `--json` and `--html` stay
pipeable. Add a flag to change the format, `--work-unit` to name the stretch
by one of its transcripts, or `--file` to analyze exactly one transcript:

```bash
sumcp --json                                  # the session_overview payload
sumcp --html > report.html                    # a self-contained HTML report

sumcp --work-unit <path/to/session.jsonl>     # the whole stretch containing that transcript
sumcp --file <path/to/session.jsonl>          # that single transcript only
```

`--file` stays single-transcript on purpose (an explicit path is an explicit
scope) and prints a stderr note when the transcript is part of a larger
stretch, so a partial report is never silent about being partial.

---

## The eight tools

All read-only; all return compact JSON evidence, never narration. The first
six are the forensic layer (payload `v: 2`); the last two are the
review-context layer (payload `v: 3`).

| Tool | What it returns |
|------|-----------------|
| `session_overview` | Totals, token economics, and top-3 struggle files. |
| `struggle_areas` | Ranked struggle files with class, edit count, per-category breakdown, the ranking rule that ordered them, and evidence-backed findings. |
| `file_story` | Chronological event story for one file (head + tail kept, middle elided). |
| `blind_spots` | Secrets-file touches, blind-write attempts, review-burden findings, and instant-accept outliers, with suppression status for heuristic metrics. |
| `context_health` | Cache hit ratio and token economics (informational). |
| `evidence` | Dereference a finding's `idxs` into the raw actions that prove them. |
| `review_context` | **Start here for review.** What was asked, what the human decided and the other options offered, what was left unfinished, what the agent claimed it did. Facts with citations, never judgments. |
| `session_intent` | The full verbatim human requests. Large by design, deliberately excluded from `review_context`, to be pulled only when a specific ambiguity survives the diff and the compact context. |

What `review_context` will not do, on purpose:

- It reports `options_not_chosen`, never "options rejected". A free-text
  answer like "SQLite with WAL" affirms an option it does not literally match,
  and the tool does not pretend to know the difference.
- Claims carry `window_interrupted`, so a summary the human cut off mid-turn
  is never mistaken for a completion claim.
- Harness notices ("API Error...", overload and spend-limit messages,
  "No response requested.") are excluded from claims by provenance markers.
  Driving an early build against a real session returned one of those as the
  first "claim", which is exactly the false positive this layer exists to
  remove, so the filter is regression-tested against each notice class.
- Turns whose origin cannot be attributed to the human (interrupt markers,
  slash-command echoes; 13% of textual turns in the measured corpus) are
  counted and disclosed, never quoted as intent.

---

## How it works

<p align="center">
  <img src="docs/assets/diagram-pipeline.svg" alt="session.jsonl to a deterministic Rust parser to a session graph to MCP tools to your agent, cited. No LLM, no network, read-only." width="760">
</p>

`locate → ingest → model → signals/context → rank → payloads`. suMCP parses
transcripts permissively (a bad line never fails a file), merges every
transcript in the work unit plus their subagent transcripts into one
totally-ordered timeline, then runs pure functions over it. The forensic layer
looks for edit-shape churn, rework, re-reads, failure loops, reverts, and
comprehension signals. The context layer extracts requests, decisions,
unfinished work, and claims. Every finding carries a **tier**, an
**exact-vs-heuristic** flag, a **confidence**, and the action indices that
prove it. See [docs/metrics.md](docs/metrics.md) for the reader-facing
catalog, or [docs/metrics-spec.md](docs/metrics-spec.md) for the authoritative
spec.

Counting is insured, not assumed: a deliberately naive second implementation
that shares no code with the extractor must agree exactly on every count, in
CI for fixtures and on demand for real data. The current extraction agrees
with its recounter across 625 real transcripts. This exists because a 3x
undercount once survived 271 green tests whose fixtures shared the code's own
blind spot.

---

## What is and is not established

Honesty about which claims have evidence behind them, by layer.

**The forensic layer (v0.2, released) is measured, and the measurement cuts
both ways.** On the author's own corpus (43 sessions, 552 file-sessions, one
project held out), files suMCP flagged for review were 8.9x more likely to
show renewed struggle in the next 3 sessions than unflagged edited files. The
same study put the ranking against one-line baselines and it lost: sorting
files by edit count and taking the top 3 did at least as well on relative
risk, precision, and miss rate at once, so the weighted score was deleted and
the shipped ordering is a four-key rule you can verify by hand. File class
survived measurement (a fifth fewer false alarms at identical recall); the
rest is a usability claim, not an accuracy claim. Method, tables, and caveats
in [the predictive-validity study](docs/validation/2026-07-22-predictive-validity.md)
and [the file-class note](docs/validation/2026-07-26-file-class-measurement.md).

**The review-context layer (v0.3, unreleased) is built and unvalidated, and
says so.** The claim that recorded context raises a reviewing agent's
precision is externally supported (ARCTIC, above) but not yet demonstrated
for this tool. The validation experiment is designed with kill criteria fixed
in advance, and its first honest result is about itself: a pilot of 20 blind
Codex reviews on this repository measured **1.65 findings per commit** (30%
of commits yield zero), where the initial power estimate had assumed 3 to 6.
At the measured yield, the naive two-arm design needs more commits than this
repository has, so the experiment is being redesigned around paired
per-finding adjudication and simulation-based power before any headline
number is produced. The 33 findings already collected are the blind arm of
the real experiment. Until that experiment reports, treat `review_context` as
plumbing with disclosed semantics, not as a proven precision improvement.

---

## Findings and roadmap

- **The consumer changed, and the goal inverted.** v0.1 and v0.2 targeted a
  human deciding what to read, and the honest lesson of their own validation
  was that "which files to look at" is a question a one-line rule answers as
  well as a product. v0.3 targets the reviewing agent, where the measured
  bottleneck is false findings, and where the transcript holds exactly the
  facts (scope, intent, decisions, incompleteness) whose absence produces
  them.
- **suMCP retrieves; the agent judges.** The layer that survived every
  redesign is deterministic extraction with citations. Every attempt at a
  judgment layer (weighted scores, heuristic "abandoned approach" detection)
  has either lost to a trivial baseline or been cut before shipping for
  false-positive risk. That is now a design rule rather than a lesson.
- **Precision discipline is enforced in the payload shape.** Capped lists with
  unconditional totals, elision reflected in `truncated`, coverage gaps
  disclosed at top level, quotes only from explicitly attributed human turns,
  and no field name that claims more than the extraction can prove.
- **The experiment gates the roadmap.** A project memory layer is designed
  (per-file index of decisions, dismissed findings, and history, serving both
  the reviewer and the builder) and deliberately unbuilt until the precision
  experiment says the context helps. Durable storage for an unproven signal
  would compound noise while looking authoritative.
- **Known dependencies and risks, stated.** The transcript format is
  undocumented and owned by Anthropic; any release can break the parser. The
  approach is absorbable by whoever owns the format. And single-author data
  underlies every number on this page, so external corpora remain the
  standing next step.
- **Two things differ on Windows**, both by design and both detailed under
  [Install](#install): no end-of-session nudge, and installed files are not
  permission-restricted.

---

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
