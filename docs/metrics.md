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
| Secrets file touched | T1 | Exact | A credentials or key file (`file_class::is_secrets`: `.env` and its suffixed variants, `.netrc`, `.pgpass`, `credentials`, SSH keypair files, `.pem`/`.key`/`.p12`/`.pfx`) was read, edited, or written | Zero-tolerance by design: the user's rule is that a secrets file should never be touched at all, so one occurrence solo-qualifies a file for review rather than needing a second finding, and it is surfaced through `blind_spots` rather than the ranking (the ranking puts `Config` in the last tier, which would bury exactly this). **No predictive validation**: this kind did not exist when the 2026-07-22/2026-07-26 corpus studies were measured, so unlike every other row above it carries no hit-rate evidence. It is a policy signal, not a measured one. |

## What "approval latency" is, precisely

The original metrics spec's "approval latency" (timestamp delta between an
Edit/Write proposal and its result) is not, itself, a finding kind. In the
shipped code it is the raw signal that feeds Large-write-instant-accept above,
and it surfaces separately as an active/suppressed status flag in the
`blind_spots` payload's `suppression.approval_latency` field, not as its own
row of evidence. Review burden is the layer's actual anchor; approval latency
is a corroborating, and more fragile, secondary signal.

## Ranking is a stated rule, never a score

Files are ordered by four keys, in this order:

1. **edited files before never-edited ones**, because a file with no change has
   nothing to review;
2. **file class**, code and web first, then notes, then docs, then config and
   other;
3. **edit count**, descending;
4. **path**, so the order is total and stable.

Every ranked entry carries its `class`, its `edits`, the per-category
`breakdown` of findings about it, and `ranking_rule`: the rule above as one
sentence, shipped with the order it produced. There is no score.

**Why there is no score.** Until 2026-07-26 rank was
`sum of (weight x evidence count)` per category. The 2026-07-22 study found
that ranking did not beat sorting files by edit count, and a follow-up pass that
fitted weights to maximize hits *with the outcomes in hand* gained at most 4
hits out of 39, assigning maximum weight to edit count regardless. A tuned sum
that cannot beat counting edits even while cheating is not worth its opacity, so
the sum, the `Weights` type, and its TOML override (ADR A6) were removed.

Findings still do all the explaining. Every kind above keeps its tier, its
exact-versus-heuristic flag, its confidence, and the action indices that prove
it. They explain and cite; they no longer vote.

## What each signal did on the measured corpus

From the tune split of the 2026-07-22 study, 552 (session, file) pairs with 39
strong-recurrence outcomes. These are occurrence counts and outcome rates, not
weights: nothing here is tuned or fed into the ordering.

| kind | pairs | outcomes | rate |
|---|---|---|---|
| churn | 242 | 33 | 0.136 |
| re_read | 97 | 21 | 0.216 |
| rework | 94 | 19 | 0.202 |
| blind_write_attempt | 24 | 0 | 0.000 |
| failure_loop | 4 | 2 | 0.500 |
| true_revert | 2 | 1 | 0.500 |

Three things follow, and no more than these:

- **`blind_write_attempt` was weighted joint-highest** on the strength of
  IDE-Bench's finding that premature editing appears in 63% of failed runs, and
  on this corpus it fired 24 times with **zero** outcomes. Read that precisely:
  zero in 24 rules out a large positive effect. It does not establish the signal
  is harmful, and it does not refute IDE-Bench, whose population is autonomous
  benchmark trajectories rather than interactive sessions.
- **`failure_loop` and `true_revert` are too rare here to characterise.** There
  were 58 confirmed failed commands across the tune sessions, a median of 1 per
  session. That is a property of this corpus, not of the detectors.
- **`re_read` had the best rate of the frequent kinds** while being weighted
  below both `rework` and `fumble`. The literature-derived weight ordering did
  not reproduce on the only corpus it has been measured against, which is part
  of why the weights are gone rather than adjusted.

## File class

`class` is a pure function of the path (`sumcp_core::file_class`): `code`,
`web`, `notes`, `docs`, `config`, or `other`, from a fixed table of extensions
and path patterns, with no filesystem access.

Only the **code versus docs-and-config** boundary rests on adequate data:
documentation was 192 of 552 pairs carrying 1 of 39 outcomes, config was 37
carrying none, code was 285 carrying 34. Every other tier relationship is a
declared judgment on thin cells: `notes` is 19 pairs with 3 outcomes and shows a
*higher* rate than code, too thin to promote it, and `web` is 7 pairs grouped
with code because web files are code-like. Full breakdown and caveats in
[docs/validation/2026-07-26-file-class-measurement.md](validation/2026-07-26-file-class-measurement.md).
