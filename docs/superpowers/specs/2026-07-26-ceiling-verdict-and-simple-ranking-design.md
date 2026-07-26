# Ceiling verdict and simple ranking

Date: 2026-07-26. Status: approved (brainstorm session).
Scope: `crates/sumcp-core/examples/validity_dump.rs`, `scripts/validity_sweep.py`,
new `scripts/ceiling_analysis.py`, new `crates/sumcp-core/src/file_class.rs`,
`score.rs`, `payloads.rs`, `html.rs`, `review.rs`, `sumcp-cli/src/main.rs`,
`sumcp-mcp/src/main.rs`, `sumcp-mcp/src/server.rs`,
`docs/payload-schema.md` (v0 to v1),
`fixtures/mock-payloads/`, `scripts/check_payloads.py`, `SPEC.md`,
`docs/metrics.md`, `README.md`, `tasks/todo.md`, new validation report.

## Problem

`tasks/todo.md` carries an open decision that blocks Checkpoint E: given T5.3's
negative result, either ship the narrowed usability framing, or treat "beat the
edit-count baseline" as a v0.1 goal and do a predict-then-check tuning pass.
The user chose the second.

A feasibility pass (Phase 0, run 2026-07-26 during the brainstorm) established
that the goal is not reachable on this corpus, and established it strongly
enough to close the decision rather than defer it again. Along the way it
surfaced specific, measured defects in the shipped ranking. This spec covers
confirming the negative result, then acting on the defects.

## Decisions locked before any measurement

Recorded here because a predict-then-check pass is only meaningful if these
predate the numbers.

1. **Win condition**: precision at matched flag count. The product's flags must
   fire on about as many (session, file) pairs as the baseline and have a
   higher share of them recur. Chosen because it states the review-queue
   promise directly.
2. **Outcome definition**: frozen at the already-preregistered primary,
   `strong` recurrence within the `next 3 sessions`. The predictor side was
   left completely open; the target side was not, because a movable target
   makes "beat the baseline" unfalsifiable.
3. **Corpus timing**: archive first, tune now, accept that only a large effect
   would be credible.
4. **Holdout rule**: a tuned config ships on tune-split evidence alone, but the
   README may state an accuracy claim only if the single holdout run also shows
   the product flag at or above the baseline on precision.
5. **Selection method** for the full pass, had it run: leave-one-project-out
   within the tune split, not best overall fit.

## Phase 0 findings (2026-07-26)

Tune split: 552 pairs, 39 positives, base rate 0.071, 24 sessions, projects
`proj-01`/`proj-02`/`proj-03`. Held out and excluded from every number:
`proj-04`, 238 pairs withheld. One frozen holdout fingerprint is absent from
the corpus. Outcome throughout: `next3_strong`.

### The corpus was decaying

Oldest surviving transcript was 2026-06-25 against a run date of 2026-07-26.
`cleanupPeriodDays` is unset, so Claude Code's 30-day default cleanup applies
and the corpus is a rolling window that loses sessions as fast as it gains
them. The 2026-07-22 study's stated range begins 2026-06-24, so at least one
analyzed session was already gone. The absent holdout fingerprint is most
likely cleanup rather than a project falling below `MIN_ACTIONS`.

Mitigated during the brainstorm: `~/.claude/projects` was copied to
`~/sumcp-corpus-archive/projects-2026-07-26/` (434 files, 186 MB, parent mode
0700, outside the repo so it can never be committed).

### Where we stood

| Rule | Flags | Hits | Precision |
|---|---|---|---|
| `flagged_nr` (product) | 53 | 19 | 0.358 |
| `flagged_top3` (product) | 54 | 18 | 0.333 |
| baseline top-3 by edits | 65 | 22 | 0.338 |

Precision ceiling at a 65-flag budget is 39/65 = 0.600, since only 39
positives exist.

### The file-class lever, and why it is not a win

| Rule | Flags | Hits | Precision |
|---|---|---|---|
| top-3 by edits, code files only | 52 | 22 | 0.423 |
| top-3 by edits, drop notes+config | 62 | 20 | 0.323 |
| top-3 by edits, drop notes+config+docs | 55 | 22 | 0.400 |

At the matched budget the apparent gain nearly vanishes:

| At 52 flags | Hits | Precision |
|---|---|---|
| top-3 by edits, code only | 22 | 0.423 |
| baseline trimmed to its 52 highest-edit flags | 21 | 0.404 |
| global top-52 by edits | 21 | 0.404 |

One extra hit out of 39. The real effect of the class filter is flagging 13
fewer files for the same hit count, which is a product improvement but not a
selection improvement.

### Restricted to code files, the weighting adds nothing

Code-only population: 285 pairs, 34 positives.

| Rule, code files only | Flags | Hits | Precision |
|---|---|---|---|
| `flagged_nr` | 40 | 18 | 0.450 |
| `flagged_top3` | 38 | 17 | 0.447 |
| baseline top-3 by edits | 52 | 22 | 0.423 |

Disagreement between `flagged_nr` and the code-only baseline: **product-only 3
flags carrying 0 hits, baseline-only 15 flags carrying 4 hits.** The product's
flags are close to a subset of the baseline's, the three files it adds are all
wrong, and the fifteen it omits contain four positives it misses.

### The ceiling: no weighting can win

Weights were fitted by coordinate ascent with random restarts to maximize hits
at a fixed budget **on the same pairs being scored**. That is maximally
overfit on purpose, so it upper-bounds any honest rule. Features: edits, log
edits, changed lines, log changed lines, number of distinct finding kinds,
presence of churn / rework / re-read / blind-write, four file-class
indicators, and `last_edit_verified`.

| Budget | Edit count alone | In-sample max (14 features) | Leave-one-project-out |
|---|---|---|---|
| 40 | 18 (0.450) | 21 (0.525) | 18 (0.450) |
| 52 | 21 (0.404) | 23 (0.442) | 9 (0.173) |
| 65 | 21 (0.323) | 25 (0.385) | 17 (0.262) |

Two readings:

- The ceiling is **+2 to +4 hits out of 39**. On 39 positives that cannot clear
  a confidence interval, and an in-sample maximum is optimistic, so no honest
  rule can do better.
- At every budget the optimizer assigned the **maximum available weight to
  `edits`**. Given free choice over 14 features, the best weighting is
  counting edits.

The leave-one-project-out column is worse than edit count everywhere, but it is
non-monotonic across budgets (18, 9, 17), which with only three coarse folds
indicates instability rather than a clean measurement. The verdict rests on the
in-sample ceiling, not on this column.

### Supporting distributions

Positives by edit count of the file in session N:

| Edits | Pairs | Positives | Rate |
|---|---|---|---|
| 1 | 310 | 6 | 0.019 |
| 2-3 | 149 | 11 | 0.074 |
| 4+ | 93 | 22 | 0.237 |

Six positives sit on single-edit files, unreachable by any rule requiring two
or more edits, at a rate well below the base rate. Not worth chasing.

By file class, over all tune pairs:

| Class | Pairs | Positives | Rate |
|---|---|---|---|
| code | 285 | 34 | 0.119 |
| docs | 192 | 1 | 0.005 |
| config | 37 | 0 | 0.000 |
| notes | 19 | 3 | 0.158 |
| other | 12 | 0 | 0.000 |
| web | 7 | 1 | 0.143 |

By finding kind:

| Kind | Pairs | Positives | Rate | Ranks today? |
|---|---|---|---|---|
| churn | 242 | 33 | 0.136 | yes |
| re_read | 97 | 21 | 0.216 | yes |
| rework | 94 | 19 | 0.202 | yes |
| blind_write_attempt | 24 | 0 | 0.000 | yes |
| failure_loop | 4 | 2 | 0.500 | yes |
| true_revert | 2 | 1 | 0.500 | no |

`re_read` has the best rate among the frequent kinds and is weighted 1.5,
below both `rework` and `fumble` at 3.0. `fumble` fires 24 times with zero
positives while holding a joint-highest weight.

Failure signals are rare because failures are rare: **58 confirmed failed
commands across 24 tune sessions, median 1 per session**, 20 of 24 sessions
with at least one. `failure_loop` firing four times is proportionate, not an
attribution defect. This is a corpus limitation to document, not a bug.

Floor and cap alternatives, both worse on the chosen metric:

| Variant | Pairs | Hits | Precision |
|---|---|---|---|
| churn-only files (blocked by the 2-findings floor) | 86 | 7 | 0.081 |
| churn-only and 4+ edits | 16 | 3 | 0.188 |

More than three files clear a 2-findings floor in 14 of 24 sessions, so raising
the cap adds flags. Both of the error autopsy's top-ranked detector ideas
therefore reduce precision, which is the metric this pass is judged on.

### Verdict

A credible precision win over top-N-by-edit-count is not reachable on this
corpus. The information is not present in the observable features. The
weighting is not the source of the lift the 2026-07-22 study measured.

## Direction

Confirm the verdict against the one feature set Phase 0 could not see, publish
it, then stop shipping a ranking mechanism that has been measured to do
nothing, while keeping every part that works.

Demote the weighted score. Rank by a rule that can be stated in one sentence
and verified by hand. Keep the findings as the explanation and citation layer,
which is the part no other tool has.

## Part 1: confirm the verdict

### 1a. Emit the missing features

`validity_dump.rs` currently emits, per file, `edits`, `changed_lines`,
`kinds` (presence only), `flagged_nr`, `flagged_top3`, `last_edit_verified`.
Add the weighted `score` (f64) and the per-category `breakdown` map, both
already present on `FileScore`.

Bump `CACHE_SCHEMA` in `scripts/validity_sweep.py` from 2 to 3. The existing
comment on `cache_path` states why this is mandatory: the freshness check
compares cache and transcript mtimes and cannot see that the dump binary
changed, so without a bump every cached dump would silently lack the new
fields and the reader's defaults would render the omission as a plausible
column of zeros.

### 1b. Promote the analysis into the repo

New `scripts/ceiling_analysis.py`, replacing the scratch script the brainstorm
used. **python3 stdlib only**, matching the discipline every other dev script
in this repo already follows (`sanitize.py`, `check_payloads.py`,
`validity_sweep.py`). The brainstorm's scratch version used numpy; a pure
implementation is cheap enough here, since coordinate ascent over 552 pairs and
roughly 20 features is a few hundred thousand dot products over short lists.
Budget a couple of minutes of runtime and reduce restarts before reaching for a
dependency.

Requirements:

- Reuses `validity_sweep.py`'s `build_corpus`, `group_by_project`,
  `anonymize_projects`, `held_out_project_ids`, `window_sessions`,
  `outcome_in_window`, and `top_n_files` by import rather than reimplementing
  them, so the split and the outcome logic cannot drift from the study.
- Performs the tune/held-out split **before any metric**, matching
  `docs/validation/holdout.md`.
- Seeded RNG, every collection sorted, so two runs are byte-identical.
- Feature set gains the real magnitudes from 1a: churn count, rework count,
  re-read count, blind-write count, failure-loop count, and the product
  `score` as a single feature.
- Reports, at budgets 40, 52 and 65: edit count alone, the in-sample maximum,
  and leave-one-project-out, with the fold-instability caveat printed
  alongside rather than left to the reader.

### 1c. Point the corpus at the archive

`PROJECTS_DIR` in `scripts/validity_sweep.py` currently points at
`~/.claude/projects`, which the 30-day cleanup is emptying. It must point at
the archive instead. This change belongs in `validity_sweep.py`, not in
`ceiling_analysis.py`, because the latter imports the former's `build_corpus`
and would otherwise silently read a different corpus than the study does.

Make the location an override rather than a hardcoded path: read
`SUMCP_CORPUS_DIR` from the environment, defaulting to the archive, and print
the resolved path on every run so which corpus produced a number is never in
doubt. Refreshing the archive stays a manual step, deliberately, so a run can
never quietly pick up sessions that arrived mid-analysis.

This cannot disturb holdout membership. Fingerprints hash the dump's `project`
field, which comes from `Session.cwd` and falls back to the transcript's parent
directory name, and the archive is a faithful copy that preserves both. The
fail-closed check in `held_out_project_ids` still guards the case where the
archive is wrong or empty.

### 1d. The gate, specified before the run

**Confirmed** if the in-sample maximum exceeds edit count by at most **4 hits
at every budget**. Rationale: four hits at budget 65 moves precision from
0.323 to 0.385, which is inside the interval width the 2026-07-22 study
reported for precision on this corpus, and an in-sample maximum is an
optimistic bound, so a rule that cannot reach +5 while cheating cannot
generalize a win. +4 is also the largest margin Phase 0 observed, so this gate
asks whether magnitudes change the picture, not whether they reproduce it.

**Overturned** if any budget exceeds +4. Then Part 2 does not proceed as
written; instead run the full preregistered pass (candidate set, leave-one-
project-out selection, one holdout scoring), and this spec is superseded.

## Part 2: the product changes

Proceeds only if Part 1 confirms.

### 2a. `file_class.rs`

New pure module in `sumcp-core`. One public function mapping a path to an enum
with variants `Code`, `Web`, `Notes`, `Docs`, `Config`, `Other`. No filesystem
access, so ADR A9 holds. Classification is by lowercased basename and
extension, in this precedence order:

1. Basename starting with `.env` is `Config`.
2. Path containing `/memory/`, or basename starting with `memory.`, or path
   containing `/notes` is `Notes`. Checked before extensions so that
   `memory/foo.md` is `Notes`, not `Docs`.
3. Extension tables, checked in order `Code`, `Docs`, `Config`, `Web`.
4. Everything else is `Other`.

Extension tables:

- `Code`: rs py ts tsx js jsx go java c h cc cpp hpp rb swift kt sh bash zsh
  sql vue svelte cs php lua scala dart ex exs clj hs ml m mm r pl
- `Web`: html css scss sass less
- `Docs`: md mdx txt rst adoc tex
- `Config`: json toml yaml yml ini cfg conf env lock properties gradle xml
  plist

Ranking tiers: `Code` and `Web` tier 0, `Notes` tier 1, `Docs` tier 2,
`Config` and `Other` tier 3.

Honesty note that must appear in the rustdoc and in `docs/metrics.md`: only
the code-versus-docs-and-config separation is supported by adequate data (285
pairs against 192 and 37). `notes` at 19 pairs shows a **higher** positive
rate than code (0.158 against 0.119) on three positives, which is far too thin
to promote it above code, so it sits directly below code rather than with
docs. `web` at 7 pairs is grouped with code because web files are code-like,
not because 7 pairs measured anything. The tier order is a declared judgment
on thin data everywhere except the code/docs boundary.

### 2b. Ranking

`score::rank` sorts by, in order:

1. Was the file edited at all, edited before un-edited.
2. Class tier ascending.
3. Edit count descending.
4. Path ascending.

Key 1 exists so the queue is about changes. An un-edited file has no change to
review, and today a never-edited `.jpg` ranks fourth on the demo fixture
purely for having been read four times. Un-edited files stay in the list,
because hiding data contradicts the project's ethos, but always below every
edited file. Key 4 preserves today's total, stable order.

`FileScore` drops `score: f64` and gains `class: FileClass` and `edits: u64`.
It keeps `file`, `breakdown` and `findings` unchanged, since those are the
explanation and the citations. `class` serializes as a lowercase snake_case
string (`code`, `web`, `notes`, `docs`, `config`, `other`), matching how
`FindingKind` already serializes.

**`Weights` is removed entirely.** Nothing else uses it: the detectors in
`signals/` never consult it, and every current caller
(`score::rank`, `html::render_html`, `payloads::struggle_areas`,
`sumcp-mcp/src/server.rs`, both binaries) is a ranking call site. Leaving a
public type that nothing reads would be dead weight that reads as
configurable. So `rank` loses its `&Weights` parameter, `render_html` loses
its, and the `weights` field on the MCP server state at
`sumcp-mcp/src/server.rs:45` goes with them.

Every detector, every finding, and every `Tier` / `exact` / `Confidence` label
is untouched. Those are properties of the evidence, not of the ranking, and
they are what the findings layer contributes.

Expected effect on the demo fixture, which is the README hero and must be
regenerated: the `.py` file carrying `failure_loops 2` moves from fifth to
second, the two `.md` files move below it, and the never-edited `.jpg` moves
last.

### 2c. Signal claims

No weight decimals are retuned, because 2b removes weights outright and Part 1
establishes that retuning them achieves nothing. What replaces them in the
documentation is the measured evidence per signal.

`docs/metrics.md` currently carries a weight column and a tier rationale
sourced from the literature. The weight column goes, since no weights exist.
In its place, each signal row records what this corpus actually showed, and
what that does and does not license:

- `fumble` (blind-write attempt) was weighted joint-highest on the strength of
  IDE-Bench's 63% figure and fires on 24 pairs with **zero** positives here.
  State the limit precisely: zero in 24 rules out a large positive effect, it
  does not establish the signal is harmful, and it does not refute IDE-Bench,
  whose population is autonomous benchmark trajectories rather than interactive
  sessions.
- `failure_loop` and `true_revert` are too rare here to characterise at all,
  with the 58-failed-commands-across-24-sessions figure as the reason, so a
  reader reads it as a property of this corpus and not of the detector.
- `re_read` has the best positive rate of the frequent kinds (0.216) and was
  weighted below both `rework` and `fumble`. Record the rate and note that the
  literature-derived ordering did not reproduce on the only corpus it has been
  measured against.

The `score.rs` module doc, which currently opens by explaining the weighted sum
and the editorial provenance of the decimals, is rewritten to describe the
ordering rule from 2b.

### 2d. Payload contract, v0 to v1

The shape change is unavoidable once `FileScore` changes, and is cheap now
because nothing is published. Exact edits:

- `payloads.rs:236` (`session_overview.top_struggles` entries): replace
  `"score"` with `"class"` and `"edits"`.
- `payloads.rs:364` (`struggle_areas` file entries): same replacement.
- `payloads.rs:386`: replace the `"weights"` object with a `"ranking_rule"`
  string. Exact value, a constant in `score.rs` so the payload and the report
  cannot drift apart:
  `"edited files first, then code before docs and config, then by edit count, ties by path"`
- `payloads.rs:337`: `struggle_areas` loses its `weights: &Weights` parameter.
  This removes the caller-controlled `weights.source` string that the cap loop
  currently has to truncate at `payloads.rs:344`, so that truncation branch and
  its test go too.
- `docs/payload-schema.md`: bump v0 to v1, rewrite lines 54-55 (which
  currently promise the `weights` used are always echoed) and the v-history
  row at line 85, and add a v1 row explaining the break.
- `fixtures/mock-payloads/struggle_areas.json` and `session_overview.json`:
  updated to the v1 shape.
- `scripts/check_payloads.py`: assert the v1 shape, including that
  `ranking_rule` is present and non-empty and that no `score` key survives.
- The `struggle_areas_echoes_weights_and_breakdown` test at `payloads.rs:830`
  becomes an equivalent test for `ranking_rule`.

Payload caps stay enforced by construction. The shrink loop is unchanged in
structure; it is re-verified against the new field set, since `class` and
`edits` are smaller than the `weights` object they replace, so the change is
cap-favourable.

### 2e. Consumers

- `html.rs:545` and `html.rs:825`: render class and edit count where they
  render `score` and `score {:.1} · {why}` today.
- `sumcp-cli/src/main.rs:258`: same for the terminal ranking line.
- `review.rs`: no rule change. Its floor stays 2 or more findings, or one
  high-signal finding, because Part 1 measured every looser variant as worse.
  It only follows the field rename. Note the free improvement: `needs_review`
  walks `ranked` in order, so the new class ordering changes which three files
  fill its cap for the better, without touching the floor.

### 2f. ADR A6 is retired

With weights gone, the TOML weights config has no job. `load_weights_from` at
`sumcp-mcp/src/main.rs:19` is removed along with its tests, and a config file
that is still present produces a one-line stderr notice that it is no longer
read, so an existing user is told rather than silently ignored.

This also closes a divergence the Codex review flagged: the CLI always used
`Weights::default()` while the MCP server loaded the config, so the two
surfaces could rank the same session differently. After this change there is
one ranking rule and no configuration to diverge on.

## Part 3: documents and the release gate

- **New** `docs/validation/2026-07-26-ceiling-analysis.md`: the method, the
  confirmed ceiling tables, the leave-one-project-out fold-instability caveat,
  the corpus limitations including failed-command scarcity and the 30-day
  cleanup, and the verdict. Anonymized projects, no real paths, matching the
  existing validation reports.
- **`docs/validation/2026-07-22-predictive-validity.md`**: unchanged as a dated
  record, with a forward pointer added.
- **`SPEC.md`**: amend decision 6 (transparent weighted ranking) and ADR A6,
  in the same amendment style the file already uses.
- **`tasks/todo.md`**: close "Decide what v0.1 claims" with the decision and
  its evidence, and add the archive requirement.
- **`README.md`**: rewrite "The numbers" to the claim the evidence supports.
  Not that the ranking is accurate, and not merely that it is cheap: the review
  queue is a rule a reader can verify in one line, it flags fewer files than
  the obvious alternative for the same hit count, and every entry carries
  deterministic evidence. Regenerate the hero screenshot, since 2b changes it.
- **Held-out gate**, once, after everything above is final:
  `python3 scripts/validity_sweep.py --release-eval`. Per decision 4, the
  config ships regardless and the number is published whatever it says. Since
  Part 1 forecloses any accuracy claim, no claim attaches to it.

## Testing

Red first, per the repo's practice.

- `file_class.rs`: one test per table, plus the precedence cases that are easy
  to get wrong (`memory/foo.md` is `Notes` not `Docs`; `.env.local` is
  `Config`; an extensionless `Makefile` is `Other`; uppercase `README.MD` is
  `Docs`).
- `score::rank`: edited before un-edited; class tier ordering; edit count
  within a tier; path tiebreak; and a regression test pinning the demo
  fixture's new order so the README hero cannot silently drift.
- Docs-only session: the ranking and `needs_review` are non-empty and the
  report renders, so a prose session never produces an empty report.
- `payloads.rs`: v1 shape for both affected tools, `ranking_rule` present and
  non-empty, no `score` key anywhere, and the existing cap tests re-run
  against the new shape.
- `review.rs`: existing tests pass unchanged apart from the field rename,
  which is the evidence that the floor was not altered.
- `html.rs`: class and edit count render; the redaction and determinism tests
  stay green.
- `scripts/check_payloads.py` and `scripts/check_narration.py` pass against
  the updated mocks.
- `scripts/ceiling_analysis.py`: two consecutive runs produce byte-identical
  output.
- Whole workspace: `cargo fmt --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`. CI runs
  `check_payloads.py`, so the schema, mocks and checker must land in the same
  commit as the payload change or CI fails, which is the intended behaviour.

## Out of scope

- Retuning weight decimals. Part 1 establishes there is nothing to gain.
- New detectors. Phase 0 found no unexploited orthogonal feature, and the
  failure-signal scarcity is a corpus property rather than a detector defect.
- Loosening the `needs_review` floor or raising its 3-file cap. Both measured
  as worse on the chosen metric.
- Any cross-session or memory-graph work. That remains the v0.2 direction in
  `docs/ideas/2026-07-21-cross-session-memory-graph.md`.
- Growing the corpus or seeking external validation. Still the top post-v0.1
  item, unchanged by this spec.
- Setting `cleanupPeriodDays` in the user's settings. Recommended separately;
  the archive plus the archive-reading requirement in 1b is what this spec
  depends on.

## Natural commit boundaries

One branch, in this order, so no intermediate state is broken:

1. Part 1a, 1b and 1c: the dump fields, the analysis script, and the corpus
   pointed at the archive.
2. Part 1d, the gate run and its recorded result.
3. Part 2a, `file_class.rs` with tests, used by nothing yet.
4. Part 2b, ranking, plus 2d and 2e together, because the payload contract,
   the mocks, the checker and the consumers must move as one or CI fails.
5. Part 2c and 2f, documentation claims and the vestigial config.
6. Part 3, reports, README, screenshot, and the release gate run.
