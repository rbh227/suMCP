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
- sessions analyzed: 43
- date range: 2026-06-24T17:42:08.403000+00:00 to 2026-07-24T14:24:07.238000+00:00
- held out, excluded from every number below: proj-04 (scored only at a release gate, via `validity_sweep.py --release-eval`)
- (session, edited-file) pairs in the metrics below: 552 (tune split)
- pairs excluded (session N is the last session of its project, no window successor exists): 50

## Metrics

Relative risk (RR) = P(outcome | flagged) / P(outcome | unflagged), over
edited files. RR > 1 means a flagged file is more likely to show the
outcome than an unflagged one. Contingency counts: a = flagged+outcome,
b = flagged+no-outcome, c = unflagged+outcome, d = unflagged+no-outcome.

### flagged_nr (review::needs_review)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.36 (19/53)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.32 | 0.64 | 0.80 | 34 | 19 | 138 | 361 |
| next 3 sessions | strong (struggle recurrence) | 8.94 | 0.36 | 0.51 | 19 | 34 | 20 | 479 |
| within 14 days | weak (any future edit) | 2.07 | 0.66 | 0.82 | 35 | 18 | 159 | 340 |
| within 14 days | strong (struggle recurrence) | 9.42 | 0.42 | 0.50 | 22 | 31 | 22 | 477 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | n/a | 1.24 | 1.72 |
| next 3 sessions | strong (struggle recurrence) | n/a | 4.16 | 3.55 |
| within 14 days | weak (any future edit) | n/a | 0.94 | 1.66 |
| within 14 days | strong (struggle recurrence) | n/a | 3.23 | 4.26 |

### flagged_top3 (score::rank top-3)

False-alarm share (flagged files with no edit at all in the next 3 sessions): 0.39 (21/54)

| window | outcome | RR | precision | miss rate | a | b | c | d |
|---|---|---|---|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 2.19 | 0.61 | 0.81 | 33 | 21 | 139 | 359 |
| next 3 sessions | strong (struggle recurrence) | 7.90 | 0.33 | 0.54 | 18 | 36 | 21 | 477 |
| within 14 days | weak (any future edit) | 1.96 | 0.63 | 0.82 | 34 | 20 | 160 | 338 |
| within 14 days | strong (struggle recurrence) | 8.42 | 0.39 | 0.52 | 21 | 33 | 23 | 475 |

Stratified RR by session-N edit count of the file (busy-file confound check):

| window | outcome | 1 edit | 2-3 edits | 4+ edits |
|---|---|---|---|---|
| next 3 sessions | weak (any future edit) | 0.00 | 1.42 | 1.52 |
| next 3 sessions | strong (struggle recurrence) | 0.00 | 3.12 | 3.39 |
| within 14 days | weak (any future edit) | 0.00 | 1.08 | 1.47 |
| within 14 days | strong (struggle recurrence) | 0.00 | 2.49 | 4.07 |

## Baseline comparison: does the weighted ranking earn its complexity?

The tables above establish that the product's flags beat *chance*. That
is a low bar. The question that decides whether the weighting is worth
anything is whether it beats a rule a reader could implement in one
line, so every trivial predictor below is pushed through the identical
contingency / RR / precision machinery, over the identical tune-split
pair population, as the two product flags.

### Threshold provenance

Every threshold is either zero-parameter or a constant that already
existed in this repo, for an unrelated reason, before this comparison
was written. Nothing here was swept, and where a quantity has two
pre-existing boundaries both are reported rather than the better-looking
one being selected after the fact.

| rule | how the threshold was fixed |
|---|---|
| PRODUCT flagged_nr (review::needs_review) | product rule, frozen Weights::default() |
| PRODUCT flagged_top3 (weighted score::rank top-3) | product rule, frozen Weights::default() |
| baseline edits >= 2 | lower boundary of this script's pre-existing edit_stratum() buckets ('1' vs '2-3'), which predate this comparison; also equals the product's own CHURN_MIN_EDITS = 2 in signals/edit_shape.rs |
| baseline edits >= 4 | upper boundary of this script's pre-existing edit_stratum() buckets ('2-3' vs '4+'), which predate this comparison |
| baseline top-3 by edit count | zero-parameter; N = 3 fixed by the product flag it is compared against (flagged_top3) |
| baseline changed lines >= 200 | lower edge of the human code-review band cited in signals/comprehension.rs (SmartBear/Cisco: 'under 200, not to exceed 400'); predates this comparison |
| baseline changed lines >= 400 | REVIEW_BAND_HI = 400 in signals/comprehension.rs, the product's own review-burden threshold; predates this comparison |
| baseline top-3 by changed lines | zero-parameter; N = 3 fixed by the product flag it is compared against (flagged_top3) |
| baseline session has >= 1 failed command | zero-parameter: 'any confirmed failed command at all' is the only threshold on this quantity that requires choosing no magnitude. SESSION-level, so it is constant across every file in a session and has no within-session discriminative power by construction |
| reference: flag every edited file | degenerate reference, not a candidate: flags everything, so recall is perfect and miss rate is 0 by definition. Its precision IS the base rate, which is the number every other rule's precision must beat |

Top-N baselines use N = 3, the same N as the product flag they are
compared against, so the comparison is like-for-like on flag count and
not just on definition. Ties are broken by file path: deterministic, and
independent of the outcome being predicted.

`changed_lines` counts only Edit/Write actions whose tool result
confirmed success, matching `report.rs`'s `lines_written`. `edits`
counts every attempted Edit/Write, so a file can have edits > 0 and
changed_lines == 0.

`flagged` is the count of tune-split pairs the rule fires on, out of
552. It is reported first on purpose: a rule that flags
everything achieves perfect recall and zero miss rate for free, and must
be readable as doing so. The `flag every edited file` row is that
degenerate reference; its precision is the base rate every other rule has
to beat.

`thin cells` lists the contingency cells with fewer than 5 observations. Any
row with a thin cell has an RR that cannot support a conclusion, no
matter how large the point estimate looks.

### Headline: strong (struggle recurrence), next 3 sessions

This (window, outcome) pair was declared the primary comparison in the
script before any result was computed. The other three combinations
follow, in full, regardless of how this one came out.

| rule | flagged | flag share | RR | RR 95% CI | precision | miss rate | a | b | c | d | thin cells |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PRODUCT flagged_nr (review::needs_review) | 53 | 0.10 | 8.94 | 5.11-15.67 | 0.36 | 0.51 | 19 | 34 | 20 | 479 | - |
| PRODUCT flagged_top3 (weighted score::rank top-3) | 54 | 0.10 | 7.90 | 4.50-13.89 | 0.33 | 0.54 | 18 | 36 | 21 | 477 | - |
| baseline edits >= 2 | 242 | 0.44 | 7.05 | 3.00-16.54 | 0.14 | 0.15 | 33 | 209 | 6 | 304 | - |
| baseline edits >= 4 | 93 | 0.17 | 6.39 | 3.53-11.55 | 0.24 | 0.44 | 22 | 71 | 17 | 442 | - |
| baseline top-3 by edit count | 65 | 0.12 | 9.70 | 5.44-17.28 | 0.34 | 0.44 | 22 | 43 | 17 | 470 | - |
| baseline changed lines >= 200 | 42 | 0.08 | 3.13 | 1.54-6.38 | 0.19 | 0.79 | 8 | 34 | 31 | 479 | - |
| baseline changed lines >= 400 | 12 | 0.02 | 5.14 | 2.17-12.18 | 0.33 | 0.90 | 4 | 8 | 35 | 505 | a |
| baseline top-3 by changed lines | 65 | 0.12 | 2.94 | 1.54-5.62 | 0.17 | 0.72 | 11 | 54 | 28 | 459 | - |
| baseline session has >= 1 failed command | 544 | 0.99 | 0.56 | 0.09-3.59 | 0.07 | 0.03 | 38 | 506 | 1 | 7 | c |
| reference: flag every edited file | 552 | 1.00 | n/a | n/a | 0.07 | 0.00 | 39 | 513 | 0 | 0 | c,d |

### Verdict

Computed from the headline table, not written by hand, so it cannot
drift out of step with the numbers above it. Judged at
strong (struggle recurrence), next 3 sessions. A baseline
`dominates` a product flag when it is at least as good on ALL THREE of
RR, precision, and miss rate. Ties count as domination: the question is
whether the weighting BUYS anything over a one-line rule, and matching a
one-line rule buys nothing.

Strongest baseline by RR: baseline top-3 by edit count (RR 9.70).

- **flagged_nr** (RR 8.94): beats the strongest baseline on RR: NO. RR 95% CI overlaps that baseline: yes. Dominated by: none.
- **flagged_top3** (RR 7.90): beats the strongest baseline on RR: NO. RR 95% CI overlaps that baseline: yes. Dominated by: baseline top-3 by edit count.

**Verdict: the weighted ranking does NOT beat the trivial baselines. At least one one-line rule matches or dominates a product flag on RR, precision and miss rate simultaneously, and the RR confidence intervals overlap, so this corpus cannot distinguish the weighted score from counting edits.**

What this does and does not license as a claim:

- STILL SUPPORTED: the flags are far better than chance. Every
  product row above has an RR well over 1 with a CI excluding 1.
  Flagged files really do recur more than unflagged ones.
- NOT SUPPORTED: any claim that the weighting, the score, or the
  multi-signal model is what produces that lift. A rule that sorts
  files by how many times they were edited and takes the top 3 does
  the same job, on this corpus, within noise.
- NOT SUPPORTED: any comparative or superiority claim over simpler
  tools, since the simplest possible tool was not beaten here.
- The honest framing is that the product currently packages a
  known-useful signal (repeated edits) with explanation and evidence
  attached. That is a real product, but it is a usability claim, not
  a predictive-accuracy claim, and the README must not imply the
  latter until a corpus large enough to separate the two exists.

### weak (any future edit), next 3 sessions

| rule | flagged | flag share | RR | RR 95% CI | precision | miss rate | a | b | c | d | thin cells |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PRODUCT flagged_nr (review::needs_review) | 53 | 0.10 | 2.32 | 1.81-2.97 | 0.64 | 0.80 | 34 | 19 | 138 | 361 | - |
| PRODUCT flagged_top3 (weighted score::rank top-3) | 54 | 0.10 | 2.19 | 1.70-2.83 | 0.61 | 0.81 | 33 | 21 | 139 | 359 | - |
| baseline edits >= 2 | 242 | 0.44 | 1.82 | 1.42-2.35 | 0.42 | 0.41 | 101 | 141 | 71 | 239 | - |
| baseline edits >= 4 | 93 | 0.17 | 2.20 | 1.74-2.78 | 0.57 | 0.69 | 53 | 40 | 119 | 340 | - |
| baseline top-3 by edit count | 65 | 0.12 | 2.42 | 1.92-3.05 | 0.65 | 0.76 | 42 | 23 | 130 | 357 | - |
| baseline changed lines >= 200 | 42 | 0.08 | 1.16 | 0.76-1.78 | 0.36 | 0.91 | 15 | 27 | 157 | 353 | - |
| baseline changed lines >= 400 | 12 | 0.02 | 1.07 | 0.48-2.41 | 0.33 | 0.98 | 4 | 8 | 168 | 372 | a |
| baseline top-3 by changed lines | 65 | 0.12 | 1.04 | 0.72-1.52 | 0.32 | 0.88 | 21 | 44 | 151 | 336 | - |
| baseline session has >= 1 failed command | 544 | 0.99 | 1.25 | 0.37-4.18 | 0.31 | 0.01 | 170 | 374 | 2 | 6 | c |
| reference: flag every edited file | 552 | 1.00 | n/a | n/a | 0.31 | 0.00 | 172 | 380 | 0 | 0 | c,d |

### weak (any future edit), within 14 days

| rule | flagged | flag share | RR | RR 95% CI | precision | miss rate | a | b | c | d | thin cells |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PRODUCT flagged_nr (review::needs_review) | 53 | 0.10 | 2.07 | 1.64-2.61 | 0.66 | 0.82 | 35 | 18 | 159 | 340 | - |
| PRODUCT flagged_top3 (weighted score::rank top-3) | 54 | 0.10 | 1.96 | 1.54-2.49 | 0.63 | 0.82 | 34 | 20 | 160 | 338 | - |
| baseline edits >= 2 | 242 | 0.44 | 1.95 | 1.54-2.46 | 0.48 | 0.40 | 117 | 125 | 77 | 233 | - |
| baseline edits >= 4 | 93 | 0.17 | 2.00 | 1.61-2.49 | 0.60 | 0.71 | 56 | 37 | 138 | 321 | - |
| baseline top-3 by edit count | 65 | 0.12 | 2.07 | 1.66-2.59 | 0.65 | 0.78 | 42 | 23 | 152 | 335 | - |
| baseline changed lines >= 200 | 42 | 0.08 | 1.02 | 0.67-1.55 | 0.36 | 0.92 | 15 | 27 | 179 | 331 | - |
| baseline changed lines >= 400 | 12 | 0.02 | 0.95 | 0.42-2.13 | 0.33 | 0.98 | 4 | 8 | 190 | 350 | a |
| baseline top-3 by changed lines | 65 | 0.12 | 0.96 | 0.67-1.37 | 0.34 | 0.89 | 22 | 43 | 172 | 315 | - |
| baseline session has >= 1 failed command | 544 | 0.99 | 2.84 | 0.45-17.82 | 0.35 | 0.01 | 193 | 351 | 1 | 7 | c |
| reference: flag every edited file | 552 | 1.00 | n/a | n/a | 0.35 | 0.00 | 194 | 358 | 0 | 0 | c,d |

### strong (struggle recurrence), within 14 days

| rule | flagged | flag share | RR | RR 95% CI | precision | miss rate | a | b | c | d | thin cells |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PRODUCT flagged_nr (review::needs_review) | 53 | 0.10 | 9.42 | 5.60-15.82 | 0.42 | 0.50 | 22 | 31 | 22 | 477 | - |
| PRODUCT flagged_top3 (weighted score::rank top-3) | 54 | 0.10 | 8.42 | 5.00-14.17 | 0.39 | 0.52 | 21 | 33 | 23 | 475 | - |
| baseline edits >= 2 | 242 | 0.44 | 8.11 | 3.49-18.88 | 0.16 | 0.14 | 38 | 204 | 6 | 304 | - |
| baseline edits >= 4 | 93 | 0.17 | 6.49 | 3.73-11.29 | 0.27 | 0.43 | 25 | 68 | 19 | 440 | - |
| baseline top-3 by edit count | 65 | 0.12 | 9.86 | 5.76-16.87 | 0.38 | 0.43 | 25 | 40 | 19 | 468 | - |
| baseline changed lines >= 200 | 42 | 0.08 | 2.70 | 1.34-5.43 | 0.19 | 0.82 | 8 | 34 | 36 | 474 | - |
| baseline changed lines >= 400 | 12 | 0.02 | 4.50 | 1.92-10.57 | 0.33 | 0.91 | 4 | 8 | 40 | 500 | a |
| baseline top-3 by changed lines | 65 | 0.12 | 2.50 | 1.33-4.70 | 0.17 | 0.75 | 11 | 54 | 33 | 454 | - |
| baseline session has >= 1 failed command | 544 | 0.99 | 0.63 | 0.10-4.04 | 0.08 | 0.02 | 43 | 501 | 1 | 7 | c |
| reference: flag every edited file | 552 | 1.00 | n/a | n/a | 0.08 | 0.00 | 44 | 508 | 0 | 0 | c,d |

### Where they disagree, who is right?

Two rules can post identical RR and precision while firing on completely
different files, so matching a baseline on the marginals does not by
itself show the weighted score is just recomputing edit count. This
restricts attention to the pairs where a product flag and a baseline
disagree, and reports the outcome rate inside each disagreement bucket
(strong (struggle recurrence), next 3 sessions).
A product-only rate clearly above the baseline-only rate would mean the
weighting adds something the count does not see. Equal rates mean the
disagreements are noise.

These are the thinnest cells in the study. Read the raw counts.

| product rule | baseline | both n/pos/rate | product only n/pos/rate | baseline only n/pos/rate | neither n/pos/rate |
|---|---|---|---|---|---|
| flagged_nr | baseline edits >= 2 | 53/19/0.36 | 0/0/n/a | 189/14/0.07 | 310/6/0.02 |
| flagged_nr | baseline edits >= 4 | 35/15/0.43 | 18/4/0.22 | 58/7/0.12 | 441/13/0.03 |
| flagged_nr | baseline top-3 by edit count | 43/17/0.40 | 10/2/0.20 | 22/5/0.23 | 477/15/0.03 |
| flagged_nr | baseline changed lines >= 200 | 15/7/0.47 | 38/12/0.32 | 27/1/0.04 | 472/19/0.04 |
| flagged_nr | baseline changed lines >= 400 | 6/3/0.50 | 47/16/0.34 | 6/1/0.17 | 493/19/0.04 |
| flagged_nr | baseline top-3 by changed lines | 18/7/0.39 | 35/12/0.34 | 47/4/0.09 | 452/16/0.04 |
| flagged_nr | baseline session has >= 1 failed command | 52/19/0.37 | 1/0/0.00 | 492/19/0.04 | 7/1/0.14 |
| flagged_nr | reference: flag every edited file | 53/19/0.36 | 0/0/n/a | 499/20/0.04 | 0/0/n/a |
| flagged_top3 | baseline edits >= 2 | 52/18/0.35 | 2/0/0.00 | 190/15/0.08 | 308/6/0.02 |
| flagged_top3 | baseline edits >= 4 | 36/15/0.42 | 18/3/0.17 | 57/7/0.12 | 441/14/0.03 |
| flagged_top3 | baseline top-3 by edit count | 44/17/0.39 | 10/1/0.10 | 21/5/0.24 | 477/16/0.03 |
| flagged_top3 | baseline changed lines >= 200 | 14/6/0.43 | 40/12/0.30 | 28/2/0.07 | 470/19/0.04 |
| flagged_top3 | baseline changed lines >= 400 | 6/3/0.50 | 48/15/0.31 | 6/1/0.17 | 492/20/0.04 |
| flagged_top3 | baseline top-3 by changed lines | 19/6/0.32 | 35/12/0.34 | 46/5/0.11 | 452/16/0.04 |
| flagged_top3 | baseline session has >= 1 failed command | 52/18/0.35 | 2/0/0.00 | 492/20/0.04 | 6/1/0.17 |
| flagged_top3 | reference: flag every edited file | 54/18/0.33 | 0/0/n/a | 498/21/0.04 | 0/0/n/a |

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
- The baseline comparison is descriptive. No rule is fitted, so nothing
  here is corrected for multiple comparisons; the RR intervals are
  marginal 95% intervals for each row read on its own, and the rows are
  not independent of each other (they score the same pairs).
