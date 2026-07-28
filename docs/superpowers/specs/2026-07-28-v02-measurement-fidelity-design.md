# Design: v0.2 measurement fidelity (work units and the recount gate)

**Status:** draft 2026-07-28, awaiting approval. First of four v0.2
sub-projects. The other three (exact evidence from `file-history`, detection
accuracy with an independent outcome label, and the remaining planned v2
surface) get their own specs and depend on this one.

**Why it is first:** detection accuracy cannot be measured until the unit of
analysis is right. Today one file's churn is split across several transcripts
and each fragment lands below the two-kind review floor, so the misses and the
undercount are the same defect seen from two directions.

Raphael is learning Rust: implementation must annotate new code heavily and
explain non-obvious constructs in plain language in comments.

---

## 1. What it does, and for whom

**What:** suMCP stops treating one transcript file as one session. It groups
the transcripts belonging to a single continuous stretch of work into a **work
unit**, merges them into one total-ordered timeline using the machinery that
already merges subagents, and reports on the whole. Alongside that, a second
independent implementation recounts every quantity from raw JSONL and must
agree exactly, forever, as a build gate.

**For whom:** the same two users as the rest of suMCP. The developer reading a
debrief currently sees a fraction of their work described as the whole of it.
The agent querying its own ground truth inherits the same distortion.

**Why now:** the defect was found by the author noticing that a report said
about 50 edits for a day he knew was far larger. It is reproducible and large.

---

## 2. The evidence this rests on

All measured 2026-07-27 and 2026-07-28 on this machine's own corpus (82 main
transcripts, 323 subagent transcripts), now archived at
`~/claude-corpus-archive` (2011 files, sha256-verified) so these numbers stay
reproducible past the 30-day `cleanupPeriodDays` window.

**The undercount is real and roughly 3x.** The stretch of work around
2026-07-21 in this project is 7 transcripts totalling 275 Edit/Write calls,
split `[88, 0, 0, 51, 49, 86, 1]`, and it crosses midnight. Today's reporting
unit shows at most 88 of those 275. Two of the seven have zero edits, so a
debrief landing on one reports nothing at all.

**The merge itself is not broken.** On session `e88269ab`, suMCP reports 205
edits plus 21 writes, and an independent recount gives 23 main-lane plus 203
subagent-lane file operations. 226 equals 226. Subagent flat-merge (T5.0) works
exactly as specified. The defect is purely the scope of what gets merged.

**Subagent work is not a rounding error.** Corpus-wide, main transcripts carry
1334 Edit and 341 Write calls; subagent transcripts carry 867 Edit and 310
Write. Roughly 41% of all file modification happens in subagent lanes, which is
why any future analysis that silently drops them is unusable.

**Grouping is insensitive to the threshold.** Of 76 consecutive same-project
transcript pairs, 11 overlap in time and 21 more begin under a minute after the
previous one ends. Grouping the 82 transcripts at several idle-gap thresholds:

| gap | pairs joined | work units | max sessions in one unit |
|---|---|---|---|
| 5 min | 45 | 37 | 8 |
| 15 min | 47 | 35 | 8 |
| 30 min | 48 | 34 | 8 |
| 60 min | 49 | 33 | 8 |
| 120 min | 51 | 31 | 8 |

Across a 24x range of threshold the unit count moves only from 37 to 31. There
is almost nothing to fit, which is the argument for a declared rule.

**No explicit continuation marker exists.** Zero of 82 transcripts open with a
compaction or continuation summary, and `preventedContinuation` is `false` in
all 401 occurrences. A time-and-project rule is the only option available.

**Overlaps are concurrent instances, not continuations.** 11 of 76 pairs
overlap, which is two Claude Code processes in one project at once. They must
merge as parallel lanes, never chain as a sequence. The extreme case in this
corpus is a unit of 8 transcripts on 2026-07-20 whose every join is an overlap,
with gaps of -63.9 to -13.5 minutes. This is almost certainly the 4-session
concurrency pileup recorded at Checkpoint D, and it is the natural fixture for
the lane-scoping regression test in §9.

---

## 3. Success criteria

1. A work unit spanning several transcripts produces a single report whose
   totals equal the sum of an independent per-file recount, exactly.
2. `scripts/recount.py` agrees with `sumcp --json` on every archived transcript
   and every work unit, for edits, writes, reads, bash calls, files touched,
   and token totals. Disagreement fails CI.
3. Overlapping transcripts merge as concurrent lanes in one deterministic total
   order, and no position-based finding (`true_revert`, `flip`, failure
   proximity attribution) ever compares across two different sessions' lanes.
4. Every finding resolves to its originating session id, so a work-unit report
   can be drilled back down to which session produced any given piece of
   evidence.
5. The report discloses its own grouping: how many sessions were joined and
   across what gaps, in a sentence the reader can verify by hand.
6. Analyzing a single transcript directly (`sumcp --file`) still works and still
   reports that transcript alone, unchanged.
7. Any work-unit-side failure degrades gracefully: an unreadable sibling
   transcript reduces the unit rather than failing the analysis, and is counted.
8. All 155 existing tests stay green, with payload-contract assertions updated
   in lockstep across `check_payloads.py` and `docs/payload-schema.md`.

---

## 4. The work unit

### 4.1 Definition

A **work unit** is a maximal set of transcripts in one project directory where
each transcript either overlaps the previous one in time, or begins within
`WORK_UNIT_IDLE_GAP` of the previous one's last timestamp.

`WORK_UNIT_IDLE_GAP = 30 minutes`. Declared, not fitted. The table in §2 is its
justification: any value from 5 to 120 minutes produces substantially the same
grouping, so the choice is a readability decision rather than a tuned
parameter. It is a `const` in the source with that table cited beside it, and
it is not user-configurable (ADR A6's TOML override was removed once already
for adding surface without adding value).

Grouping uses each transcript's first and last event timestamps. Transcripts
with no timestamped events at all cannot be placed in time and form their own
single-transcript unit rather than being silently attached to a neighbour.

### 4.2 Overlaps are lanes

Two transcripts that overlap in time are concurrent Claude Code instances. They
join the same work unit, but they are separate lanes, exactly as a subagent
transcript is a separate lane today. `merge_sessions` already total-orders
across lanes by `(timestamp, lane, line_no)`; this generalizes the lane
identity from `Main | Sub` to a lane that carries its originating session id.

The lane-scoping rule from the subagent merge design (§5 of the 2026-07-20
spec) carries over unchanged and is load-bearing here: comparisons that depend
on adjacency, meaning `true_revert`, `flip`, and failure-attribution proximity,
compare only within one lane. Without this, a main-lane edit in session A could
read as a revert of a main-lane edit in session B.

### 4.3 Disclosure

Every payload that reports on a work unit states the grouping in the same
auditable spirit as `ranking_rule`:

Real values, from the 275-edit unit in §2:

```json
"work_unit": {
  "rule": "same project; joined when a transcript overlaps the previous or starts within 30 min of its end",
  "sessions": 7,
  "joined_gaps_min": [0.1, 0.1, 0.2, 0.1, 0.8, 0.2],
  "span_start": "2026-07-20T11:05:41Z",
  "span_end": "2026-07-21T22:21:32Z",
  "session_ids": ["6ee0637b", "be82d193", "00e1235d", "14a11515",
                  "1731459f", "3145f2f3", "8c1adedb"]
}
```

A **negative** value in `joined_gaps_min` means that transcript overlapped the
running span rather than following it, which is how a reader tells a concurrent
instance from a continuation without needing a separate field.

Session ids are the short 8-character stems, not full paths, to keep the cap
budget intact and to avoid leaking the home directory (an existing constraint
in `server.rs`).

### 4.4 Bounding

A work unit of 8 transcripts, each with up to `MAX_SUBAGENT_FILES = 64`
subagent children, is up to 520 files of I/O. The observed maximum unit size is
8 transcripts, so:

- `MAX_WORK_UNIT_SESSIONS = 16`, comfortably above the observed maximum, and
  truncation is disclosed via a new honesty counter
  `flags.work_unit_sessions_dropped`, mirroring `subagent_files_missing`.
- The existing per-transcript byte cap (ADR A9(3)) applies per transcript and
  is unchanged.
- Total merged actions are already capped downstream by the payload shrink loop
  added in the v0.1 release-readiness pass, so no new payload cap is needed;
  what is needed is confirming that loop still terminates at work-unit scale,
  which is a test, not a code change.

### 4.5 Which unit each entry point uses

| entry point | unit | rationale |
|---|---|---|
| `sumcp` (bare, in a project) | work unit containing the newest transcript | matches what the user just finished doing |
| `sumcp --file X` | transcript X alone | explicit path means explicit scope; unchanged behaviour |
| `sumcp --work-unit X` | work unit containing X | new flag, for analyzing a past stretch |
| MCP tools, explicit `session_id` | work unit containing that session | the debrief wants the whole stretch |
| MCP tools, no `session_id` | unchanged fail-closed identification (ADR A4), then its work unit | identification semantics are untouched |

The MCP change is deliberate and is the one behavioural break for the debrief
skill: it now narrates a stretch of work rather than one transcript. The skill
text needs a matching edit so it says "this stretch of work" rather than "this
session," and reports the grouping.

---

## 5. Accuracy enforcement: the recount gate

### 5.1 What "100% accurate" means here

The `metrics-spec.md` list of quantities not computable from a transcript
stands unchanged. This is not a claim about those. For every quantity that *is*
countable from the transcript, the standard is: **suMCP's number equals an
independent recount, exactly.**

### 5.2 Why the current tests cannot enforce that

Every existing test asserts against fixtures produced by the same code path it
tests. A systematic scope error, which is exactly what this whole spec is
about, is invisible to that design: the code and the fixture agree because they
share the bug. Nothing in 155 green tests caught a 3x undercount.

### 5.3 The harness

`scripts/recount.py`, stdlib-only, matching the `sanitize.py` and
`validity_sweep.py` conventions. A deliberately naive second implementation:
walk raw JSONL, count `tool_use` blocks by name, dedup by `requestId` and
`uuid` per the parser rules, and emit totals. Then diff against `sumcp --json`
for every transcript and every work unit in `~/claude-corpus-archive`.

Deliberately naive matters. The value comes from the two implementations being
written differently, not from the second one being good. If the recount grows
to share helpers with the Rust code, it stops being independent and stops being
worth running.

Quantities covered: `edits`, `writes`, `reads`, `bash`, `file_ops`,
`files_touched`, and token totals. Not covered: anything requiring signal
detection, since a second implementation of the signal logic would be a
reimplementation rather than a check.

Runs against the archive, not against `~/.claude`, so it is reproducible and
does not shift under a live session. A CI job runs it against a small committed
fixture set; the full-archive run is a local command, since the archive is
private and must never be committed.

### 5.4 The known-good case it must reproduce

Session `e88269ab` must recount to 226 file operations and suMCP must report
226. This is a regression test for the check itself, since a harness that
agrees with everything is worthless.

---

## 6. Transcript events currently discarded

`unknown_event_types` on one real session showed eight types being counted and
thrown away. Two are adopted now, because they make an existing heuristic
exact; the rest are recorded here and deferred.

**Adopt: `mode`.** Carries the permission mode directly. suMCP currently infers
auto-accept in order to suppress the approval-latency and instant-accept
heuristics. Inference becomes an exact read, which changes which signals fire
and is therefore fidelity work rather than a new feature. Note the event
appeared 96 times in one session, so mode changes during a session and the
suppression decision must be per-action, not per-session. That is a real
correctness improvement over today's single session-level flag.

**Adopt: `origin` on user messages.** Distinguishes `{"kind": "human"}` (452
occurrences) from `{"kind": "task-notification"}` (101). The review-burden
metric counts lines written between substantive user messages, currently keyed
on `isMeta`. A task notification is not a human turn, and counting it as one
truncates the review-burden window and understates the metric.

**Defer to sub-project 2:** `file-history-snapshot` and `file-history-delta`.
These are pointers into `~/.claude/file-history/`, which holds real pre-edit
file contents (1118 files present). That is the foundation of the exact-content
evidence work and is too large to fold in here. This spec only requires that
the parser stop counting them as unknown, so the honesty counter reflects
genuinely unmodelled types.

**Not adopted:** `ai-title`, `last-prompt`, `attachment`, `queue-operation`,
`system`. Recorded as known-and-ignored so `unknown_event_types` means what it
says.

---

## 7. Payload contract: v1 to v2

The contract went v0 to v1 on 2026-07-26. This is v2. Changes:

- **Added** `work_unit` object (§4.3) to `session_overview`.
- **Added** `session` field on findings and on ranked entries, carrying the
  8-character originating session stem. This is the drill-down.
- **Changed** `session` object in `session_overview`: `id` becomes the work
  unit's newest session, and `started` becomes the unit's span start. Both
  documented as unit-scoped.
- **Added** `flags.work_unit_sessions_dropped`.
- **Unchanged**: every tool remains read-only, every finding still carries
  `idxs`, all caps still enforced by construction.

`docs/payload-schema.md`, `scripts/check_payloads.py`, and
`fixtures/mock-payloads/` all move in lockstep, as they did for v1.

---

## 8. Error handling

| case | behaviour |
|---|---|
| sibling transcript unreadable or oversized | excluded from the unit, counted in `work_unit_sessions_dropped`, analysis proceeds |
| transcript with no timestamps | forms its own single-transcript unit, never silently attached |
| unit exceeds `MAX_WORK_UNIT_SESSIONS` | keep the newest 16, disclose the drop |
| project directory unreadable | fall back to single-transcript analysis, warn on stderr |
| symlinked sibling transcript | refused, per the existing ADR A9 guard in `locate` |
| clock skew putting a later file earlier | ordering is by timestamp and is deterministic; grouping is unaffected because it uses interval overlap, not file order |

---

## 9. Testing

Red-first where the behaviour is new, per the process note in T4.1-verify.

1. **Grouping unit tests:** two overlapping transcripts join; a 29-minute gap
   joins and a 31-minute gap does not; a timestampless transcript stands alone;
   different projects never join.
2. **Merge tests:** a work unit of three transcripts produces one total order;
   `true_revert` does not fire across two sessions' main lanes (the key
   regression risk, and the direct analogue of the subagent lane-scoping test).
3. **Disclosure test:** `work_unit.joined_gaps_min` matches the actual gaps.
4. **Bounding test:** 20 transcripts in one unit truncate to 16 and disclose 4.
5. **Differential test:** `recount.py` against a committed multi-transcript
   fixture, asserting exact equality, plus the `e88269ab` known-good case.
6. **Unchanged-path test:** `sumcp --file` on a single transcript produces
   byte-identical output to v0.1 for the demo fixture, except the new fields.

---

## 10. Non-goals

- Exact content evidence from `file-history`. Sub-project 2.
- Any new detector, threshold change to the review floor, or change to the
  3-file review cap. Sub-project 3, where it can be measured.
- The git-based outcome label. Sub-project 3.
- Cross-session friction across *different* work units, and the personal
  baseline for localization dispersion. Sub-project 4. Note that this spec
  builds the foundation for it: once a work unit exists, "across work units" is
  the natural next aggregation.
- Any accuracy or predictive claim. This spec changes what is measured, not
  what is claimed, and the README's Findings and roadmap section stays honest
  until sub-project 3 produces a measurement.

---

## 11. Decisions on the two softest points

**Debrief timing.** The concern was that the Stop hook fires at the end of
transcript 3 of 7, so a work-unit debrief would describe a partial stretch and
call it whole. On inspection this dissolves: transcripts 4 through 7 do not
exist yet at that moment. The unit is computed from what is on disk at analysis
time, and it is complete and correct as of then. The only real consequence is
that re-running the same debrief tomorrow yields a larger unit, which is
correct behaviour rather than a discrepancy.

Decision: no special handling, no suppression timer, no daemon. The
`work_unit` disclosure already reports `sessions` and the span, so a report is
self-describing about what it covered. The debrief skill gains one sentence
noting that a stretch still in progress will grow.

**Whether `--file` hints.** Decision: yes, stderr only. When an explicitly
named transcript belongs to a larger unit, print `note: this transcript is 1 of
7 in a work unit; use --work-unit to analyze all of it` to stderr. Stderr keeps
`--json` and `--html` pipeable, which is an existing invariant, and the
alternative is a user reading a 51-edit report with no indication that 224 more
edits sit beside it.

These two are flagged as the softest points in the design and are the first
places to look if implementation turns up friction.
