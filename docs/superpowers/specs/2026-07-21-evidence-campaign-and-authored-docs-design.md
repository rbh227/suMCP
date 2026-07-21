# suMCP v0.1 — Evidence Campaign & Authored Docs (design)

Date: 2026-07-21
Status: approved (brainstorm), pre-plan
Scope: the back half of v0.1 — T5.1–T5.4 + Checkpoint E — with the center of
gravity on a real evidence campaign and an authored (non-generic) README.
Supersedes nothing; extends `SPEC.md` §6 (release gate) and `tasks/todo.md`
Phase 5.

---

## 0. Framing

This is **not** "write a README." It is: *prove the thesis with real numbers,
verify the research honestly, then wrap both in a document that expresses the
author's view.* Three workstreams feed one document, in dependency order. The
README is authored **last** because it consumes everything upstream. Writing it
earlier as a marketing artifact would reintroduce exactly the unverified
hand-waves this project exists to reject.

### 0.1 The spine (author's own words, locked during brainstorm)

> **Agents don't know when they're making mistakes. suMCP is a cheap, honest
> mirror — for the agent *and* the user — of where the agent struggled and what
> it actually did.**

Consequences that bind every downstream choice:
- **Not adversarial.** The agent isn't a liar; it's *blind to its own struggle*.
  Voice is diagnostic and generous, never gotcha.
- **The agent is a first-class user**, not just the audited party. It calls the
  mirror mid-session.
- **No single target reader.** This is an authored artifact — the author's view,
  expressed through the project — not a persona-optimized conversion funnel.
  Voice: opinionated, first-person-honest, evidence-backed, not sales-y.

### 0.2 Non-negotiable integrity constraint

Because this is an *honesty* tool, its own docs are held to the standard the tool
enforces: **no number ships that traces back to nothing.** This applies to the
token ratio (already forbidden as "~200:1 estimated" in `SPEC.md`) *and* to every
ρ value / percentage in `docs/metrics-spec.md`. Their provenance is currently
**unknown** (author does not recall the sourcing). They are therefore treated as
unverified until an audit proves otherwise.

---

## 1. Workstream 1 — The Evidence Campaign (the numbers)

The centerpiece and the largest new build. Produces the README's headline
results.

### 1.1 Experimental design — task-level A/B

- **Arm A (baseline / "without the tool"):** a fresh headless agent (`claude -p`)
  is handed a raw session transcript and asked *"where did this agent struggle?
  cite evidence."* It re-reads and reasons unaided.
- **Arm B (with suMCP):** identical question, identical session, but the agent
  answers by calling suMCP's six tools and narrating from the compact JSON.

### 1.2 Quality pin — hand-labeled ground truth (gold standard)

A token/speed win is meaningless unless answer quality is held constant (an empty
debrief "wins" on tokens). So:

- Author + assistant read each gold-set session **once** and record the *true*
  top struggles (files, kinds, evidence idxs) as a labeled key.
- Each arm's answer is scored on:
  - **Recall of true struggles** (did it find them?), and
  - **False-accusation rate** (did it invent struggles that aren't real?) —
    weighted as the more important metric, because a mirror that fabricates is
    worse than one that misses.
- Token/speed numbers are only reported **at matched quality**.

### 1.3 Measured outcomes

- Tokens consumed per arm (dedup-correct, cross-checked against `ccusage` where
  possible).
- Wall-clock per arm.
- **Completion / context-fit rate for Arm A** — the sleeper headline: on large
  sessions the naive re-read may not fit in context at all, i.e. the naive
  method physically cannot answer while the tool always can. Report this
  explicitly; it may be more striking than any ratio.

### 1.4 Corpus — two tiers, two masters

- **Gold set (~5–8 of the author's own sessions):** hand-labeled; drives the
  rigorous A/B. These are the README's numbers. Honest limitation stated: one
  machine, one author's corpus.
- **External confirmation (2–3 volunteers; the SPEC §6 release gate / T5.3):**
  volunteers are asked only *"do the top-3 struggle files feel true on your own
  session?"* — a lightweight yes/no. **No labeling labor is asked of them.**
  Ideally at least one volunteer contributes a sanitized session for parser
  diversity, but that is a bonus, not the gate.

### 1.5 Staging (de-risk the first number)

1. **Smoke pass:** completion-rate + raw token/wall-clock counts on a couple of
   sessions, *no* quality pin. Purpose: produce *a* number fast, validate the
   harness, surface surprises.
2. **Full pass:** the rigorous hand-labeled A/B across the gold set. This is what
   the README cites.

### 1.6 New build — the A/B harness

A small, scripted, reproducible harness (likely driving `claude -p` headless with
the suMCP MCP server configured for Arm B, and without it for Arm A) that runs
both arms over a session list and logs tokens + wall-clock to a structured file.
Deterministic inputs; LLM nondeterminism handled by repetition (N runs per arm
per session, report spread, not a single point). This is the single largest new
engineering piece in the back half of v0.1.

---

## 2. Workstream 2 — Research Provenance Audit (the credibility)

- Enumerate **every** empirical claim in `docs/metrics-spec.md` (e.g.
  read-before-edit ρ≈+0.68, edit-heavy openings ρ≈−0.78, validation ρ≈+0.50,
  premature-editing ~63% of failed runs, thrashing ~28%, context-rot onset
  ~50k tokens, capitulation-flip detectability).
- For each, WebSearch the actual SWE-agent / LLM-failure / agent-eval literature
  and assign a **verdict**: `cited` (real source found, link it), `directional`
  (informed prior, no paper-exact source — relabel honestly, no fake precision),
  or `cut` (neither — remove).
- Deliverables: a per-claim provenance table (committed), corrected
  `docs/metrics-spec.md` in place, and the sourced input for the README's "Why
  these signals" section.
- Independent of Workstream 1; can start immediately; low risk.

---

## 3. Workstream 3 — The Authored Docs (the view)

### 3.1 README — authored artifact

Drafted **early as a living skeleton** that fills in as WS1/WS2 land, so the
story is visible while the campaign runs. Proposed spine-ordered structure:

1. **The why** — the story: agents are blind to their own struggle; the
   "you're absolutely right" reversal; comprehension debt you accept without
   seeing. First-person, honest, no hype.
2. **What it does in 60 seconds** — bare `sumcp` debrief + the six tools; the
   HTML report screenshot (from T5.1).
3. **The evidence** — real numbers from WS1: token/speed A/B at matched quality,
   Arm-A completion-rate, honestly framed (methods, N, spread, limitations).
4. **The research grounding** — "Why these signals," honest to WS2's verdicts.
5. **Architecture** — the diagrams (§3.2).
6. **Install & use** — via T5.2.
7. **Honest limitations** — one-machine corpus, heuristic tiers, the unverified
   2.1.x subagent path, what the tool refuses to compute.

Voice guardrail: diagnostic and generous, opinionated, evidence-backed. Reads
like it has an author.

### 3.2 Diagrams (assistant owns; author will not draw them)

Mermaid source committed in-repo as the diffable, honest-to-code source of truth
(renders natively on GitHub so the README is never broken):
- System architecture (core / cli / mcp crates; read-only boundary).
- The pipeline: ingest → merge (subagent flat-merge) → signals → weighted rank →
  payload.
- The fail-closed identification handshake (explicit `session_id` primary;
  opportunistic tail-scan; `ambiguous_session` fallback).
- Subagent flat-merge total ordering.

Plus polished renders (SVG or the report's Win9x aesthetic) for the 1–2 hero
diagrams that sell the story in a screenshot.

### 3.3 OSS docs (T5.4)

LICENSE (MIT OR Apache-2.0 dual, already locked), CONTRIBUTING, CHANGELOG,
SECURITY.md, rustdoc pass, issue templates.

---

## 4. Already-specced tasks — how they fit

- **T5.1 (HTML report):** design already locked 2026-07-19 (Win9x aesthetic,
  session-timeline centerpiece, no 3D graph). Build **early** — it yields the
  screenshot the README needs and is a natural artifact to show.
- **T5.2 (installer):** `sumcp install`/`uninstall` + Stop hook, fresh-home test.
  Build **before** volunteers so external validation (T5.3) is one command.

---

## 5. Sequence

```
1. T5.1  HTML report          locked design; yields the screenshot
2. WS2   Research audit        independent; start anytime, low risk
3. WS1   Evidence campaign     build harness → smoke pass → hand-label gold set → full A/B   ← the big lift
4. T5.2  Installer             onboard volunteers cleanly
5. T5.3  External validation   2–3 volunteers; satisfies SPEC §6 release gate
6. T5.4  README + docs         consumes WS1, WS2, T5.3 + diagrams; skeleton drafted early
7. CHECKPOINT E — v0.1.0 tag   publishing asks first
```

README skeleton (WS3) begins in parallel with step 1 and fills in as steps 2–5
produce content. Diagrams (§3.2) can be produced alongside step 1 onward.

---

## 6. Decisions locked in brainstorm

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | No single target reader; README is an authored artifact | Author wants to express his view, not run a funnel |
| D2 | Spine: "agents don't know when they're mistaken; a cheap honest mirror for agent + user" | Author's own words; kinder/more accurate than "don't trust the narrator" |
| D3 | Headline = task-level A/B (tokens + speed), not payload-vs-transcript | Most defensible, directly proves the thesis; author accepted the heavier path |
| D4 | Quality pinned by hand-labeled ground truth (recall + false-accusation) | Gold standard; false-accusation weighted highest for an honesty tool |
| D5 | Two-tier corpus: gold set (author's ~5–8, labeled) + external (2–3, feel-true only) | Rigor where it counts; no labor asked of volunteers; satisfies the release gate |
| D6 | Provenance audit of all metrics-spec ρ/%, provenance currently unknown | Honesty tool cannot cite numbers that trace to nothing |
| D7 | Assistant owns diagrams; Mermaid source + polished hero renders | Author will not draw them; Mermaid is diffable and honest-to-code |
| D8 | README drafted early as a living skeleton | Author wants to watch the story take shape |
| D9 | Evidence campaign staged: smoke number first, then full hand-labeled A/B | Early signal without abandoning rigor |

---

## 7. Open items / risks

- **A/B harness fairness:** Arm A's prompt must be a genuinely reasonable naive
  attempt, not a strawman. Design the prompt carefully and document it verbatim
  in the methods.
- **Nondeterminism:** LLM variance across runs — mitigate with repetition and
  reported spread; never a single cherry-picked number.
- **Volunteer recruitment** is a real-world dependency and may gate the tag; the
  gold-set A/B (author's machine) does not depend on it and can proceed first.
- **2.1.x subagent on-disk path** remains unverified against a real session
  (from T5.0); a volunteer session is the first chance to confirm it.
- Some ρ values may not survive the audit — the README must be willing to cut or
  soften them, and that is an acceptable outcome, not a failure.
