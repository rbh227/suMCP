# sessionscope: Metrics Spec (research-condensed prompt)

Context for this session: we are building sessionscope, a deterministic (no-LLM) Rust MCP server that parses Claude Code session transcripts (`~/.claude/projects/<url-encoded-path>/<session-id>.jsonl`) into struggle, comprehension, and efficiency signals. This doc is the distilled research on what to measure. Treat it as the authoritative metrics spec. Do not invent metrics outside it without flagging them.

## Transcript schema facts (reverse-engineered, no official spec)

- One JSON event per line; `type` discriminator: `user`, `assistant`, `system`, `summary`, `attachment`.
- Common fields: `uuid`, `parentUuid`, `timestamp` (ISO 8601), `sessionId`, `cwd`, `gitBranch`, `version`, `permissionMode`.
- Assistant entries: `message.content[]` blocks (`text`, `thinking`, `tool_use`, `tool_result`), `message.model`, `message.id`, top-level `requestId`.
- Token usage in `message.usage`: `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens` (+ nested `cache_creation.ephemeral_5m/1h`).
- Tool errors: `is_error: true` on `tool_result` blocks (the reliable signal). Structured output in top-level `toolUseResult`, linked via `sourceToolUseID`.
- Interruptions: user content starting with `[Request interrupted by user`.
- Subagents: `isSidechain: true` + `agentId`; child transcripts are `agent-<agentId>.jsonl`; children have NO parent pointer, link via `toolUseResult.agentId` in the parent.
- Flags: `isMeta` (not real user input), `isApiErrorMessage`. `summary` lines use `leafUuid`, not `uuid`.

Parser rules (non-negotiable):
- Every field optional; unknown line types are data, not errors; never fail the file on one bad line.
- Dedup streaming duplicates by `requestId`/`message.id` (last wins) and resumed-session replays by `uuid`, or all counts inflate.
- Large tool outputs live in external files referenced by path; follow the reference.
- Reliability tiers: T1 stable (`type`, `uuid`/`parentUuid`, `timestamp`, `usage.*`, `tool_use.name/input`, `is_error`, `isSidechain`); T2 needs edge handling (`requestId`, `toolUseResult`, summaries, external files); T3 unstable (undocumented flags). Tag every metric with its tier. T1 field breaking = fire drill; T3 = routine.

## Research findings that shape scoring

- Session LENGTH is NOT a struggle signal (reverses sign once task difficulty is controlled). Never use it.
- Structure discriminates: gathering context before editing correlates strongly with success (read-before-edit ρ ≈ +0.68; edit-heavy openings ρ ≈ −0.78; validation effort ρ ≈ +0.50). (arXiv:2604.02547, 9,374 trajectories.)
- **Interactive caveat (2026-07-18):** the ρ figures come from autonomous runs. In interactive Claude Code the human may direct an immediate edit, so opening-move classification is computed per task segment, never whole-session-only, and validation share is informational-only (humans run tests outside the transcript).
- Premature editing is the #1 empirical failure mode (~63% of failed runs, IDE-Bench arXiv:2601.20886); thrashing/backtracking ~28%; context loss ~28%.
- "Coherence collapse": most capable-model failures edit the RIGHT location repeatedly and still fail — same-region re-edit count is a distinct failure signature (TRAJEVAL arXiv:2603.24631: dominant theme in 39.7% of edit-quality failures on SWE-bench Verified).
- Relative (size-normalized) churn predicts defects; absolute churn does not (Nagappan & Ball, ICSE 2005). Within-session transfer is by analogy — relative churn is a refinement field, never a standalone metric.
- Review burden: human defect detection collapses past ~200–400 LOC per review (~87% under 100 lines → ~28% over 1,000; SmartBear/Cisco, 3.2M LOC), and AI-assisted developers are overconfident about their code's security (Perry et al., CCS 2023). LOC-per-human-turn against this band is the best-grounded comprehension-debt operationalization — and unlike approval latency, it works under auto-accept.
- Stuck-in-loop is standard in the SWE-bench literature (≥3 consecutive identical tool+args; SEAlign arXiv:2503.18455), but SWE-agent's authors abandoned automated loop detectors over false positives — flags are advisory-only, require byte-identical calls.
- Localization dispersion: agents read ~22× more functions than needed (TRAJEVAL), but the baseline needs gold patches — without one, report the read:edit ratio informationally and defer outlier flagging to a cross-session personal baseline.
- Context rot: accuracy degrades well before the window fills (~50k tokens in), mid-context info is the blind spot. Track window fill. *(The specific ~50k figure is uncited — source it or soften; see `docs/research-provenance-audit.md`.)*
- Capitulation flips ("You're absolutely right") are string-detectable and meaningful after user pushback — but only reversal WITHOUT new evidence is sycophancy (FlipFlop arXiv:2311.08596); reversal after a failing test or new read is healthy revision.

_Citation provenance for the findings above was verified 2026-07-21 (full-text,
not abstract) — see `docs/research-provenance-audit.md`. All load-bearing arXiv
IDs and the Nagappan & Ball / Perry et al. references resolve and match; two
earlier abstract-level flags were false alarms and are cleared._

## Metric catalog (grouped, priority-marked)

Legend: [H] high value, [D] differentiating (no existing tool computes it), [P] partial/heuristic — ship with confidence caveat.

### A. Token & cost
1. Session totals: tokens, est. cost (sum `usage`, dedup by requestId) — baseline, must match ccusage ±few %.
2. [H] Cache-hit ratio: `cache_read / (input + cache_creation + cache_read)` — best single efficiency signal.
3. [H] Context-window fill over time per turn (approx from input + cache_read).
4. [H][D][P] Context waste: tokens read via Read/Grep whose content never reappears in later edits/references.
5. Cost per model (`message.model` grouping).
28. *(added 2026-07-18)* [P] Localization dispersion: distinct-files-read : distinct-files-edited, reported by `context_health()` **only when the session has edits**, informational-only in v0.1 (TRAJEVAL's ~22× baseline needs gold patches). Outlier flagging is a named seam for the v2 cross-session layer, where a personal baseline makes "unusually dispersed" meaningful.

### B. Edit & churn
6. Files touched, edit/write counts (baseline).
7. [H] Churn: repeat edits per file AND per region (group by `file_path` + `old_string` overlap) — coherence-collapse signature. *(2026-07-18)* carries a `relative_churn` field (churned lines / last-known file size from the most recent full Read, heuristic [P]) used for within-category weighting; raw count is the fallback when no denominator is known.
8. [H] Blind write: Edit/MultiEdit with no prior Read of that file — premature-editing, top failure mode.
9. [H] Read-before-edit share + opening-move classification — strongest validated predictors. *(2026-07-18, per interactive caveat)* computed **per task segment** (segments start at each substantive non-meta user message; only segments with ≥5 tool actions are classified), confidence Medium, leading user message cited as evidence so the narrating agent can overrule. Reports the paper's numeric forms as fields: edit-fraction of the segment's first 10 actions, and first-edit index. Session-level roll-up = share of patch-first segments.
10. [H] Large single-shot writes (size from tool input, no prior read/iteration).
11. [H][P] Reverted work: later edit restores earlier content (within-session only; true git reverts need git).

### C. Commands, tests, errors
12. [H] Tool error rate: `is_error` / total, broken down by tool.
13. [H] Validation share: Bash commands matching test/lint/build regexes as fraction of actions. *(2026-07-18)* **informational-only, never scored as struggle** — in interactive sessions humans run tests outside the transcript, so a low share is not evidence of low validation.
14. [H] Test-failure loops: repeated failing test invocations, attributed to the last-edited file.

### D. Attention & comprehension (the thesis layer)
15. [H][D] Approval latency: timestamp delta between agent proposal and user go-ahead; near-zero = approved unread. *(2026-07-18)* corroborating signal under #27, no longer the layer's headline.
16. [H][D] Large-write-then-instant-accept: #10 + #15 combined. *(2026-07-18)* corroborating signal under #27.
17. [D][P] Files the human never engaged with (transcript shows agent reads, not the human's editor — best-effort, state the limitation).
18. Human engagement depth: user turn length, question:command ratio (proxy, no LLM).
27. *(added 2026-07-18)* [H][D][P] **Review-burden ratio — the comprehension-layer anchor.** Lines written/edited by the agent (summed from Edit/Write tool inputs) between consecutive substantive user messages (`isMeta: false`), compared against the 200–400 LOC human review band (SmartBear/Cisco; Perry et al. for the overconfidence effect). Works under auto-accept, where approval latency (#15) is suppressed. Framed strictly as "review-burden risk" — a human plausibly could not have reviewed this volume — never a verdict that they didn't read. T1 fields, heuristic [P].

### E. Conversation dynamics
19. [H] Interruption count (`[Request interrupted by user` prefix).
20. [H] User pushback rate (negation/redirect keywords) and [H][D] capitulation flips: pushback → "you're absolutely right"-class response → reversal edit with no new evidence gathered between.
21. [H] Reasoning/action loops. *(2026-07-18, tightened to the literature's definition)* ≥3 **consecutive byte-identical** (tool name + full input) calls within one agent lane, per SEAlign/agentic-eval. **Advisory-only:** confidence Low (counts ×`low_confidence_factor` in ranking) — SWE-agent's authors abandoned loop detectors over false positives. Distinct from re-read thrash (our corpus-grounded per-file re-read count), which is renamed in payloads so it never masquerades as this metric.
22. Self-admitted errors (phrase match).

### F. Subagents
23. Subagent tree + per-subagent tokens/cost (via agentId linking).
24. Subagent context bloat: size of content returned to parent; bootstrap failures.

### G. Cross-session (v2, design the seam now)
25. [H][D] Per-file friction: aggregate churn/errors/failures per file across sessions = empirical "hard for the agent" map.
26. Rework: same files revisited across sessions; trends over time.

## NOT computable from transcript alone (never fake these)
True git reverts/commits/PRs (need git); tests passing in CI; whether the human actually opened a file (needs IDE); authoritative billing; precise tool wall-clock and accept/reject decisions (OTel is the better source — point users there rather than approximating silently).

## Build staging
1. Base layer: defensive parser + T1 metrics (1, 2, 6, 12, 19). Gate: token totals match ccusage on real sessions.
2. Struggle layer: 7, 8, 9, 13, 14, 20, 21. Output a per-session friction profile with evidence (action indices), never a single opaque score, never length-based.
3. Comprehension layer (headline features): 4, 15, 16, 17.
4. Cross-session (25, 26) once Reports persist to SQLite.

## Why these default weights (2026-07-18)

No study provides per-file struggle-category weights; the ρ values are
session-level correlations from different papers and are not commensurable.
Default weights are therefore **editorial by construction** — only their rank
ORDER is research-derived:

- `rework` (re-patch/coherence-collapse) and `fumble` (blind-write attempts ≈
  premature editing) rank highest: dominant failure themes (39.7% of
  edit-quality failures; 63% of failed runs respectively).
- `failure_loop` next: validation-linked, directly attributed.
- `thrash` (re-reads) lower: our own corpus observation, no external validation.
- `churn` lowest among scored: fires constantly, mostly benign iteration;
  `relative_churn` refines it when a denominator is known.
- action loops (#21): advisory, always ×`low_confidence_factor`.

The exact decimals are tuning knobs, not findings. Payloads echo the weights
used and their source; never present them as derived from the literature.

## Guardrails
- Every metric labeled: tier (T1-3) and exact vs heuristic.
- Every finding carries the action indices proving it.
- The tool returns evidence; the connected agent narrates. No LLM inside sessionscope, no telemetry, nothing leaves the machine.
- MCP responses hard-capped (~1-2k tokens), read-only hints set, defaults to current session/cwd.
