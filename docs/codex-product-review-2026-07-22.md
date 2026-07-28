# Codex product and engineering review — 2026-07-22

## Executive verdict

suMCP is not a weak project. It is a disciplined, well-tested forensic engine
built ahead of product validation and ahead of a completely reliable end-user
workflow.

The technical foundation is substantially stronger than most pre-release
open-source projects. The product thesis is real and differentiated. The
central ranking has not yet earned trust, and the advertised debrief workflow
has contract gaps that can prevent it from working as described.

The right characterization is: **a promising research-grade prototype, not yet
a dependable product**.

Release recommendation: **do not make a public v0.1 announcement yet**, but
continue the project. The remaining work is concentrated and tractable.

## Scorecard

| Area | Assessment |
|---|---:|
| Product thesis | 8/10 |
| Core architecture | 8/10 |
| Safety and privacy discipline | 8/10 |
| Automated implementation coverage | 8/10 |
| Metric validity | 3/10 |
| First-run experience | 4/10 |
| OSS and release readiness | 4/10 |
| Long-term potential | 8/10 |

## What the product should be

The strongest interpretation of suMCP is:

> A local flight recorder for coding agents that turns enormous, unstable
> transcripts into a small, inspectable body of evidence about how work
> happened and where a human should look more closely.

This is more defensible than another Claude transcript viewer. Session
browsing, HTML rendering, search, token analytics, and history management are
already crowded categories. suMCP's opportunity is the layer those products
largely do not own: **behavioral forensics with dereferenceable evidence**.

The current project mixes three products:

1. Where did the agent struggle?
2. What might the human not have reviewed?
3. How efficient was the session?

The second is the strongest user-facing outcome. "Review these files first,
for these observable reasons" is more valuable and easier to trust than "the
agent struggled with these files."

## Major strengths

### Architecture

The three-crate split is clean:

- `sumcp-core`: synchronous, deterministic parsing and analysis.
- `sumcp-mcp`: protocol and session-resolution wrapper.
- `sumcp-cli`: human output, HTML, and installation.

The `Session -> Findings -> Ranking -> Payload` design is testable and
extensible. The parser tolerates malformed lines and schema drift, preserves
unknown types, deduplicates replays, merges subagents, and keeps evidence
indices. Keeping the core dependency footprint close to `serde` and
`serde_json` is a strong decision.

### Safety

The installer is one of the strongest parts of the repository:

- Dry-run by default.
- Atomic writes.
- Manifest-based uninstall.
- Backup restoration.
- Symlink defenses.
- Drift-aware reinstall.
- Retryable failed uninstall.

Transcript handling also includes bounded reads, regular-file checks, path
containment, fail-closed session identification, excerpt redaction, and
closed-world/read-only MCP annotations.

### Test coverage

At review time, the latest `main` was `0cc8169`. A full
`cargo test --workspace` run passed 167 tests, including real stdio MCP calls,
installer round trips, fixture parsing, subagent merge, HTML, payload, and
redaction tests. The payload, narration, and sanitizer contract scripts also
passed.

This demonstrates that substantial portions work. The project is not an empty
or nonfunctional prototype.

### Writing and honesty

The tagline is strong:

> The agent tells you what it built. suMCP tells you what it actually did.

The README discloses the lack of systematic accuracy validation. The metric
catalog distinguishes data-tier reliability, exact counts, heuristics, and
confidence. The research provenance audit is thoughtful.

## Critical findings

### P0. The debrief workflow does not satisfy its own contract

The debrief skill says the Stop-hook nudge includes a session ID, but the hook
output includes only the edit count. Compare `skills/debrief/SKILL.md` with
`HOOK_TEMPLATE` in `crates/sumcp-cli/src/install.rs`.

This matters because no-argument session identification is documented as
opportunistic. Under concurrent sessions, the primary installed workflow can
immediately return `ambiguous_session`.

Additional inconsistencies:

- The skill never calls `blind_spots`, although its output contract requires
  blind spots and suppression state.
- The output requires a duration, but `session_overview` does not return one.
- It mentions files written and never reread, but that metric is not in the
  shipped `blind_spots` payload.
- Calls after the initial overview do not explicitly propagate the selected
  `session_id`.
- The mock narration checker checks a prewritten response against prewritten
  payloads; it does not test the real hook-to-skill-to-MCP sequence.

The hook also runs on Claude's Stop event, which occurs after responses rather
than only at a conceptual end of session. Once a session crosses the threshold,
the nudge can become repetitive.

### P0. The central ranking is not validated

The repository demonstrates that the parser runs, candidate signals are
research-informed, and payloads are smaller than transcripts. It does not
demonstrate that:

- The top-ranked file was genuinely where the agent struggled.
- The top three beat simple edit-count or failure-count baselines.
- Editorial weights improve accuracy.
- A low-ranked file is safe to ignore.
- Results transfer from autonomous benchmark trajectories to interactive
  Claude sessions.

Research provenance validates why a signal was worth trying. It does not
validate suMCP's thresholds, weights, implementation, or ranking.

Until there is a blinded, hand-labeled evaluation, the weighted ranking is a
hypothesis.

### P0. Advertised payload caps are not generally enforced

Only `evidence()` actually shrinks output based on serialized size.

- `struggle_areas(n)` accepts an effectively unbounded `n`.
- It caps findings per file but not the total 1,500-token payload.
- `blind_spots()` emits every matching finding and always reports
  `truncated: false`.
- `session_overview()` can include arbitrary path lengths and event maps.
- `context_health()` always reports `truncated: false`.

The existing test proves a small fixture fits; it does not prove the cap holds
for large or adversarial sessions.

Finding truncation is also biased by detector order. Several rework findings
can consume the per-file slots before failure-loop or action-loop evidence is
shown, even when those categories contributed to the breakdown.

### P0. Packaging and release gates are incomplete

`cargo package` rejects both `sumcp-cli` and `sumcp-mcp` because the path
dependency on `sumcp-core` lacks a version requirement.

There is also a distribution problem: installing `sumcp-cli` alone does not
install the sibling `sumcp-mcp` binary expected by `sumcp install`.

At review time:

- `cargo test --workspace` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` failed on a test-only
  nested `format!`.
- `cargo fmt --all -- --check` failed on the newly merged installer.
- GitHub Actions ran dependency auditing only, with no test, format, clippy,
  documentation, or OS matrix workflow.

For v0.1, prebuilt release archives containing both binaries are likely a
better primary distribution mechanism than three independently published
crates.

## Product and messaging weaknesses

### "Exact" does not mean the interpretation is correct

A churn count can be exact while its interpretation as struggle is wrong. A
reread count can be exact while the file was intentionally used as a reference.
Overlapping edits can be deliberate refinement.

Consider separating:

- `measurement: exact | inferred`
- `interpretation_confidence: high | medium | low`
- `schema_reliability: T1 | T2 | T3`

### The tool does judge

The README says the tool does not judge, but it labels behavior as struggle,
thrash, fumbles, and blind spots, applies editorial weights, and produces a
ranking.

A more accurate claim is:

> suMCP measures observable behavior deterministically and makes its
> interpretations and weighting visible.

### The token claim is overstated

"Same answer, a fraction of the context" is too strong. The payload is
deliberately lossy and answers a constrained family of forensic questions.

The headline reduction compares `session_overview` with the entire transcript,
while an actual debrief uses overview, struggle areas, blind spots, and perhaps
evidence. The reduction will probably remain impressive, but it should be
measured over the completed workflow.

### The first 60 seconds are not implemented

The product one-pager says bare `sumcp` should analyze the latest session, but
the executable exits with a usage error unless `--file` is supplied. Asking a
new user to locate an internal JSONL transcript path is not a compelling first
experience.

The design proposed `sumcp report --html`; the implementation uses
`sumcp --file ... --html`.

### Documentation has drifted

Examples found during review:

- T5.1 remains unchecked although HTML exists.
- T5.3 remains unchecked while the README publishes a token sweep.
- The payload schema contains finding kinds and fields that do not match the
  current Rust implementation.
- Some findings are computed but unreachable through the MCP tools.
- HTML accepts `weights` and ignores them.
- CLI always uses default weights while MCP loads configured weights, so the
  two surfaces can rank the same session differently.

The project has enough specifications that they are beginning to compete as
sources of truth.

## Recommended product direction

Do not add more signals yet.

Reframe the primary promise as:

> After an agent session, suMCP gives you an evidence-backed review queue:
> which files deserve human attention first, and why.

The ideal primary output is closer to:

1. Review `install.rs` first: large generated change, overlapping rewrites, and
   failed validation attempts.
2. Review `server.rs` second: repeatedly reread and edited after correction.
3. Lower concern: documentation-only churn with validation succeeding.

Keep the forensic engine and MCP layer, but lead with the zero-config CLI and
HTML report. MCP should be an advanced integration surface rather than the
prerequisite for understanding the product.

## Prioritized roadmap

### P0: Make the claimed workflow true

1. Carry `session_id` from Stop-hook input into the nudge.
2. Make the skill call `session_overview`, `struggle_areas`, and `blind_spots`
   with the same explicit session ID.
3. Return duration/span or remove it from the output contract.
4. Remove nonexistent categories from the skill.
5. Add an installed-system test covering hook input through final debrief.
6. Avoid Stop-hook spam after every response.
7. Enforce every payload cap by construction.
8. Fix format, all-target clippy, packaging, and CI.

### P1: Prove the ranking

Create a committed evaluation package:

- 20 to 30 sessions across several project types.
- Labels created before viewing suMCP output.
- File-level ordinal labels for agent difficulty, human review priority, final
  defect/risk, and surprising behavior.
- Top-1 and top-3 precision, recall, NDCG, and false-accusation rate.
- Comparisons against edit count, changed-line count, failed-command count,
  and most-touched-file baselines.
- Signal ablations.
- A held-out external set.

The weighted model must beat simple baselines or deliver meaningfully better
explanations.

### P2: Repair onboarding and distribution

- Bare `sumcp` analyzes the latest session.
- `sumcp report` writes and opens a named HTML file.
- Publish macOS and Linux release archives with both binaries and checksums.
- Clearly state supported platforms.
- Add a versioned parser/fixture compatibility matrix.
- Add a short real workflow demo.
- Give the timeline a legend, position scale, and meaningful tooltips.

The Win9x identity is distinctive, but the current screenshot reads more as
instrumentation than insight. Dense timelines and sanitized hashed filenames
make the value difficult to understand immediately.

### P3: OSS hardening

- CI on Linux and macOS: tests, fmt, all-target clippy, docs, payload scripts.
- Declare `rust-version` or check in a toolchain file.
- Add package metadata, versioned internal dependencies, and README/license
  inclusion.
- Document the adapter boundary and schema assumptions.
- Commit reproducible evaluation results.
- Reduce historical plans/specs exposed as current documentation.
- Publish a small roadmap and fixture-contribution path.

### P4: Expand only after validation

Then consider:

- Codex, OpenCode, and Cursor adapters.
- Cross-session personal baselines.
- Model and prompt comparisons.
- Final validation status and git-diff risk.
- A longitudinal "what should become an instruction?" report.

Do not build a cloud dashboard, team product, cross-session graph, or more
detectors before the core ranking is validated.

## Final judgment

The code is not the weak part.

The project is a **strong engine with strong engineering instincts, good
differentiation, an incomplete workflow, and insufficient product evidence**.

The next milestone should be:

> Five strangers can install it, run one command, receive the correct
> session's review queue, understand every accusation, and agree that its top
> three are more useful than "files with the most edits."

If that can be demonstrated, suMCP is a worthwhile open-source project. If the
ranking cannot outperform simple baselines, preserve the parser/evidence engine
and pivot from struggle detection to transparent session reporting and review
triage.

## Instructions for the Claude Code agent reading this review

Treat this as an external audit, not an implementation specification.

1. Verify each concrete finding against the current `HEAD`.
2. Produce a disposition table: confirmed, partly confirmed, already fixed, or
   rejected with a technical reason.
3. Separate findings into P0 release blockers, P1 validation work, P2
   onboarding/distribution, and later ideas.
4. Do not change code until the disposition and implementation sequence have
   been approved by the user.
5. Preserve existing uncommitted work.
6. For approved implementation work, add a failing test first and make one
   focused commit per independent fix.
