# File class and recurrence: a descriptive breakdown

Status: internal note. Scope: the tune split of the 2026-07-22 predictive
validity study, broken down by file class. Computed 2026-07-26.

## What this is, and what it is not

This is a **descriptive breakdown**, not a second study. It partitions the same
(session, file) pairs the 2026-07-22 study already reported and counts outcomes
in each partition. No model is fitted, no threshold is swept, nothing is
selected on the basis of an outcome, so there is nothing here to overfit.

It makes **no accuracy claim** and does not revisit the earlier study's
conclusion. Its only job is to record the measurement that motivated replacing
the weighted ranking score with a stated ordering rule, so that change rests on
counted evidence rather than on taste.

Population, unchanged from
[the 2026-07-22 study](2026-07-22-predictive-validity.md): 552 (session, file)
pairs across the tune split, held-out project excluded. Outcome: strong
recurrence (a failure loop, user correction, revert, flip, or re-qualifying for
review) within the next 3 sessions. 39 pairs carry that outcome.

## The breakdown

| class | pairs | outcomes | rate |
|---|---|---|---|
| code | 285 | 34 | 0.119 |
| docs | 192 | 1 | 0.005 |
| config | 37 | 0 | 0.000 |
| notes | 19 | 3 | 0.158 |
| other | 12 | 0 | 0.000 |
| web | 7 | 1 | 0.143 |

Classes are as defined by `sumcp_core::file_class::classify`, which is a pure
function of the path.

## What it supports

**Documentation and config churn does not predict recurrence.** Documentation
is 35% of the pair population and carries 1 of 39 outcomes. Config is 37 pairs
and carries none. Code is 285 pairs and carries 34. Ranking code above
documentation and config is supported by adequate data on both sides of that
boundary.

**Nothing else here is.** `notes` shows a *higher* rate than code, 0.158 against
0.119, on 19 pairs and 3 outcomes. `web` shows 0.143 on 7 pairs and 1 outcome.
Both cells are far too thin to order confidently. That is why the shipped tiers
place `notes` directly below code rather than beside documentation, and group
`web` with code because web files are code-like, not because 7 pairs measured
anything. Only the code-versus-docs-and-config boundary rests on real data;
every other tier relationship is a declared judgment on thin cells.

## The defect this fixed

Before the change, ranking was a weighted sum of struggle signals. On the
project's own demo fixture that sum produced this order:

```
1. data_store.py    score 13.0
2. architecture.md  score 10.0
3. api-notes.md     score  7.5
4. diagram.jpg      score  6.0
5. routes.py        score  6.0
```

Two documentation files and a **never-edited image** outranked a `.py` file
whose commands were failing. The image scored purely on having been read four
times. Under the four-key rule the same session ranks both `.py` files first and
the image last.

## Caveats

- Single author, single machine. Not generalizable beyond this author's working
  style, and the same limitation the 2026-07-22 study carries.
- Descriptive only. A breakdown cannot establish that class *causes* the
  difference; code files differ from documentation in many ways besides class.
- Thin cells everywhere except code and docs. Read the raw counts, not the
  ratios: a 3-outcome cell is not a rate.
- The corpus is a rolling window. Claude Code's `cleanupPeriodDays` defaults to
  30, so this exact population cannot be recomputed from the live transcript
  directory: sessions have already aged out of it.
- The class tables are a fixed, deliberately narrow list of extensions and path
  patterns. A project whose layout they misread would be ranked accordingly, and
  the tables are the first thing to check if a ranking looks wrong.
