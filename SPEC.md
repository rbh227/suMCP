# suMCP — Specification (v0.1)

Post-session forensics MCP server for Claude Code. Parses session transcripts
(`~/.claude/projects/<url-encoded-path>/<session-id>.jsonl`) into a structured
session graph of deterministic behavioral signals, so a connected agent can give
the developer honest, evidence-based answers about what it actually did — for a
fraction of the tokens of re-reading the session.

**The agent tells you what it built; suMCP tells you what it actually did.**

The authoritative metrics catalog is `docs/metrics-spec.md`. This spec records
the decisions layered on top of it (all grilled and locked 2026-07-14/15), the
empirical amendments from validating against 155 real transcripts, and the
engineering contract.

---

## 1. Objective

- **Users:** developers running Claude Code who ship agent-written code they
  don't fully understand (comprehension debt), and the agent itself, which
  queries suMCP mid-conversation for ground truth about its own behavior.
- **Thesis:** the transcript is behavioral evidence — every edit, read, failure,
  and user pushback, ordered and timestamped. Parsing is deterministic and
  near-free. No LLM in the tool; the connected agent is the intelligence layer.
- **Honest value claim:** evidence survives compaction and self-report bias.
  The token-ratio claim (structured query vs re-reading the transcript,
  ~200:1 estimated on real 4–6 MB sessions) leads the README once measured.
- **Definition of done (v0.1):** near the end of a real session, a Stop-hook
  nudge triggers the debrief skill; Claude names the 3 files it struggled with
  and why, grounded in cited evidence, in under 500 tokens, without re-reading
  anything.

### Locked design decisions

| # | Decision | Resolution |
|---|----------|------------|
| 1 | Debrief timing | **In-session, Stop-hook nudge.** SessionEnd is dropped (no agent left to answer). Manual `/debrief` also works on any past session. |
| 2 | Subagents | **Full ingest, flat-merge by timestamp.** Every `Action` carries `agent_id`. Both layouts supported: `<session-uuid>/subagents/agent-*.jsonl` (2.1.x, has `sessionId` back-pointer) and legacy sibling `agent-<agentId>.jsonl` (linked via `toolUseResult.agentId`). |
| 3 | Revert/flip rules | Four Finding kinds: `rework` (patch-hunk overlap with earlier edit), `true_revert` (whitespace-normalized `new_string` == earlier `old_string`), `flip` (true revert + intervening user pushback and/or capitulation phrase, no new evidence gathered between), `user_corrected` (`userModified: true`). |
| 4 | Failure attribution | Four-step chain: stderr/stdout paths → command-string paths → last-edited-within-5-actions → unattributed. Confidence `high/medium/low` stored on the Finding; low counts ×0.5 in ranking; unattributed never pinned to a file. |
| 5 | v0.1 scope | Metrics-spec staging **L1 + L2**, plus **#15 approval latency** and **#16 large-write-then-instant-accept** pulled forward from L3 (they are the comprehension-debt thesis and are cheap timestamp deltas). Deferred: #4 context waste, #17 human-engagement (v0.2); L4 cross-session (v2, seam only). |
| 6 | Ranking | **Transparent weighted count.** Rank = Σ(config weight × evidence count) per Tier-1 category. Weights in config with documented defaults; payload always shows the per-category breakdown. Never a single opaque score; never session-length-based. |
| 7 | MCP tools | **Six** (five + `evidence`). See §2. |
| 8 | Name | **suMCP.** Binary names `sumcp` (CLI) and `sumcp-mcp` (server). |

### Empirical amendments to docs/metrics-spec.md (validated on this machine's corpus)

1. **Metric #8 (blind write) cannot fire as written** — the Claude Code harness
   enforces read-before-edit. Reframed as **blind-write attempts**: count
   `tool_use_error` results like `"File has not been read yet"`. Observed live
   (2–4 per struggling session).
2. **Usage dedup nuance:** streaming writes one assistant response as multiple
   lines sharing `requestId`/`message.id` (82–440 duplicate lines per large
   file observed) but with **unique `uuid`s and distinct content blocks**.
   Dedup applies to `usage` accounting (last-wins per `message.id`), **not** to
   line/tool_use extraction — dropping lines wholesale loses tool calls.
3. **Empirical priors:** churn, rework, re-read thrash, failure loops, and tool
   fumbles fired strongly across real sessions; `true_revert`/`flip`/
   `user_corrected` fired zero times in five large sessions. Detectors stay
   (rare ⇒ high-signal when they fire) but ranking and demos must not depend on
   them.
4. **Additional live schema types** beyond the metrics doc: `mode`,
   `last-prompt`, `ai-title`, `file-history-snapshot`, `attachment`. Parser
   treats all unknown types as data (already a parser rule); fixtures cover
   versions 2.1.56 → 2.1.183.

## 2. MCP tools (all read-only, hard payload caps, default to current session/cwd)

| Tool | Returns | Cap |
|------|---------|-----|
| `session_overview()` | duration, files touched, edit/command/token totals (deduped), cache-hit ratio, opening-move class, top-3 struggle files with category breakdown | ~1k tokens |
| `struggle_areas(n)` | ranked files, per-category evidence counts + weights used, Finding idxs | ~1.5k tokens |
| `file_story(path)` | chronological events for one file (who: main/agent_id, what, outcome), truncated middle-out | ~1.5k tokens |
| `blind_spots()` | blind-write attempts; files written but never re-read by any agent; approval-latency outliers; large-write-then-instant-accept incidents | ~1k tokens |
| `context_health()` | window-fill over time, cache-hit ratio, files-read-never-referenced (informational, no "waste" judgment in v0.1) | ~1k tokens |
| `evidence(idxs)` | raw actions behind any Finding, ≤10 actions, ≤150 tokens each | ~1.5k tokens |

Every Finding carries: kind, tier (T1–T3 field reliability), exact-vs-heuristic
flag, confidence, and the action `Idx`s proving it. The tool returns evidence;
the agent narrates.

Deliverables besides the server: a **debrief skill** (the end-of-session ritual,
<500-token output contract) and a **Stop hook** that nudges when the session had
enough activity to warrant a debrief.

Adoption packaging (added by the idea-refine pass, see `docs/ideas/sumcp.md`):
`sumcp` run bare prints the latest session's debrief with zero config (the
first-60-seconds experience), and `sumcp report --html` renders the same
Report into one self-contained HTML file (the shareable screenshot). Both are
views of the single `Report` type — no live dashboard, ever (viewers are the
crowded non-moat category).

## 3. Commands

```bash
cargo build --workspace          # build everything
cargo test --workspace           # unit + fixture snapshot tests
cargo clippy --workspace -- -D warnings
cargo fmt --all
cargo run -p sumcp-cli -- overview [--session <id>] [--project <path>]  # human CLI
cargo run -p sumcp-cli -- install    # register MCP server + skill + hook (prints every write first)
cargo run -p sumcp-mcp           # MCP server over stdio
python3 scripts/sanitize.py <transcript.jsonl> <out.jsonl>  # fixture sanitizer (dev only)
```

Toolchain: stable Rust via rustup (not yet installed on this machine — first
build task), latest edition.

## 4. Project structure

```
crates/
  sumcp-core/        # library: locate → ingest → model → signals → score → Report
    src/locate.rs    #   find transcript(s) for cwd/session, incl. subagent files
    src/ingest.rs    #   permissive streaming parse, dedup, unknown-type counters
    src/model.rs     #   Session, Action (monotonic Idx, agent_id), ordering first-class
    src/signals/     #   pure fns &Session -> Vec<Finding>, one module per group
    src/score.rs     #   transparent weighted ranking; Weights config struct
    src/report.rs    #   Report + per-tool payload shaping/truncation
  sumcp-cli/         # thin binary: human-readable output of the same Reports
  sumcp-mcp/         # thin binary: MCP over stdio, 6 tools, read-only hints
fixtures/            # sanitized real transcripts, version-stamped (2.1.56, 2.1.183, …)
scripts/sanitize.py  # dev-time fixture sanitizer (structure-preserving, reviewed by hand)
docs/metrics-spec.md # authoritative metric catalog (amended per §1)
skills/debrief/      # the end-of-session debrief skill (installed by `sumcp install`)
hooks/               # Stop-hook nudge script (installed by `sumcp install`)
```

Seams left open, not built: symbol mapping (tree-sitter), git join (git2),
persistence (SQLite, for L4 cross-session), incremental ingest (byte-offset
param exists from day one, used later for real-time tailing).

## 5. Code style

- Dependency budget: `serde`/`serde_json` (core), `clap` (CLI only), `rmcp`
  pinned with `features = ["server"]` (MCP binary only — tokio comes with it
  and stays confined there), `toml` + `dirs` (config). Justify anything beyond
  this list. `sumcp-core` depends on serde/serde_json alone.
- Parsing paths never panic: no `unwrap`/`expect` on transcript data; one bad
  line never fails a file; unknown fields/types increment counters, never error.
- Signals are pure functions `&Session -> Vec<Finding>`; no I/O below `ingest`.
- Weights and thresholds live in a `Weights`/config struct — nothing tunable is
  hardcoded.
- Every metric tagged in code with its reliability tier (T1–T3) and
  exact-vs-heuristic status, mirrored in payloads.

## 6. Testing strategy

- **Fixture corpus:** sanitized real transcripts across harness versions,
  including one with subagents and one with streaming duplicates. Snapshot
  tests: fixtures → full Report JSON.
- **Gate tests (from metrics-spec staging):** token totals match `ccusage`
  within a few % on real sessions; dedup invariants (usage counted once per
  `message.id`; no tool_use lost).
- **Signal unit tests:** hand-built minimal Sessions per Finding kind, incl.
  the zero-fire cases (revert/flip on a calm session ⇒ empty).
- **Payload tests:** every tool response under its token cap on the largest
  fixture.
- **Validation gates already passed:** gate 1 (signals fire and surprise on
  real data — passed 2026-07-14, findings in §1). Gate 2 (token ratio) —
  measure on fixtures and put the number in the README before v0.1 ships.

## 7. Boundaries

**Always**
- Everything stays on the machine: no LLM inside the tool, no telemetry, no
  network calls.
- Return evidence, let the agent narrate; every claim traceable to action idxs.
- Label every metric exact-vs-heuristic; attribution below high confidence is
  labeled and down-weighted.

**Ask first**
- New dependencies beyond the budget in §5.
- Any write path (v0.1 is read-only end to end).
- Publishing (crates.io, GitHub) or changing the six-tool MCP surface.

**Never**
- Fake non-computable metrics (git reverts, CI results, whether a human opened
  a file, authoritative billing — see metrics-spec "NOT computable").
- Output a single opaque struggle score, or use session length as a signal.
- Let the tool editorialize ("you did badly") — evidence only.

## 8. Architecture decisions (ADR summary, grilled 2026-07-15)

| # | Decision | Why |
|---|----------|-----|
| A1 | **Rust**, despite no local toolchain yet and a permissive-parsing workload | Distribution: a single static binary with zero runtime deps is a real adoption edge for an MCP server. Secondary: learning Rust is an explicit goal — accept slower iteration; put the learning value in `sumcp-core`, not plumbing. |
| A2 | **rmcp** (official SDK, pinned) over hand-rolled JSON-RPC | Protocol correctness outsourced; MCP is still evolving. Tokio confined to the `sumcp-mcp` binary; core stays sync/pure; SDK wrapped thinly so swapping it later is one crate. |
| A3 | **Memoized re-parse** for freshness | Transcript grows for hours under a long-lived server. Stat on each call; re-parse (~tens of ms for 6 MB) only if (mtime, size) changed. Always fresh, no mutable-model bugs. `byte_offset` param exists but is always 0 in v0.1 (real-time seam). |
| A4 | **Self-identifying current session** | The calling session has already appended this very tool_use to its own transcript — scan recent files' tails for our tool_use id. Immune to concurrent sessions in one project (the mtime heuristic silently analyzes the wrong session — fatal for an honesty tool). Fallback: newest-mtime, labeled "inferred by recency" in the payload. Explicit `session_id` param everywhere. |
| A5 | **Compact JSON payloads** | Agents parse it reliably, snapshot tests diff it, token caps enforceable by construction (`truncated: true` markers). CLI renders the same `Report` for humans — one Report type, two views. |
| A6 | **Compiled default Weights + optional TOML** (`~/.config/sumcp/config.toml`) | Zero-setup adoption; payloads echo the weights used (transparency guardrail). |
| A7 | **Sanitizer script + hand review for fixtures** | Real transcripts contain private code/prompts/paths. Structure-preserving rewrite (ids, ordering, usage, error shapes kept; content synthesized) keeps the repo publishable without losing the weirdness that breaks parsers. |
| A8 | **`sumcp install` subcommand** | Registers the server, copies skill + hook, printing every write before making it (+ `uninstall`). On-thesis for the distribution motive. Plugin packaging deferred to v0.2. |
