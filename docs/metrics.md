# Metrics

Every signal suMCP ships, what it means, and how far to trust it. This is the
reader-facing distillation of [docs/metrics-spec.md](metrics-spec.md); the spec
is the authoritative catalog and the research citations live there.

Each finding carries a **tier**, an **exact-vs-heuristic** flag, a
**confidence**, and the action indices that prove it. `Tier` is a data-
reliability label, not an importance ranking: T1 fields are stable across
Claude Code versions, T2 need edge-case handling, T3 would be unstable
(nothing shipped rests on T3 today). This table has one row per `FindingKind`
that ships in `crates/sumcp-core/src/model.rs` today; nothing here is aspirational.

| Metric | Tier | Exact or heuristic | What it detects | Known limits |
|--------|------|--------------------|-----------------|--------------|
| Churn | T1 | Exact | A file edited (Edit/Write) 2+ times | High churn can be legitimate iteration, not struggle. When a recent Read reported the file's line count, the finding's weight is scaled by relative churn (churned lines / file lines, clamped 0.5x-2x) instead of the raw count alone; that denominator can go stale once the file has since grown or shrunk. |
| Rework | T2 | Exact | A later edit whose patch hunk (from `toolUseResult.structuredPatch`) overlaps an earlier edit's hunk on the same file | Overlapping hunks can be a deliberate refinement of the same region, not confusion. Depends on the harness populating `structuredPatch`; an edit without hunk data can't be compared. |
| Re-read thrash | T1 | Exact | A file `Read` 3+ times in the session | Fires on read count alone. It does **not** require an edit to be interleaved between the reads, so a file legitimately re-read many times (e.g. cross-referenced while writing elsewhere) still counts as thrash. |
| Failure loops | T2 | Heuristic | 2+ failing Bash commands attributed to the same file via a four-step chain: file path in the command/error text, else the most recently edited file in the same lane within the last 5 actions, else unattributed (dropped) | Attribution confidence varies: a direct path match is High confidence, a proximity guess is Low (and counts at half weight in ranking). Only ever attributes to a file the session actually touched; never touches the real filesystem. |
| Blind-write attempts | T1 | Exact | An Edit/Write whose tool result errored with "File has not been read yet" | Reframed from the original spec's "blind write" metric: the harness blocks true blind writes before they land, so this counts the *attempt*, not a write that actually happened blind. This is also the only detector behind the ranking category named `fumbles` (see `crates/sumcp-core/src/score.rs`); there is no separate, broader "tool fumble" detector for generic bad-argument or malformed-call errors. |
| True revert | T2 | Exact | A later edit whose `new_string` exactly restores an earlier edit's `old_string`, same file, same lane | Rare in practice; high-signal when it fires. Computed by the detector but **not currently returned by any of the six MCP payloads** in v0 (`session_overview`, `struggle_areas`, `blind_spots`, `file_story`, `context_health`, `evidence`): it doesn't carry a ranking category, so `struggle_areas`/`rank()` drops it, and `blind_spots` only forwards blind-write, review-burden, and large-write-instant-accept findings. |
| Flip | T2 | Exact | A true revert where the user pushed back (matched against 8 hardcoded phrases like "no", "wrong", "revert") between the two edits **and** the agent gathered no new evidence (no Read or Bash) in between | Rare in practice; high-signal when it fires. The revert equality check is exact, but the flip-vs-plain-revert classification rests on a short, hand-picked pushback-word list, so it can miss unworded pushback or misread borderline phrasing. Reversing after a failing test or a fresh read is treated as healthy revision, not sycophancy, and correctly stays a True revert. Same exposure gap as True revert: not surfaced by any of the six MCP payloads today. |
| User corrected | T2 | Exact | An edit the harness marked `userModified: true` | Rare. Same exposure gap: computed, but not returned by any of the six MCP payloads today. |
| Opening move | T1 | Heuristic | Per task segment (the run of main-lane actions between two consecutive human messages, minimum 5 actions), whether a Read preceded the first Edit/Write in the opening 10 actions (read-first) or not (patch-first) | The human may have directed an immediate edit, so this is framed as heuristic, not a verdict, and cites the leading user message so the narrating agent can overrule it. Segments under 5 actions aren't classified. The raw per-segment finding is not exposed by any MCP payload; `session_overview` only exposes the session-wide roll-up `patch_first_segment_share` (share of classified segments that opened patch-first), not the individual findings or their evidence indices. |
| Action loop | T1 | Exact (always advisory) | 3+ consecutive byte-identical tool calls (same tool name and same full input hash) within one lane | Always emitted at `confidence: Low` by construction (ranking applies the low-confidence multiplier), because automated loop detectors are known to be false-positive-prone (their own authors abandoned them). Runs are scored per lane so an interleaved subagent call can't break or fabricate a main-lane run. |
| Review burden | T1 | Heuristic | Agent-written lines (summed `write_lines` from Edit/Write) between two consecutive human turns, flagged when they exceed the 200-400 line human code-review band | This is the comprehension layer's anchor and runs unconditionally, including under auto-accept, because that is exactly when nobody else is gating the writes. Framed strictly as risk ("this volume plausibly could not have been reviewed"), never a verdict that the human didn't read it, since the transcript can't see their editor. Spans files, so per-file detail needs a follow-up `evidence(idxs)` call. |
| Large-write-instant-accept | T2 | Heuristic | A single main-lane Edit/Write of 2000+ characters whose tool result came back within 3 seconds | A timestamp delta can't distinguish "read it fast" from "auto-accepted" from "stepped away," so this is suppressed **entirely** whenever the session ran under an auto-accept permission mode, rather than reported as a meaningless number (unlike review burden, which is never suppressed). Main-lane only: a subagent write has no human gating it, so the same write on a subagent lane produces no finding. Never reported as exact. |

## What "approval latency" is, precisely

The original metrics spec's "approval latency" (timestamp delta between an
Edit/Write proposal and its result) is not, itself, a finding kind. In the
shipped code it is the raw signal that feeds Large-write-instant-accept above,
and it surfaces separately as an active/suppressed status flag in the
`blind_spots` payload's `suppression.approval_latency` field, not as its own
row of evidence. Review burden is the layer's actual anchor; approval latency
is a corroborating, and more fragile, secondary signal.

## Ranking stays a transparent weighted count, never a score

Only six of the twelve finding kinds above feed the file ranking that
`struggle_areas` and `session_overview.top_struggles` return: churn, rework,
failure loops, re-read, blind-write attempts (category `fumbles`), and action
loops. Rank is `sum of (config weight x evidence count)` per category, per
file, always returned as a per-category `breakdown` alongside the `weights`
used, never collapsed into a single opaque number. It is never based on
session length. True revert, flip, user corrected, opening move, review
burden, and large-write-instant-accept are informational: they carry real
evidence but do not move a file's rank.

The default category weights are editorial, not literature-derived; only
their relative order (rework and blind-write attempts tied for highest, then
failure loops, then re-read, then churn lowest; action loops always
advisory) reflects the research summarized in
[docs/metrics-spec.md](metrics-spec.md). The decimals themselves are tuning
knobs, overridable via `~/.config/sumcp/config.toml`.
