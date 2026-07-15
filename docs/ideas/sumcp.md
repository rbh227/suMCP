# suMCP — Idea One-Pager

Refined 2026-07-15 (idea-refine session). Companion to `SPEC.md` (engineering
contract) and `docs/metrics-spec.md` (metric catalog). This doc records the
product direction and the assumptions that must be validated.

## Problem Statement

How might we give developers honest, evidence-grounded answers about what their
coding agent *actually did* — without trusting the agent's self-report and
without paying the tokens to re-read the transcript?

## Recommended Direction

**The Autopsy, packaged for adoption.** The product is the MCP query layer over
deterministic session forensics (per SPEC.md) — six read-only tools the agent
calls mid-conversation to answer from evidence at ~1–2k tokens instead of
re-reading megabytes. Two packaging layers make it adoptable by strangers:

1. **Zero-config CLI first hit:** `sumcp` run bare prints the latest session's
   debrief in seconds, before anyone touches `.mcp.json`. The first 60 seconds
   is the product.
2. **Static self-contained HTML report:** `sumcp report --html` renders the
   same `Report` JSON into one file — churn timeline, struggle table, evidence.
   The screenshot that markets the tool.

Both are views of the one `Report` type the MCP serves (ADR A5); neither is a
second product. Success metric (6 months): **strangers install it.**

Explicitly *not* pivoting to the Morning Briefing (SessionStart injection of
last session's friction) or the CLAUDE.md Compiler (findings → durable context
rules) — they're the v0.2/v0.3 retention ladder and reuse this core unchanged.

## Key Assumptions to Validate

- [ ] **Signals feel true on *other people's* sessions** (dealbreaker —
  validated only on one corpus, one project, one machine). Test: run the gate
  script on 2–3 volunteers' transcripts across OS/versions *before* v0.1
  ships; if the top-3 struggle files feel wrong to them, weights or signals
  need work.
- [ ] **Parser survives harness diversity** (dealbreaker). Test: fixtures from
  ≥3 Claude Code versions incl. one not from this machine; unknown-type
  counters stay informative, never fatal.
- [ ] **An agent narrates well from compact JSON in <500 tokens** (dealbreaker
  for the debrief DoD). Test: build the debrief skill against a hand-written
  mock payload in week 1 — before the Rust parser is finished.
- [ ] **The token ratio is actually impressive** (~200:1 estimated). Test:
  measure on real fixtures; the number leads the README or the claim gets cut.
- [ ] **Strangers can discover it** (should-be-true). Test: MCP directory
  listings + one show-HN/reddit post with the HTML-report screenshot; success
  metric is installs by people never met.

## MVP Scope

SPEC.md v0.1 as locked (L1+L2 metrics + approval latency, six MCP tools,
debrief skill, Stop hook, `sumcp install`) **plus** the bare-CLI instant
debrief and the static HTML report. One job done well: *after a session, name
where the agent struggled, with citable evidence.*

## Not Doing (and Why)

- **Live local dashboard / web app** — viewers (claude-devtools, lm-assist,
  claude-code-log, claude-session-analyzer) are the crowded, non-moat category
  by the project's own thesis; the static HTML report captures the screenshot
  value at ~5% of the cost.
- **Morning Briefing & CLAUDE.md Compiler** — retention ladder, not the wedge;
  both need the autopsy core first (v0.2/v0.3).
- **Real-time tailing, cross-session store, team/PR product, public
  legibility index** — seams stay open, none in v0.1.
- **Any LLM, telemetry, or network call inside the tool** — the honesty brand
  depends on it.

## Open Questions

- Fixture/validation diversity beyond one machine — donated sanitized
  transcripts? a `sumcp sanitize --donate` flow?
- Does the v0.2 briefing need SQLite, or does a flat JSON friction-cache per
  project suffice?
- Discovery channel ranking: MCP registries vs r/ClaudeCode vs HN — where do
  actual MCP installs come from?
