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
- Structure discriminates: gathering context before editing correlates strongly with success (read-before-edit ρ ≈ +0.68; edit-heavy openings ρ ≈ −0.78; validation effort ρ ≈ +0.50).
- Premature editing is the #1 empirical failure mode (~63% of failed runs); thrashing/backtracking ~28%; context loss ~28%.
- "Coherence collapse": most capable-model failures edit the RIGHT location repeatedly and still fail — same-region re-edit count is a distinct failure signature.
- Context rot: accuracy degrades well before the window fills (~50k tokens in), mid-context info is the blind spot. Track window fill.
- Capitulation flips ("You're absolutely right") are string-detectable and meaningful after user pushback.

## Metric catalog (grouped, priority-marked)

Legend: [H] high value, [D] differentiating (no existing tool computes it), [P] partial/heuristic — ship with confidence caveat.

### A. Token & cost
1. Session totals: tokens, est. cost (sum `usage`, dedup by requestId) — baseline, must match ccusage ±few %.
2. [H] Cache-hit ratio: `cache_read / (input + cache_creation + cache_read)` — best single efficiency signal.
3. [H] Context-window fill over time per turn (approx from input + cache_read).
4. [H][D][P] Context waste: tokens read via Read/Grep whose content never reappears in later edits/references.
5. Cost per model (`message.model` grouping).

### B. Edit & churn
6. Files touched, edit/write counts (baseline).
7. [H] Churn: repeat edits per file AND per region (group by `file_path` + `old_string` overlap) — coherence-collapse signature.
8. [H] Blind write: Edit/MultiEdit with no prior Read of that file — premature-editing, top failure mode.
9. [H] Read-before-edit share + opening-move classification (read-first vs patch-first in first ~10 tool calls) — strongest validated predictors.
10. [H] Large single-shot writes (size from tool input, no prior read/iteration).
11. [H][P] Reverted work: later edit restores earlier content (within-session only; true git reverts need git).

### C. Commands, tests, errors
12. [H] Tool error rate: `is_error` / total, broken down by tool.
13. [H] Validation share: Bash commands matching test/lint/build regexes as fraction of actions.
14. [H] Test-failure loops: repeated failing test invocations, attributed to the last-edited file.

### D. Attention & comprehension (the thesis layer)
15. [H][D] Approval latency: timestamp delta between agent proposal and user go-ahead; near-zero = approved unread.
16. [H][D] Large-write-then-instant-accept: #10 + #15 combined — the canonical comprehension-debt pattern.
17. [D][P] Files the human never engaged with (transcript shows agent reads, not the human's editor — best-effort, state the limitation).
18. Human engagement depth: user turn length, question:command ratio (proxy, no LLM).

### E. Conversation dynamics
19. [H] Interruption count (`[Request interrupted by user` prefix).
20. [H] User pushback rate (negation/redirect keywords) and [H][D] capitulation flips: pushback → "you're absolutely right"-class response → reversal edit with no new evidence gathered between.
21. [H] Reasoning/action loops: repeated identical greps/reads/tool sequences (n-gram repetition).
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

## Guardrails
- Every metric labeled: tier (T1-3) and exact vs heuristic.
- Every finding carries the action indices proving it.
- The tool returns evidence; the connected agent narrates. No LLM inside sessionscope, no telemetry, nothing leaves the machine.
- MCP responses hard-capped (~1-2k tokens), read-only hints set, defaults to current session/cwd.
