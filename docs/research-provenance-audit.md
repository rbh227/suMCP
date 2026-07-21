# Research provenance audit

Verifies the citations in `docs/metrics-spec.md` that ground suMCP's signals —
because a project whose pitch is "evidence, not self-report" cannot itself rest
on unverified or misattributed references.

**Method + limits.** Each arXiv ID was resolved against arxiv.org (July 2026)
and its abstract/metadata checked against the claim the spec attaches to it.
This is an **abstract-level** check: a "flag" means the abstract doesn't support
the specific claim, NOT that the paper definitely doesn't — the figure may live
in the body. Flags are "verify against full paper," not "false." Non-arXiv
industry/venue citations are marked high-confidence-real but were not re-fetched
here.

## arXiv citations

| ID | Paper (verified title) | Claim in metrics-spec | Status |
|----|------------------------|-----------------------|--------|
| `2604.02547` | *Beyond Resolution Rates: Behavioral Drivers of Coding Agent Success and Failure* — Mehtiyev & Assunção | 9,374 trajectories; read-before-edit ρ≈+0.68; length↔failure reverses once difficulty controlled | ✅ **verified** — title, scale, and core findings match |
| `2603.24631` | *Coherence Collapse: Diagnosing Why Code Agents Fail After Reaching the Right Code* — Kim et al. (TRAJEVAL, 16,758 trajectories) | "dominant theme in **39.7%** of edit-quality failures on SWE-bench Verified"; agents read ~22× more functions than needed | ⚠️ **reconcile number** — paper real & core claim (agents trash correct code) verified, but abstract states coherence collapse is **60–69%** of failures; the 39.7% figure and the 22× ratio need confirming against the body / a specific sub-metric |
| `2311.08596` | *Are You Sure? Challenging LLMs Leads to Performance Drops in The FlipFlop Experiment* — Laban et al. | capitulation flips = sycophancy; reversal WITHOUT new evidence is the unhealthy case | ✅ **verified** — studies reversal-under-pushback (~46% reversal, ~17% accuracy drop) |
| `2601.20886` | *IDE-Bench: Evaluating LLMs as IDE Agents on Real-World SE Tasks* — Mateega et al. | "Premature editing is the #1 empirical failure mode (**~63%** of failed runs)" | ⚠️ **likely misattributed** — paper is an 80-task IDE-agent benchmark; its abstract does not describe a 63% premature-editing failure-mode breakdown. Either the figure is elsewhere in the paper or the citation is wrong. **Highest-priority flag** (premature editing / blind-write is a headline signal). |
| `2503.18455` | *SEAlign: Alignment Training for Software Engineering Agent* — Zhang et al. | stuck-in-loop = ≥3 consecutive identical tool+args | ⚠️ **verify attribution** — paper is about MCTS alignment training; abstract doesn't mention identical-action loop detection. Spec already treats loops as advisory-only, so low blast radius, but the citation should be confirmed or replaced. |

## Non-arXiv citations (high-confidence real; not re-fetched here)

| Citation | Claim | Note |
|----------|-------|------|
| Nagappan & Ball, ICSE 2005 | relative (size-normalized) churn predicts defects; absolute churn does not | Foundational, well-known; underpins the `relative_churn` refinement field. |
| Perry et al., CCS 2023 | AI-assisted developers overconfident about their code's security | Real; supports the review-burden framing. |
| SmartBear/Cisco (3.2M LOC) | human defect detection collapses past ~200–400 LOC/review | Industry study; the review-burden band. |

## Bottom line for the README's credibility

- **The construct-validity backbone holds.** The core signals — churn/rework
  (coherence collapse), read-before-edit / premature editing, reverts/flips —
  are each grounded in a **verified** paper (`2604.02547`, `2603.24631`,
  `2311.08596`) plus Nagappan & Ball. suMCP can honestly claim its signals are
  research-backed difficulty indicators.
- **Two specific numbers must not go into public copy until reconciled:** the
  "63% premature editing" (IDE-Bench attribution) and the "39.7% coherence
  collapse" (vs the abstract's 60–69%). Until then, cite the *finding* ("premature
  editing is a leading failure mode"; "agents often trash correct code"), not the
  unverified percentage.
- Weights remain **editorial by construction** (rank-order only), exactly as
  `metrics-spec.md` §"Why these default weights" already states — no research
  provides per-file category weights, and this audit doesn't change that.

## Follow-ups
- [ ] Body-check `2601.20886` for the 63% figure; correct or replace the citation.
- [ ] Reconcile the `2603.24631` 39.7% / 22× figures against the paper body.
- [ ] Confirm or replace the `2503.18455` stuck-in-loop attribution.
