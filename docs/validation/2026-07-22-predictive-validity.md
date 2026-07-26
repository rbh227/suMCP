# Predictive-validity draft: do flags predict future rework?

Status: DRAFT, internal. Frozen default weights everywhere; no tuning
performed anywhere in this pass. Any future tuning follows a
predict-then-check rule: parameters would be set on one subset of
projects and re-run, unchanged, on the held-out remainder, never
fit and reported on the same data.

Scope: every number in this report is computed on the TUNE SPLIT only.
Held-out projects contribute nothing to any table here, and their
per-pair outcomes are not written to the raw output either. Holdout
membership is frozen by project fingerprint in
`docs/validation/holdout-snapshot.json`, so it cannot drift as the
corpus grows.

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

- projects: 5 (anonymized as proj-01..proj-05)
- sessions analyzed: 44
- date range: 2026-06-24T17:42:08.403000+00:00 to 2026-07-24T20:36:57.157000+00:00
- held out, excluded from every number below: proj-04 (scored only at a release gate, via `validity_sweep.py --release-eval`)
- (session, edited-file) pairs in the metrics below: 554 (tune split)
- pairs excluded (session N is the last session of its project, no window successor exists): 65

## Metrics

Relative risk (RR) = P(outcome | flagged) / P(outcome | unflagged), over
edited files. RR > 1 means a flagged file is more likely to show the
outcome than an unflagged one. Contingency counts: a = flagged+outcome,
b = flagged+no-outcome, c = unflagged+outcome, d = unflagged+no-outcome.

### flagged_nr (review::needs_review)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.34 (18/53)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.38 | 0.66 | 0.80 | 35 | 18 | 139 | 362 |
| next 3 sessions | strong (struggle recurrence) | 8.98 | 0.36 | 0.51 | 19 | 34 | 20 | 481 |
| within 14 days | weak (any future edit) | 2.11 | 0.68 | 0.82 | 36 | 17 | 161 | 340 |
| within 14 days | strong (struggle recurrence) | 9.04 | 0.42 | 0.51 | 22 | 31 | 23 | 478 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | n/a | 1.24 | 1.78 |
| next 3 sessions | strong (struggle recurrence) | n/a | 4.16 | 3.55 |
| within 14 days | weak (any future edit) | n/a | 0.93 | 1.72 |
| within 14 days | strong (struggle recurrence) | n/a | 2.91 | 4.26 |

### flagged_top3 (score::rank top-3)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.37 (20/54)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.25 | 0.63 | 0.80 | 34 | 20 | 140 | 360 |
| next 3 sessions | strong (struggle recurrence) | 7.94 | 0.33 | 0.54 | 18 | 36 | 21 | 479 |
| within 14 days | weak (any future edit) | 2.00 | 0.65 | 0.82 | 35 | 19 | 162 | 338 |
| within 14 days | strong (struggle recurrence) | 8.10 | 0.39 | 0.53 | 21 | 33 | 24 | 476 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 0.00 | 1.42 | 1.58 |
| next 3 sessions | strong (struggle recurrence) | 0.00 | 3.12 | 3.39 |
| within 14 days | weak (any future edit) | 0.00 | 1.06 | 1.53 |
| within 14 days | strong (struggle recurrence) | 0.00 | 2.27 | 4.07 |

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
