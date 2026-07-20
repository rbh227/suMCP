---
name: debrief
description: End-of-session debrief grounded in suMCP evidence. Use at the end of a working session (or when the Stop hook nudges) to report honestly where the agent struggled, flip-flopped, or left blind spots — from transcript evidence, never from memory or self-report.
---

# Session debrief

Report what actually happened this session, grounded in suMCP's deterministic
transcript evidence. Your own recollection is self-report; the tools are the
record. Never answer from memory what the tools can answer from evidence.

## Procedure

1. If the invocation context provides a session id (the Stop-hook nudge
   includes one; a user request may name one), call
   `session_overview(session_id=...)` with it from the start. Otherwise call
   `session_overview()` bare — the server may verify you opportunistically.
   If it returns `error: ambiguous_session`, do NOT pick a candidate
   yourself: `cwd_match` is true for every session in this project and
   newest-mtime is exactly the recency guess this tool refuses to make
   (Claude Code flushes transcript writes late, so the live session is often
   not the newest file). Ask the user which session to debrief, listing the
   candidates' ids and mtimes.
2. Call `struggle_areas(3)`.
3. Only if a top finding needs a concrete quote: `evidence(idxs)` for ONE
   finding. Do not bulk-fetch.
4. Write the debrief. **Hard budget: 500 tokens.** Do not re-read any
   transcript, file history, or prior conversation to "check" the tools.

## Output contract

```
## Session debrief — <duration>, <edits> edits across <files> files

**Where I struggled:**
1. <file> (<top categories with counts>) — one sentence of what happened,
   with [idx] citations after each claim.
2. …
3. …

**Blind spots for you:**
- <blind-write attempts / files written and never re-read / approval
  outliers, each with [idx]>

**One takeaway:** <single most useful action for the developer, one sentence>
```

## Rules

- Every claim carries its `[idx]` citation so the developer can drill in with
  `evidence([...])`. A claim you cannot cite does not go in the debrief.
- Report the breakdown numbers (`churn 24, rework 9…`), not the opaque score.
- Do not soften. "I reworked the same region three times" is the point of
  this ritual; euphemisms defeat it.
- Do not editorialize beyond the evidence ("the code is bad" is not a
  finding; "I edited it 24 times" is).
- If `suppression` says approval-latency is suppressed, do not mention
  approval timing at all.
- If a `truncated` flag is set and something seems missing, say so rather
  than inventing.

## Mock mode (pre-v0.1 validation)

When the MCP server is not yet available, read the payloads from
`fixtures/mock-payloads/*.json` instead of calling tools. The output contract
is identical — this mode exists to validate the narration contract (T0.2).
