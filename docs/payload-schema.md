# suMCP payload schema v2 (T0.1, frozen at Checkpoint A; bumped 2026-07-27)

The contract for what the six MCP tools return. Canonical examples live in
`fixtures/mock-payloads/` and are enforced by `scripts/check_payloads.py`
(token cap ≈ chars/4, required fields, provenance). The Rust `report.rs`
builders must produce payloads that pass the same checker.

Format is **compact JSON** (ADR A5): agents parse it reliably, snapshot tests
diff it, caps are enforceable by construction. The tool returns evidence; the
connected agent narrates.

## Envelope (every non-error payload)

| field | contents |
|---|---|
| `v` | payload schema version, `2` |
| `session.id` | session uuid |
| `session.identified_by` | **provenance, ADR A4**: `tool_use_id` (verified self-identification), `explicit` (caller passed session_id), or `cli_latest` (CLI-only recency mode). MCP never emits a guess. |
| `truncated` | `true` whenever any cap trimmed content |

`work_unit` is **not** part of this envelope: it appears only on
`session_overview`, and only when several transcripts were merged into one
report (T7). See the dedicated section below. Every other payload never
carries a `work_unit` key, merged or not.

## Finding shape

Every finding-like object (anything with a `kind`) carries:

```json
{"kind":"rework","tier":"T2","exact":true,"confidence":"high","idxs":[102,141]}
```

- `kind` — churn | rework | failure_loop | re_read (renamed from thrash,
  2026-07-18) | fumble | blind_write_attempt | true_revert | flip |
  user_corrected | write_no_reread | read_unreferenced |
  large_write_instant_accept | opening_move | action_loop | review_burden |
  secrets_file_touched (2026-07-26)
- `tier` — field-reliability tier T1–T3 (metrics-spec parser rules)
- `exact` — `true` = deterministic count; `false` = heuristic (attribution,
  latency); heuristics also carry a human-readable `note`
- `confidence` — high | medium | low (low counts ×0.5 in ranking)
- `idxs` — action indices proving the finding, dereferenceable via `evidence()`
- `nums` — optional map of numeric operationalizations (2026-07-18
  re-grounding); present keys per kind: opening_move
  `edit_fraction_first10`+`first_edit_index`, churn `relative_churn`,
  action_loop `repeats`, review_burden `loc`+`band_hi`. Absent when empty.
- `session`: **conditional, T7**, an 8-character short transcript id, present
  only in `struggle_areas` and `blind_spots` findings, and only when the
  underlying session merges several transcripts. Resolved from the finding's
  first proving `idx`: the action at that index carries which transcript it
  came from, and this is the same value short-formed. Absent for a
  single-transcript analysis, because there is only one transcript and
  naming it would be noise, not signal. Neither `struggle_areas` nor
  `blind_spots` ever renders a `work_unit` block of its own (see the
  Envelope section above): `session` here is a bare transcript id with no
  grouping explanation in the same document. A client that needs to
  interpret it, for instance to learn how many transcripts were merged or
  the gaps between them, has to read `session_overview`, the only payload
  that carries `work_unit`.

Ranked entries (`struggle_areas`' per-file objects, `session_overview`'s
`top_struggles`) never carry a `session` key, even inside a work unit: a
ranked file spans the whole unit by construction (it is edited across
however many transcripts touched it), so stamping it with one transcript id
would assert something false about where its evidence came from. `session`
only ever answers "which transcript backs this specific finding", never
"which transcript backs this file".

## Tools, caps, truncation rules

| tool | cap (tokens) | truncation rule |
|---|---|---|
| `session_overview` | 1000 | fixed shape; `top_struggles` capped at 3 |
| `struggle_areas(n)` | 1500 | files capped at n, findings per file capped (`findings_per_file_cap`), tail-first |
| `file_story(path)` | 1500 | **middle-out**: head + tail kept, middle elided with `elided:{count,between}` marker |
| `blind_spots` | 1000 | each list tail-truncated |
| `context_health` | 1000 | `read_never_referenced` sampled, total count always present |
| `evidence(idxs)` | 1500 | ≤10 actions, excerpts ≤600 chars |

All ranking output shows the per-category `breakdown` and the `ranking_rule`
that produced the order. There is no score: see the v1 section below.

These caps are enforced by construction as of 2026-07-25; see the dated
section at the bottom for the exact shrink order and disclosure fields.

## Error payload (fail-closed, ADR A4)

```json
{"v":2,"error":"ambiguous_session","message":"...","candidates":[{"id":"...","mtime":"...","cwd_match":true}],"hint":"pass session_id"}
```

Emitted when self-identification cannot verify the caller and no explicit
`session_id` was given. Listing candidates lets the agent recover in one turn.

## Suppression (heuristic honesty)

`blind_spots.suppression` reports whether approval-latency metrics are active.
Suppression is per action (the transcript's `mode` events say which permission
mode each action ran under), so "suppressed" here means every main-lane action
ran under auto-accept; a session that mixed modes stays "active" and its
normal-mode stretches keep their signals rather than being reported as
meaningless numbers. `review_burden` (the comprehension-layer
anchor, metrics-spec #27) is **never suppressed** — LOC-per-human-turn stays
meaningful under auto-accept, which is exactly when it matters most; the
suppression object says so explicitly.

## 2026-07-18 additive fields (non-breaking, `v` stays 0)

| payload | field | contents |
|---|---|---|
| `session_overview` | `patch_first_segment_share` | share of classified task segments opening patch-first (metrics-spec #9 roll-up); `null` when nothing classified |
| `blind_spots` | `review_burden` | ReviewBurden findings (LOC per human turn > 400 band) |
| `context_health` | `read_edit_file_ratio` | distinct files read ÷ distinct files edited, informational (#28); `null` for read-only sessions |
| `struggle_areas.weights` | `re_read`, `action_loop` | `thrash` key renamed; advisory loop weight added |

## 2026-07-20 additive field (T4.2, non-breaking, `v` stays 0)

| payload | field | contents |
|---|---|---|
| `session_overview.flags` | `subagent_files_missing` | count of subagent spawns whose child transcript could not be analyzed — not found, unreadable, oversized, parsed to zero actions, or beyond the 64-file cap. `0` when every spawn's work was merged in (the common case) and when no subagents ran. Replaces the pre-merge `subagents_excluded` counter. |

## 2026-07-23 additive fields (codex-review P0, non-breaking, `v` stays 0)

| payload | field | contents |
|---|---|---|
| `session_overview.session` | `started`, `duration_min` | wall-clock span of the parsed actions (first timestamp, whole minutes first→last). The mock contract always promised these; the shipped builder now emits them so the debrief's duration line is backed by data. Both `null` when no action has a parseable timestamp. |

## 2026-07-25 additive fields (headline undercount, non-breaking, `v` stays 0)

| payload | field | contents |
|---|---|---|
| `session_overview.totals` | `file_ops` | file-modifying operations that **confirmed success** (`is_error == Some(false)`). `edits` alone reads as "everything that changed a file" but omits `Write`, undercounting by 20-50% on a typical session. Both operands stay in the payload for consumers that want the split. |
| `session_overview.totals` | `lines_written` | lines of NEW content across every **confirmed-successful** Edit (`new_string`) and Write (`content`), summed over all lanes so subagent work counts. A tool-call count says how *often* a tool fired, not how much changed: one Edit can rewrite hundreds of lines. Counted at ingest on the full string before capping, so large writes stay accurate. **Scope: lines written.** Deletions are excluded (an Edit removing 50 lines and adding 2 contributes 2), because the stored `old_string` is capped and would undercount exactly the largest edits. |
| `session_overview.totals` | `file_ops_unconfirmed` | Edit/Write actions whose result did NOT confirm success: explicit `is_error: true`, or no tool result at all (truncated or mid-flight session). Excluded from the two fields above and disclosed here rather than silently counted either way. `edits`/`writes` stay unfiltered and describe what was *attempted*, which the failure signals need. |

Why: `session_overview` led with `edits`, which measured neither all
file-modifying operations nor the volume of change. On a real 401-action
session it read `edits 71` where the honest figures are 88 operations and
3,751 lines written. Both surfaces (text view and HTML facts strip) now lead
with `file_ops` + `lines_written` and keep edits/writes as the breakdown.

The success filter matters because ingest captures `write_lines` from the
*proposed* tool input, before the result is known. Across the 75-session
development corpus, 57 of 2,237 Edit/Write actions (2.5%) failed, carrying
3,692 proposed lines (4.4%) that never reached a file. Counting those as
"lines written" would overstate real work, so `file_ops`/`lines_written`
require a confirmed-successful result and the remainder is disclosed in
`file_ops_unconfirmed`.

## 2026-07-25 cap enforcement (by construction, non-breaking, `v` stays 0)

Until now only `evidence()` actually measured its own output. The other five
built a payload and hoped it fit, so an adversarial or merely large session
blew the advertised budget. Measured before the fix on a **plain, non-hostile**
300-edit / 12-file synthetic session (ordinary paths, real detector output):
`blind_spots` came out at ~3166 tokens against its 1000 budget, and
`struggle_areas` at ~2827 against 1500 merely because the caller asked for
`n=99`. Under adversarial input it was far worse: a single 4 KB path (legal
under POSIX) put `file_story` over 1500 on its own, 500 distinct event types
put `session_overview` at ~6450 against 1000, and 200 ranked files put
`struggle_areas` at ~1.6M tokens against 1500.

Every builder now **shrinks its own output until it fits**, using the same
loop `evidence()` always used. Two invariants make the guarantee total rather
than probable:

1. Every caller-controlled string is capped before the loop runs: file paths
   (`160` chars), the session id (`120`), timestamps (`40`). Over-long strings
   are elided **middle-out with an inline marker** stating the loss, e.g.
   `/work/pro…[3901 chars elided]…/deep/file.rs`. The marker makes the value
   unmistakably not a real path, so it is never silently mistaken for one.
   An elided path cannot be passed back to `file_story`.
2. Every list has a `k = 0` floor, so shrinking always terminates with a
   payload made only of scalars and capped strings.

Whenever anything is dropped, `truncated` is `true` **and** the payload still
carries the full count, following the `subagent_files_missing` /
`elided:{count}` precedent.

| payload | knob shrunk, in the documented order | disclosure added |
|---|---|---|
| `session_overview` | `flags.unknown_event_types` **sampled**: the `k` most frequent unmodeled types, ties by name (deterministic), `k` walking 8 → 0 | `flags.unknown_event_types_total` (distinct types seen), `top_struggles_total` (files that ranked). `truncated` is now `true` when more than 3 files ranked, when the type map was sampled, or when a string was elided |
| `struggle_areas(n)` | `n` **clamped to 20** (`n_max`); then **tail-first**: lowest-ranked files dropped one at a time, and only once a single file remains do its findings start going | `files_total`, `n_max`, per-file `findings_omitted`, and `findings_per_file_cap` now reports the cap ACTUALLY applied (may be below 4) |
| `file_story(path)` | unchanged **middle-out**; the head/tail edge shrinks 8 → 0 if the capped path still leaves no room | `elided.count` as before |
| `blind_spots` | all three lists **tail-truncated to the same `k`**, walking 8 → 0 | `totals` (full count of each list, always present) and `list_cap` (the `k` in force) |
| `context_health` | fixed shape; the only droppable item is the prose `note` | `truncated` is now computed, not hard-coded `false` |
| `evidence(idxs)` | unchanged: ≤10 actions, excerpts ≤600 chars, tail-first drop | as before |

Findings echoed in any payload also cap their proving `idxs` at **10** (all
`evidence()` will dereference anyway) and add `idxs_total` when they do. This
is the single biggest real-world overrun: one churn finding on a file edited
800 times carries 800 indices, and a `review_burden` finding's `idxs` span
every edit in its segment.

### Which findings survive the per-file cap

`struggle_areas` used to keep the first 4 findings a file had, which was
**detector order**: `edit_shape` → `failures` → `dynamics` → `comprehension`.
A file scored on rework *and* failure loops *and* blind writes showed four
rework findings and no failure evidence, contradicting its own `breakdown`.

The rule is now **round-robin over the scoring categories, most alarming
first** (the fixed `SEVERITY_ORDER`: failure_loops, fumbles, rework, churn,
re_read, action_loops). Every category that contributed to the score gets one
finding before any category gets a second. Within a category the detectors'
own (chronological) order is preserved and the tail is what drops. So the
retained set is representative of the `breakdown` the same payload prints.

### Not addressed here

`context_health`'s advertised `read_never_referenced` sampling describes a
list the shipped builder does not emit: `read_unreferenced` and
`write_no_reread` are mock-only finding kinds with no detector behind them
yet. When they land they must arrive with their own cap and total.

## Versioning

`v` bumps on any breaking shape change; the checker and mock payloads update
in the same commit (they are the contract test).

## 2026-07-26 BREAKING: `v` goes 0 to 1 (spec 2026-07-26)

The weighted score is gone, so two payloads change shape. Every payload's `v`
becomes `1`.

| payload | removed | added |
|---|---|---|
| `struggle_areas` | `weights` object, per-file `score` | `ranking_rule` string, per-file `class` and `edits` |
| `session_overview` | `top_struggles[].score` | `top_struggles[].class`, `top_struggles[].edits` |
| `blind_spots` | (nothing) | `secrets_file_touched` list plus its `totals` entry: one finding per credentials/key file (`FindingKind::SecretsFileTouched`) the session read, edited, or wrote. Zero-tolerance signal, surfaced here rather than ranked because `file_class` puts `Config` in the last ranking tier and burying a secrets touch there would defeat the point. Shares the same `list_cap` shrink as the other three `blind_spots` lists. |

`class` is one of `code`, `web`, `notes`, `docs`, `config`, `other`. `edits`
counts Edit and Write attempts against that file.

Why: fitting ranking weights to maximize hits with the outcomes in hand bought
at most 4 hits out of 39 on the only corpus this has been measured against, and
the fit assigned maximum weight to edit count anyway. The order is now four
declared keys a reader can check by hand, and `ranking_rule` ships alongside
the order so SPEC §7's "never an opaque number" holds more strongly than
before. Full method and tables in
`docs/validation/2026-07-22-predictive-validity.md`, and the file-class
measurement study is forward-referenced at
`docs/validation/2026-07-26-file-class-measurement.md`.

## 2026-07-27 BREAKING: `v` goes 1 to 2, work-unit grouping disclosed (T7)

suMCP can now merge several transcripts of one continuous stretch of work
(same project, joined when a transcript overlaps the previous or starts
within 30 minutes of its end) into a single report. This version makes that
grouping visible instead of silently folding several transcripts' evidence
into one payload with no way to tell they were ever separate. Every
payload's `v` becomes `2`.

### `work_unit` (new, `session_overview` only)

Present when the report covers more than one transcript, OR when something
was discovered but excluded (an unreadable member, an undatable sibling, a
cap drop): an exclusion is exactly what must not go undisclosed. Absent
entirely for a plain single-transcript analysis with nothing excluded.

Every count and list describes the transcripts that were **actually
analyzed**, so the invariants below hold even when a discovered member could
not be read; what was excluded is disclosed in its own fields rather than
skewing the analyzed counts.

```json
"work_unit":{
  "rule":"same project; joined when a transcript overlaps the previous or starts within 30 min of its end",
  "sessions":3,
  "joined_gaps_min":[5.5,-12.0],
  "span_start":"2026-01-01T09:00:00Z",
  "span_end":"2026-01-01T19:15:00Z",
  "session_ids":["aaaaaaaa","bbbbbbbb","cccccccc"],
  "dropped":0,
  "members_unreadable":0,
  "siblings_unplaced":0
}
```

| field | contents |
|---|---|
| `rule` | the grouping rule, printed verbatim (same auditability promise as `ranking_rule`: a reader checks the grouping by hand, never by trusting an opaque decision) |
| `sessions` | how many transcripts were analyzed and merged into this report |
| `joined_gaps_min` | one entry per analyzed transcript after the first: the gap in minutes between the running span's end and that transcript joining. **A negative value means the transcript overlapped the running span instead of following it**, which is how a reader spots a concurrent Claude Code instance rather than a continuation. Always `sessions - 1` entries. |
| `span_start` / `span_end` | first and last timestamp across the analyzed members |
| `session_ids` | analyzed transcript ids, oldest first, each shortened to its first 8 characters (`short_id`): long enough to identify a transcript, short enough to hold the token cap, and it leaks neither the home directory nor the username the way a full path would. Always exactly `sessions` entries. |
| `dropped` | how many transcripts the unit's size cap discarded (oldest first); `0` when everything merged in |
| `members_unreadable` | discovered members of this unit that could not be loaded (unreadable file, over the byte ceiling). Their ids, gaps, and spans are excluded from the fields above rather than miscounted into them. |
| `siblings_unplaced` | same-directory transcripts whose time span could not be read at discovery. Such a file cannot be placed in time, so whether it belongs to this unit is unknown; the count says "something here could not be grouped" without claiming membership. |

### `session` (new, conditional, on findings only)

Every finding rendered by `struggle_areas` and `blind_spots` gains an
optional `session` key: the 8-character short id of the transcript its
evidence came from, resolved from the finding's first proving `idx` (the
action at that index carries a `session_ix` into the work unit's transcript
table). Present only when the underlying session merges several
transcripts: a single-transcript report has nothing to disambiguate, so the
key is omitted rather than always printing the one transcript id.

**Neither `struggle_areas` nor `blind_spots` ever carries a `work_unit`
block alongside this key.** That block is rendered only by
`session_overview` (see the Envelope section above), and this is a
deliberate choice, not an oversight: payloads are token-budgeted, and
`session_overview` is documented as the starting point for a session, so
duplicating the grouping block into every payload that can carry a
`session` id would spend that budget restating a rule a reader can look up
once. So `session` here is a bare short id with no explanation of the
grouping in the same document; it answers only "which transcript did this
finding's evidence come from", nothing about how many transcripts were
merged or how. A client that wants that context, or the full-length session
id, must read `session_overview`, the only payload that documents the
grouping. Do not reimplement or duplicate the `work_unit` block into
`struggle_areas` or `blind_spots` to "fill this gap"; read
`session_overview` instead.

This is enforced by construction, not just by convention: `finding_session`
(the one function that produces a `session` value) returns `None` unless
BOTH the session table has two or more entries AND the caller supplied a
work unit (`meta.unit.is_some()`). A caller that passes a merged
multi-transcript `Session` while leaving the work unit unset gets no
`session` keys at all. That construction governs *whether a `session` key
may appear at all*; it says nothing about a `work_unit` block accompanying
it in the same payload, because no such block ever does, in any payload
other than `session_overview`.

Ranked entries never carry `session`, in `struggle_areas`' files or
`session_overview`'s `top_struggles`: a ranked file spans the whole work unit
by construction (edited across however many transcripts touched it), so one
transcript id on it would misstate where its evidence came from. `session`
answers "which transcript backs this finding", never "which transcript backs
this file".

### A single-transcript payload is unaffected

For a session with zero or one transcript, `work_unit` is never emitted and
no finding ever gains a `session` key. The **only** difference from a v1
payload is the `v` field itself, every other byte is identical. This is
checked directly: `crates/sumcp-core/src/payloads.rs`'s
`session_overview_omits_the_work_unit_block_for_one_transcript` and
`struggle_areas_echoes_the_ranking_rule_and_breakdown` tests assert the
absence of `work_unit` and `session` respectively on a single-transcript
session.

## 2026-08-11 NEW: `v: 3`, `review_context` and `session_intent` (review-context design)

Two new tools, `review_context(commit_range)` and
`session_intent(commit_range, [max_tokens])`, plus the equivalent bare CLI
paths (`sumcp context --file`/`--work-unit`). Both are `v: 3`. The six v2
tools documented above are unchanged and stay at `v: 2`; `v` is per-payload,
not a single crate-wide number, and only these two new payloads carry `v: 3`.

Design intent (full reasoning in
`docs/superpowers/specs/2026-08-10-review-context-design.md`): a reviewing
agent gets a small `review_context` payload pushed once at the start of a
review (facts with citations, never a verdict), and can pull the full
verbatim human requests from `session_intent` separately, on demand. Splitting
them is deliberate: handing a reviewer requirements up front and asking it to
check conformance induces overcorrection (flagging correct code because it
was primed to look for flaws), and the effect worsens the more it is asked to
explain and repair. `review_context` never includes the full request text for
this reason.

Both tools cover the WHOLE work unit by default (the same multi-transcript
scope `session_overview` covers), not a single transcript, which is why the
citation rules below exist.

### `review_context(commit_range)`

```json
{
  "v": 3,
  "session": {"id": "abc12345", "identified_by": "explicit"},
  "scope": {
    "requests": [{"text": "add a cache to the loader", "line": 4, "session": "bbbbbbbb"}],
    "files_edited": ["/loader.rs"],
    "unattributed_turns": 0
  },
  "decisions": [{
    "question": "Which store?",
    "chosen": "JSONL",
    "options_not_chosen": ["SQLite"],
    "options_not_chosen_total": 1,
    "idxs": [12],
    "line": 5,
    "session": "bbbbbbbb"
  }],
  "incomplete": {
    "unfinished_tasks": [{"subject": "Add tests", "last_status": "pending", "line": 9, "session": "bbbbbbbb"}],
    "failing_commands": [{"command": "cargo test", "idxs": [20]}]
  },
  "claims": [{"text": "Done, nothing to change.", "line": 30, "window_interrupted": false, "session": "bbbbbbbb"}],
  "totals": {"requests": 1, "files_edited": 1, "decisions": 1, "unfinished_tasks": 1, "failing_commands": 1, "claims": 1, "agent_texts_excluded": 0},
  "list_cap": 12,
  "coverage": {"coverage_incomplete": false, "unattributed_turns": 0, "agent_texts_excluded": 0, "note": "..."},
  "attributed_intent_via": "session_intent",
  "truncated": false
}
```

Four blocks (`scope`, `decisions`, `incomplete`, `claims`). A fifth,
`constraints` (heuristic, "approaches tried and abandoned"), was designed but
cut before implementation (2026-08-11, plan Task 9): exact-string command
matching does not survive real retries, and it would have been the only
heuristic block in a payload whose whole purpose is reducing false positives.
The spec is amended in place rather than left to disagree with the shipped
build.

**`work_unit`** (present under the same condition as `session_overview`'s:
more than one transcript analyzed, or something was excluded) is the SAME
block documented above, byte-for-byte, built by one shared function both
payloads call. See that section for every field.

**Citations are not uniform across blocks**, because not every extracted fact
is backed by an `Action`:

| block | primary citation | dereferences via |
|---|---|---|
| `decisions` | `idxs` (empty unless `resolution` is `Resolved`; an `AskUserQuestion` call can also go unmatched or ambiguous, and the payload does not expose `resolution` itself, only its consequence) | `evidence(idxs)` |
| `incomplete.failing_commands` | `idxs` | `evidence(idxs)` |
| `scope.requests` | `line` (+ `session` when multi-transcript) | direct transcript read; a human turn is not itself an `Action` |
| `incomplete.unfinished_tasks` | `line` (+ `session`) | direct transcript read; a task-status transition is not itself an `Action` |
| `claims` | `line` (+ `session`) | direct transcript read; a prose block used as a claim boundary is not indexed as an `Action` |

**The `session` key** (short 8-char id, same encoding as `work_unit`'s
`session_ids`) is added alongside `line` on `scope.requests`, `decisions`,
`incomplete.unfinished_tasks`, and `claims` **only when
`work_unit` is present AND the unit spans more than one transcript**
(`session_ids.len() > 1`), the same conditionality `finding_session` already
used for v2's per-finding `session` key. This exists because `line` alone is
a line number WITHIN one transcript's own file: once the default scope covers
several transcripts, two different transcripts can share the same line
number, and without `session` a citation cannot say which file it names. A
single-transcript payload never carries a `session` key on any item; that
is the exact byte-stability guarantee every earlier version's payload keeps.

**`options_not_chosen`** is NOT a rejection claim. It is every offered option
whose text is not identical to the recorded answer string, populated by exact
string inequality, not by any judgement about intent. Three cases worth
knowing:
- A free-text "Other" answer can affirm and refine an option ("SQLite with
  WAL" against offered option "SQLite"); that option still appears in
  `options_not_chosen`, even though the human chose and elaborated on it.
- An unanswered question (`chosen: null`, session ended before a result line)
  lists **every** offered option: there is no answer text to compare against,
  so nothing is excluded. The options are still evidence of what was under
  consideration, which is why ingest keeps an unanswered decision at all.
- An ambiguous decision (`chosen: null`, two questions with identical text in
  one `AskUserQuestion` call, where the transcript's own answers map cannot
  tell them apart) behaves the same way: every option for each of the two
  decisions lists as not-chosen.

`options_not_chosen` is itself a capped, elided sample: `options_not_chosen`
shows up to `list_cap` entries, each elided at `QUOTE_MAX` (500 chars), with
the true count always in `options_not_chosen_total`.

**`window_interrupted`** on a claim is `true` when the human turn that closed
the claim's window was an interrupt (`[Request interrupted by user]`) rather
than an ordinary turn. The prose is still real evidence of what the agent was
doing, but it may be mid-turn narration the human cut off rather than a
completion summary; a reviewer should read it as possible intent, not
necessarily a claim to check against the diff. Always `false` for the
trailing window (no human turn followed) and for a window closed by an
ordinary turn.

**Caps and elision.** `list_cap` (default source: `CONTEXT_LIST_MAX = 12`)
is the ONE shared `k` all six lists (`requests`, `files_edited`, `decisions`,
`unfinished_tasks`, `failing_commands`, `claims`) shrink to together,
tail-first, walking `k` down from 12 until the 2000-token cap
(`CAP_REVIEW_CONTEXT`) is met. `totals` always reports the true count of each
list, unconditionally, so "3 shown" is never mistaken for "3 happened". Every
transcript-derived string this payload echoes (requests, questions, answers,
option labels, task subjects, statuses, commands, claim text) is elided at
`QUOTE_MAX` (500 chars) or `PATH_MAX` (160 chars, for file paths); `truncated`
is `true` if EITHER a list was capped below its true length OR any single
field had to be elided, so a single oversized string can never be dropped
invisibly while `truncated` stays `false`.

**`coverage`** discloses what `review_context` could not attribute, at the
top level (not nested, so a caller checking only `truncated` still sees it):
`unattributed_turns` (origin-less user turns excluded from `scope.requests`)
and `agent_texts_excluded` (subagent prose that never made it into the pool
`claims` draws from). `coverage_incomplete` is `true` if either is nonzero.

### `session_intent(commit_range, [max_tokens])`

```json
{
  "v": 3,
  "session": {"id": "abc12345", "identified_by": "explicit"},
  "requests": [{"text": "add a cache to the loader", "line": 4, "session": "bbbbbbbb"}],
  "totals": {"requests": 1},
  "requests_included": 1,
  "requests_omitted_for_size": 0,
  "request_coverage": {"coverage_incomplete": false, "unattributed_turns": 0, "note": "..."},
  "budget_too_small": false,
  "min_viable_tokens": 210,
  "truncated": false
}
```

The full verbatim human requests, never elided: the entire point of this
tool is exact quoting, so a request that does not fit the budget is DROPPED
WHOLE rather than cut down. `line` (+ `session`, same multi-transcript
conditionality as `review_context`) cites each one the same way
`scope.requests` does; there is no `idxs` here either, for the same reason
(a human turn is not an `Action`).

**`request_coverage`, not `coverage`.** Deliberately renamed and narrower
than `review_context`'s `coverage`: this payload has no `claims` block and
never touches `agent_texts`, so `agent_texts_excluded` would describe an
exposure it does not have. Folding it in anyway to look consistent with
`review_context` would be worse than the divergence it fixes: an identically
named field with a different meaning in each payload is exactly as unsafe as
two different names for the same thing. `request_coverage` speaks only to
`unattributed_turns`, the one exposure this payload actually shares with
`review_context` (both derive it from the same `context::scope`, so the two
can never disagree about that count).

**`budget_too_small` / `min_viable_tokens`.** `max_tokens` (optional; a
caller-supplied value can only ever LOWER the 20000-token default cap
`CAP_INTENT`, never raise it) is honored as a real ceiling, not just a
suggestion. `min_viable_tokens` is the token cost of the fixed envelope with
zero requests included (session id, totals, `request_coverage`, and, after
the Minor-5 fix below, the `min_viable_tokens` field itself), computed once
per call and always reported. When the caller's cap is below it, no amount of
packing can help: the response is that zero-request envelope with
`budget_too_small: true`, rather than a payload silently bigger than what was
asked for. Otherwise, requests are packed INDIVIDUALLY in order (not via the
shared `list_cap` walk `review_context` uses): each is added if the running
payload still fits the cap, otherwise skipped and counted in
`requests_omitted_for_size`, and the walk continues to the next request
rather than giving up, so one oversized early request cannot starve every
request behind it. `requests_included` and `requests_omitted_for_size` are
always both present, so a reviewer can tell "intent existed but was withheld
for size" from "there was no intent to show".

`min_viable_tokens` is threaded through the SAME measurement every fit check
uses (not appended to the payload after the fact): earlier, the field was
added only after the packing loop's `size <= cap` check had already passed,
so its own tokens were never part of what got measured, and a budget sitting
right at the true envelope size could come back larger than the cap it
claimed to honor while still reporting `budget_too_small: false`. Fixed by
folding the field into the measured envelope itself, so the reported ceiling
is exact: the returned payload never exceeds a cap set at or above
`min_viable_tokens`.

### Citations are 0-based (crate-wide convention)

Every `line` in every payload, v0.1 through v3, is 0-based
(`ingest.rs`'s line-numbering `enumerate()` starts at 0), so a human
cross-checking `"line": 7` against an editor's line numbers is off by one. Not
a bug: it is a deliberate, consistent convention applied identically by
production extraction and by the independent recount gate
(`crates/sumcp-core/tests/context_recount.rs`), so the two can never disagree
about it. Documented here rather than changed because changing it now would
be a breaking shape change to every payload that carries a line number, for a
cosmetic off-by-one a machine consumer never notices.
