# suMCP payload schema v1 (T0.1, frozen at Checkpoint A; bumped 2026-07-26)

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
| `v` | payload schema version, `1` |
| `session.id` | session uuid |
| `session.identified_by` | **provenance, ADR A4**: `tool_use_id` (verified self-identification), `explicit` (caller passed session_id), or `cli_latest` (CLI-only recency mode). MCP never emits a guess. |
| `truncated` | `true` whenever any cap trimmed content |

## Finding shape

Every finding-like object (anything with a `kind`) carries:

```json
{"kind":"rework","tier":"T2","exact":true,"confidence":"high","idxs":[102,141]}
```

- `kind` — churn | rework | failure_loop | re_read (renamed from thrash,
  2026-07-18) | fumble | blind_write_attempt | true_revert | flip |
  user_corrected | write_no_reread | read_unreferenced |
  large_write_instant_accept | opening_move | action_loop | review_burden
- `tier` — field-reliability tier T1–T3 (metrics-spec parser rules)
- `exact` — `true` = deterministic count; `false` = heuristic (attribution,
  latency); heuristics also carry a human-readable `note`
- `confidence` — high | medium | low (low counts ×0.5 in ranking)
- `idxs` — action indices proving the finding, dereferenceable via `evidence()`
- `nums` — optional map of numeric operationalizations (2026-07-18
  re-grounding); present keys per kind: opening_move
  `edit_fraction_first10`+`first_edit_index`, churn `relative_churn`,
  action_loop `repeats`, review_burden `loc`+`band_hi`. Absent when empty.

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
{"v":1,"error":"ambiguous_session","message":"...","candidates":[{"id":"...","mtime":"...","cwd_match":true}],"hint":"pass session_id"}
```

Emitted when self-identification cannot verify the caller and no explicit
`session_id` was given. Listing candidates lets the agent recover in one turn.

## Suppression (heuristic honesty)

`blind_spots.suppression` reports whether approval-latency metrics are active;
when `permissionMode` grants auto-accept they are suppressed entirely rather
than reported as meaningless numbers. `review_burden` (the comprehension-layer
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
