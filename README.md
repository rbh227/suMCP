# suMCP

**Post-session forensics for Claude Code.** suMCP reads your Claude Code
session transcripts and tells a connected agent *what actually happened* —
which files you fought with, where work was silently reverted, where an edit
went in unverified — from **evidence**, not the agent's self-report. It's a
deterministic Rust MCP server: **no LLM, no network, read-only.**

> **Status: v0.1 pre-release.** Robust across many real projects; the *accuracy*
> of the struggle ranking is not yet systematically validated (see
> [Limitations](#limitations)). Not yet published to crates.io.

---

## Why

When you ask an agent "what did we struggle with this session?", it answers from
a lossy memory of its own context — or by re-reading the entire transcript,
which is enormous. suMCP extracts the same answer as a few hundred tokens of
structured, cited evidence.

On **15 real sessions across 6 project types** (Rust, Python/ML, TS/React,
prose, and more), the core debrief payload was **~150–290 tokens** against raw
transcripts of tens of thousands to ~1,000,000 tokens — a **median ~800×
reduction**.¹ Same answer, a fraction of the context.

<!-- TODO(T5.3): drop the HTML-report screenshot here. -->

¹ Measured as `session_overview` payload vs full transcript, `chars/3.5`. A
complete debrief that also reads `struggle_areas` + a few `evidence` calls is a
small multiple of that — still one to three orders of magnitude smaller than
re-reading the transcript.

---

## Install

Requires a stable Rust toolchain (`rustup`).

```bash
git clone https://github.com/rbh227/suMCP && cd suMCP
cargo build --release
./target/release/sumcp install          # dry-run: prints exactly what it will write
./target/release/sumcp install --apply  # register the MCP server, debrief skill, and Stop hook
```

`install` writes only under `$HOME` (everything self-contained in
`~/.claude/sumcp/`), backs up any file it touches, and is fully reversible:

```bash
sumcp uninstall --apply   # removes exactly what install added; restores backups
```

Restart Claude Code so it picks up the new user-scope server. See
[docs/](docs/) for the write contract (ADR A8).

---

## Quickstart

Instant debrief on any transcript, no server needed:

```bash
sumcp --file <path/to/session.jsonl>          # ranked struggle areas, human-readable
sumcp --file <path/to/session.jsonl> --json   # the session_overview payload
sumcp --file <path/to/session.jsonl> --html   # a self-contained HTML report
```

Once installed, at the end of a session the Stop hook nudges you to run the
**debrief skill**, which calls the tools below and narrates the result.

---

## The six tools

All read-only; all return compact JSON evidence, never narration.

| Tool | What it returns |
|------|-----------------|
| `session_overview` | Totals, token economics, and top-3 struggle files. **Start here.** |
| `struggle_areas` | Ranked struggle files with a per-category score breakdown, the weights used, and evidence-backed findings. |
| `file_story` | Chronological event story for one file (head + tail kept, middle elided). |
| `blind_spots` | Blind-write attempts and large-write-instant-accept outliers, with suppression status for heuristic metrics. |
| `context_health` | Cache hit ratio and token economics (informational). |
| `evidence` | Dereference a finding's `idxs` into the raw actions that prove them (≤10 actions, excerpts ≤600 chars). |

---

## How it works

`locate → ingest → model → signals → score → Report`. suMCP parses transcripts
permissively (a bad line never fails a file), merges any subagent transcripts
into one totally-ordered timeline, then runs pure functions that look for
edit-shape churn, rework, re-reads, failure loops, reverts, and comprehension
signals. Every finding carries a **tier**, an **exact-vs-heuristic** flag, a
**confidence**, and the action indices that prove it. See
[docs/metrics-spec.md](docs/metrics-spec.md).

---

## Limitations

Read these before trusting a ranking:

- **Accuracy not yet systematically validated.** The ranking is proven to *run*
  and *generalize* across many real projects, but whether its top-3 struggle
  files match what you *actually* struggled with has only been spot-checked, not
  measured. Treat the ranking as a strong hint, not ground truth, in v0.1.
- **Heuristic signals.** Several signals (e.g. approval latency, instant-accept)
  infer intent from edit shape and timing; they're labeled heuristic and are
  suppressed when the session ran under auto-accept.
- **Single-session only.** No cross-session/project memory yet (a planned v0.2
  direction).
- **Single-user validation.** v0.1 is validated on the author's own projects,
  not by external users. External validation is the top post-v0.1 item.

---

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
