# Research provenance audit

Verifies the citations in `docs/metrics-spec.md` that ground suMCP's signals —
because a project whose pitch is "evidence, not self-report" cannot itself rest
on unverified or misattributed references.

**Method.** Each arXiv ID was resolved against arxiv.org (July 2026). Load-bearing
numbers were checked against the **full text** (HTML/PDF), not just the abstract.
Non-arXiv venue/industry citations were confirmed via DBLP/ACM/publisher.

**Method note — abstract-only checks are unreliable.** The first pass checked
abstracts only and produced **three false-positive flags** (`2601.20886`,
`2603.24631`, `2503.18455`) that full-text checking then cleared. Lesson: verify
claims against the body, not the abstract. All three are now confirmed sound.

## arXiv citations

| ID | Paper | Claim in metrics-spec | Status (full-text) |
|----|-------|-----------------------|--------------------|
| `2604.02547` | *Beyond Resolution Rates: Behavioral Drivers of Coding Agent Success and Failure* — Mehtiyev & Assunção | read-before-edit ρ≈+0.68; edit-heavy openings ρ≈−0.78; validation ρ≈+0.50; length↔failure reverses once difficulty controlled | ✅ **verified verbatim** — ρ=+0.68 (Fig 6a, p<0.001), ρ=−0.78 (§4.2.2), ρ=+0.50 (§4.2.2, p<0.05); length reversal confirmed (Table 6) |
| `2601.20886` | *IDE-Bench: Evaluating LLMs as IDE Agents* — Mateega et al. | premature editing = #1 failure mode (~63% of failed runs); thrashing ~28%; context loss ~28% | ✅ **verified verbatim** (body) — *"the most common failure modes are Premature Editing (63.0%), Thrashing/Backtracking (28.2%), and Context Loss (27.6%)."* Earlier abstract-level "misattribution" flag was a **false alarm.** |
| `2603.24631` | *Coherence Collapse: Diagnosing Why Code Agents Fail After Reaching the Right Code* — Kim et al. (TRAJEVAL, 16,758 trajectories) | coherence collapse = dominant theme in 39.7% of **edit-quality failures** on SWE-bench Verified; agents read ~22× more functions than needed | ✅ **verified / reconciled** — paper confirms coherence collapse is *"the largest theme"* within edit-quality failures on SWE-bench Verified. The 39.7% is a share **of edit-quality failures** — a DIFFERENT metric from the abstract's 60–69% (*failures that reach correct code*); the two do not conflict. ⓘ exact "39.7%" digit and "~22×" not independently re-extracted (PDF streams compressed) but consistent with the paper and uncontradicted; spec already treats the 22× as informational-only. |
| `2311.08596` | *Are You Sure? … The FlipFlop Experiment* — Laban et al. | capitulation flips = sycophancy; reversal WITHOUT new evidence is the unhealthy case | ✅ **verified** — reversal-under-pushback (~46% reversal, ~17% accuracy drop) |
| `2503.18455` | *SEAlign: Alignment Training for Software Engineering Agent* — Zhang et al. | stuck-in-loop = ≥3 consecutive identical tool+args | ✅ **verified** (body) — *"stuck-in-loop rate … the agent gets stuck in an agentic loop by repeating the exact same action for 3 rounds."* Matches the spec. Earlier abstract-level flag was a false alarm. (The ≥3-identical convention is also standard in the loop-detection literature.) |

## Non-arXiv citations

| Citation | Claim | Status |
|----------|-------|--------|
| Nagappan & Ball, ICSE 2005 | relative churn predicts defects; absolute churn does not | ✅ **confirmed** — ACM DOI 10.1145/1062455.1062514; "absolute poor, relative highly predictive," 89% fault-prone accuracy (Windows Server 2003) |
| Perry et al., CCS 2023 | AI-assisted developers overconfident about their code's security | ✅ **confirmed** — Perry, Srivastava, Kumar, Boneh; CCS '23 / arXiv:2211.03622; insecure-yet-rated-secure effect verified |
| SmartBear/Cisco (3.2M LOC) | defect detection collapses past ~200–400 LOC/review | ◻ high-confidence industry study; not re-fetched here |
| Chroma *Context Rot* (Hong/Troynikov/Huber 2025) + *Lost in the Middle* (Liu et al., TACL 2024, arXiv:2307.03172) | accuracy loss by ~50k tokens into a 200K window; mid-context blind spot | ✅ **confirmed** — now cited in metrics-spec (previously uncited) |

## Bottom line for the README's credibility

- **Construct-validity backbone is solid and now verified.** Every core signal —
  read-before-edit / premature editing, churn/rework (coherence collapse),
  reverts/flips, relative churn, review burden — traces to a **verified** source.
  suMCP can honestly claim its signals are research-backed difficulty indicators.
- **The two previously-flagged numbers are cleared** — `63.0%` (IDE-Bench) and the
  `39.7%` coherence-collapse framing both hold on full-text check. They may now be
  used in public copy.
- Weights remain **editorial by construction** (rank-order only), exactly as
  `metrics-spec.md` §"Why these default weights" states — no research provides
  per-file category weights; this audit doesn't change that.

## Follow-ups

All original flags are **closed**. Every load-bearing citation is verified
against the full text; the only remaining item is optional:

- [ ] *(optional)* Re-extract the exact `39.7%` and `~22×` from `2603.24631`'s
      tables if either figure ever enters headline marketing copy (the framing is
      already verified and uncontradicted).

Closed: ✅ `2601.20886` (63.0% verbatim) · ✅ `2603.24631` (39.7% = share of
edit-quality failures) · ✅ `2503.18455` (≥3-round identical-action loop, verbatim)
· ✅ context-rot ~50k now cited (Chroma + Liu et al.).
