# Predictive-validity draft: do flags predict future rework?

Status: DRAFT, internal. Frozen default weights everywhere; no tuning
performed anywhere in this pass. Any future tuning follows a
predict-then-check rule: parameters would be set on one subset of
projects and re-run, unchanged, on the held-out remainder, never
fit and reported on the same data.

## Method

For every session N and every file it edited or wrote, we check whether
that file shows further struggle signal in a later window of the same
project. Two window definitions: the next 3 sessions after N, and all
sessions starting within 14 days of N. Two outcome definitions:

- weak: the file is edited again at all in the window
- strong: in the window, the file carries a failure_loop, user_corrected,
  true_revert, or flip finding, or is itself a needs_review candidate
  there (recurrence of struggle, not mere activity; plain churn/rework/
  re_read do not alone count as strong)

Two flag definitions from the same session-N analysis are compared against
both outcomes: flagged_nr (the file qualified for review::needs_review in
session N) and flagged_top3 (the file was in the top 3 of score::rank in
session N). Weights are Weights::default() throughout; nothing is tuned.

Sessions with fewer than 20 actions, and the transcript modified in the
last 10 minutes at run time (the in-progress session), are excluded from
the corpus entirely, not just as source sessions.

## Corpus

- projects: 6 (anonymized as proj-01..proj-06)
- sessions analyzed: 42
- date range: 2026-06-19T20:00:50.860000+00:00 to 2026-07-22T07:11:05.443000+00:00
- (session, edited-file) pairs in the metrics below: 663
- pairs excluded (session N is the last session of its project, no window successor exists): 90

## Metrics

Relative risk (RR) = P(outcome | flagged) / P(outcome | unflagged), over
edited files. RR > 1 means a flagged file is more likely to show the
outcome than an unflagged one. Contingency counts: a = flagged+outcome,
b = flagged+no-outcome, c = unflagged+outcome, d = unflagged+no-outcome.

### flagged_nr (review::needs_review)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.50 (35/70)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.23 | 0.50 | 0.79 | 35 | 35 | 133 | 460 |
| next 3 sessions | strong (struggle recurrence) | 6.16 | 0.23 | 0.58 | 16 | 54 | 22 | 571 |
| within 14 days | weak (any future edit) | 1.84 | 0.59 | 0.82 | 41 | 29 | 189 | 404 |
| within 14 days | strong (struggle recurrence) | 4.48 | 0.26 | 0.65 | 18 | 52 | 34 | 559 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 3.52 | 0.96 | 1.61 |
| next 3 sessions | strong (struggle recurrence) | 23.78 | 2.47 | 3.51 |
| within 14 days | weak (any future edit) | 2.46 | 0.87 | 1.48 |
| within 14 days | strong (struggle recurrence) | 13.59 | 1.73 | 4.14 |

### flagged_top3 (score::rank top-3)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.54 (41/76)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.03 | 0.46 | 0.79 | 35 | 41 | 133 | 454 |
| next 3 sessions | strong (struggle recurrence) | 6.25 | 0.22 | 0.55 | 17 | 59 | 21 | 566 |
| within 14 days | weak (any future edit) | 1.83 | 0.58 | 0.81 | 44 | 32 | 186 | 401 |
| within 14 days | strong (struggle recurrence) | 5.23 | 0.28 | 0.60 | 21 | 55 | 31 | 556 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 1.50 | 1.09 | 1.40 |
| next 3 sessions | strong (struggle recurrence) | 10.10 | 3.64 | 3.35 |
| within 14 days | weak (any future edit) | 2.13 | 1.07 | 1.31 |
| within 14 days | strong (struggle recurrence) | 12.75 | 2.42 | 3.96 |

## Caveats

- Single-machine, single-author corpus: not generalizable beyond this
  author's own working style.
- Small per-project session counts mean stratified cells can be sparse;
  a single-digit denominator makes a ratio noisy even when the sign is
  informative. Read the raw counts, not just the ratio.
- Weak outcome (any future edit) is confounded with file busy-ness; the
  strong outcome and the stratified RR exist specifically to separate
  "this file gets edited a lot" from "this file keeps struggling."
- Projects and sessions with fewer than 20 actions are excluded from the
  corpus outright, including as window members for other sessions; this
  is a scope choice, not a null result about short sessions.
- This is a frozen-weights, no-tuning pass. It measures whether the
  existing default weighting is doing anything predictive at all, not
  whether it is the best possible weighting.
