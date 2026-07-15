# suMCP v0.1 — Build Plan

## Context

suMCP is a deterministic (no-LLM) Rust MCP server that parses Claude Code
session transcripts (`~/.claude/projects/**/*.jsonl`) into struggle,
comprehension, and efficiency signals, so a connected agent can answer "what
did you actually do?" from evidence instead of self-report. The full contract
was specced and grilled over the last two sessions:

- `SPEC.md` — engineering contract: workspace layout, six read-only MCP tools,
  L1+L2 metrics + approval latency, ADRs A1–A8 (Rust; rmcp pinned; memoized
  re-parse; session self-identification; compact JSON; TOML-optional weights;
  sanitized fixtures; `sumcp install`).
- `docs/metrics-spec.md` — authoritative metric catalog (with three empirical
  amendments recorded in SPEC §1).
- `docs/ideas/sumcp.md` — product one-pager: success = strangers install it;
  v0.1 adds bare-CLI instant debrief + static HTML report; no live dashboard.

The repo has no code yet, and **no Rust toolchain is installed**. This plan
sequences v0.1 so the riskiest assumptions are tested before the expensive
work, and every task is a vertical slice with its own verification.

First execution step: copy this plan into `tasks/plan.md` and generate
`tasks/todo.md` from the task list (per the /agent-skills:plan contract).

**Open-source is a first-class requirement** (success metric = strangers
install it). Documentation is therefore an acceptance criterion inside every
task — public items get rustdoc as they're written, every metric ships with
its tier/heuristic label documented, and each ADR-bearing change lands with
its explanation (the comprehension rule). Phase 5 adds a dedicated
OSS-readiness task (T5.4); docs are never a trailing cleanup pass.

## Dependency graph

```
Report JSON contract ──► debrief skill vs mock payloads   (no Rust needed)
        │
rustup + workspace scaffold
        │
sanitizer script ──► fixture corpus (3+ versions, subagents, streaming dups)
        │                    │
        └──► locate + ingest + model  ──► session_overview ──► bare `sumcp` CLI
                                │
                     signal modules (churn/rework/thrash → failures → 
                     reverts/flips/opening-move → approval-latency)
                                │
                     score + full Report
                        │              │
                 MCP server (rmcp)   HTML report
                        │
        debrief skill (real) + Stop hook + `sumcp install`
                        │
        external validation (other people's transcripts) + token-ratio → README
```

## Phase 0 — Contract validation (no Rust; ~1 day)

**T0.1 Freeze the Report/payload JSON shapes.**
Design the compact-JSON response for all six tools (`session_overview`,
`struggle_areas`, `file_story`, `blind_spots`, `context_health`, `evidence`)
as `docs/payload-schema.md` + example payloads in `fixtures/mock-payloads/`.
Derive realistic numbers from the gate-script findings already computed on the
example-app corpus (churn 24× DataStore.ts, 6/10 attributable failures, etc.).
- Accept: every example payload ≤ its SPEC §2 token cap (measure with a
  tokenizer approximation: chars/4); every Finding carries kind, tier,
  exact-vs-heuristic, confidence, idxs.

**T0.2 Debrief skill against mocks (dealbreaker test).**
Write `skills/debrief/SKILL.md` that reads mock payloads and produces the
end-of-session debrief. Run it in a live Claude Code session against the mocks.
- Accept: output names 3 struggle files with cited evidence in <500 tokens and
  reads as true/useful. If narration needs different payload shapes, iterate
  T0.1 now — this is why Phase 0 exists.

**CHECKPOINT A:** payload schema frozen at v0; narration contract proven.
Do not start Rust until this passes.

## Phase 1 — Foundation (~1 day)

**T1.1 Toolchain + workspace scaffold.**
Install rustup (stable). Create the workspace per SPEC §4: `crates/sumcp-core`
(deps: serde, serde_json only), `crates/sumcp-cli` (clap), `crates/sumcp-mcp`
(rmcp pinned, `features=["server"]`). Empty lib/bins that compile; `cargo fmt`,
`clippy -D warnings`, `cargo test` all green. Commit hygiene starts here
(git repo already initialized; first commit includes existing docs).
- Accept: `cargo test --workspace` passes; `sumcp-core` has no async deps.

**T1.2 Sanitizer + fixture corpus.**
Write `scripts/sanitize.py` (structure-preserving: keep ids, ordering, usage,
error shapes, timestamps; synthesize paths/strings/prompts). Produce fixtures
from the local corpus: one 2.1.56 session, one 2.1.183 session, one with
subagents (`aaaa1111…/subagents/`), one with heavy streaming duplicates, one
tiny hand-built edge-case file (bad lines, unknown types).
- Accept: hand review confirms no private content; each fixture documented in
  `fixtures/README.md` with version + what it exercises.

## Phase 2 — First vertical slice: parse → overview → CLI (~2–3 days)

**T2.1 locate + ingest + model + `session_overview` + bare CLI.**
`locate.rs` (cwd→project dir mapping, session enumeration, subagent discovery
for both layouts), `ingest.rs` (permissive streaming parse; never fail a file;
unknown-type counters; THREE dedup layers per SPEC §1 amendment 2: usage
last-wins per `message.id`, action extraction deduped by line `uuid` +
`tool_use` id against resumed-session replays, streaming content preserved;
external tool-output file references followed), `model.rs` (`Session`, ordered
`Vec<Action>` with monotonic `Idx`, `agent_id`; subagent merge under the total
ordering contract: sort key `(timestamp, agent lane [main first], line
number)`, equal-timestamp cross-agent pairs marked order-uncertain and
excluded from strict before/after findings), minimal `report.rs` for overview
counts, and `sumcp` bare printing the overview table.
- Accept (gate from metrics-spec staging): token totals match `ccusage` within
  a few % on a real local session; all three dedup layers unit-tested incl. a
  resumed-replay fixture (token totals AND action counts snapshotted); an
  identical-timestamp subagent fixture yields stable `Idx` values run-to-run;
  snapshot test fixture→Report JSON; one bad line ≠ failed file; unknown types
  counted.

**CHECKPOINT B:** parse gate passed on all fixtures + one live session.

## Phase 3 — Signals (~3–5 days; each task = pure fns + unit tests incl. zero-fire cases)

**T3.1 Edit-shape signals:** churn per file, rework (structuredPatch hunk
overlap), re-read thrash, large single-shot writes, blind-write *attempts*
(harness `tool_use_error`).
**T3.2 Failure signals:** tool error rates by tool, validation share,
failure loops with the 4-step attribution chain (stderr paths → command paths
→ last-edit-within-5 → unattributed) + confidence tiers, same-command failure
chains.
**T3.3 Dynamics signals:** true_revert / flip (incl. capitulation-phrase
match) / user_corrected, opening-move classification, read-before-edit share,
interruptions, pushback keywords, n-gram action-loop repetition.
**T3.4 Comprehension signals (explicitly heuristic [P]):** approval latency
= delta from assistant `tool_use` proposal to its `tool_result`, measured only
for Edit/Write (execution ≈ instant, so delta ≈ human decision time);
suppressed entirely when `permissionMode` grants auto-accept or no permission
event can exist; never labeled exact. Large-write-then-instant-accept built on
the same suppression rules; manual validation fixtures.
**T3.5 Score + full Report:** `Weights` struct (compiled defaults + optional
`~/.config/sumcp/config.toml`), transparent weighted ranking with per-category
breakdown, low-confidence ×0.5, all six tool payload builders with caps +
`truncated:true` markers.
- Accept per task: unit tests on hand-built minimal Sessions; zero-fire tests
  (calm fixture ⇒ no revert/flip findings); T3.5 cross-validates against the
  Python gate-script numbers on the same source session (counts must match).

**CHECKPOINT C:** full Report on fixtures reproduces gate-1 findings; weights
echoed in payloads.

## Phase 4 — MCP server (~2 days)

**T4.1 rmcp stdio server.** Six tools wired to `sumcp-core`, `readOnlyHint`
annotations, memoized re-parse on (mtime,size), session self-identification
FAIL-CLOSED per ADR A4: verified own-tool_use-id match (bounded retry for
flush delay) or explicit `session_id`, else an `ambiguous_session` error
payload listing candidates — never a recency guess. Newest-mtime lives only
in the CLI `latest` mode with a provenance field.
- Accept: registered in this repo's `.mcp.json`; in a live session every tool
  answers under its cap; `evidence(idxs)` dereferences a Finding from
  `struggle_areas`; self-identification picks the right session with two
  concurrent sessions open on the same project (manual test).

**CHECKPOINT D — the DoD test:** in a real session, the debrief skill (now on
live tools, not mocks) names 3 struggle files with evidence, <500 tokens,
no transcript re-reading.

## Phase 5 — Packaging & external validation (~2–3 days)

**T5.1 Static HTML report.** `sumcp report --html`: one self-contained file
(inline CSS/JS, no framework) rendering the same Report — struggle table,
churn timeline, evidence appendix.
- Accept: opens from `file://`, screenshot-worthy, zero network requests.

**T5.2 Install story (the sole write path, ADR A8 contract).**
`sumcp install`/`uninstall`: dry-run by default, `--apply` to execute; atomic
temp+rename writes with timestamped backups of pre-existing files; rollback of
completed steps on partial failure; idempotent reinstall; manifest-tracked
uninstall that restores backups and removes only what install created. Stop
hook nudges only when session had ≥N edits.
- Accept: fresh `~/.claude` install works end-to-end AND tests cover
  pre-existing `.mcp.json`/skills/hooks, repeated install, simulated partial
  failure with rollback, and uninstall-after-manual-edits.

**T5.3 External validation + README (dealbreaker + gate 2).**
Run signals on 2–3 volunteers' transcripts (sanitizer offered for privacy);
interview: do the top-3 struggle files feel true? Measure the token ratio
(structured debrief tokens vs transcript-re-read tokens) on real fixtures.
README leads with the measured number + HTML-report screenshot; document
metrics with tier/heuristic labels.
- Accept (release gate, no escape hatch): ≥2 external "feels true"
  confirmations required to tag. If weights/signals are adjusted in response
  to feedback, the gate re-runs on held-out transcripts (OS/version/project
  diversity) — tuning never substitutes for passing. Measured token ratio in
  README; `v0.1.0` tag only after the gate passes. Publishing to
  GitHub/crates.io is an ask-first boundary per SPEC §7.

**T5.4 OSS readiness.**
LICENSE (MIT or Apache-2.0/MIT dual — decide at task time), README as the
front door (what/why in 3 sentences, install one-liner, measured token ratio,
HTML-report screenshot, quickstart, tool reference table), `CONTRIBUTING.md`
(dev setup, fixture-donation flow via sanitizer, how to add a signal),
`CHANGELOG.md`, `docs/metrics.md` (every shipped metric: definition, tier,
exact-vs-heuristic, known limits — distilled from metrics-spec), rustdoc on
all public items (`#![warn(missing_docs)]` on sumcp-core), GitHub issue
templates incl. a "signal felt wrong" template that asks for a sanitized
fixture, `cargo doc` builds clean.
- Accept: a stranger can go from README to a working debrief without reading
  source; docs CI-checked (missing_docs + doc tests in `cargo test`).

## Verification (end-to-end)

1. `cargo test --workspace` green (snapshots, gates, zero-fire, payload caps).
2. Token-total gate vs `ccusage` on a live session.
3. Live DoD run (Checkpoint D) recorded in the README demo.
4. Concurrent-session self-identification manual test.
5. Fresh-home install test (T5.2).
6. External corpus run (T5.3) — the "strangers" assumption.

## Risks / watch items

- **Rust learning curve** (first Rust project + no toolchain yet): Phase 2 is
  deliberately one thin slice; expect it to take the longest per line.
- **rmcp pre-1.0 API churn**: pin exact version; wrap thinly (ADR A2).
- **Signals tuned on one corpus**: T5.3 is a release gate, not a nice-to-have.
- **`ccusage` availability** for the token gate: if absent, install via npx or
  substitute manual usage-sum verification on a small fixture.
