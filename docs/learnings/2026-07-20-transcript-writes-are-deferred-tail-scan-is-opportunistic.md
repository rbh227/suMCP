# Transcript writes are deferred — the tail-scan is opportunistic, not reliable

**Date:** 2026-07-20 · **Context:** Checkpoint D live probes + Claude Code `/debug` log · **Status:** applied — retry trimmed, debrief skill re-anchored on explicit `session_id`, SPEC A4 amended

## Problem

ADR A4's tail-scan rested on the premise that the calling session "has
already appended this very tool_use to its own transcript" by the time the
MCP server runs — so a bounded retry (4 × 150 ms) would cover any flush
delay. In live testing the no-arg call failed closed with
`ambiguous_session` from a brand-new session, then again from a
hours-old session: 1 verified hit in 4 live attempts.

## Root cause

Claude Code **defers transcript writes**. The `/debug` log for a failing
call shows dispatch at `06:25:16.282`, 536 ms of server retries, and the
caller's own transcript mtime sitting at `06:25:08` — 8 seconds stale —
through the entire window. The triggering `tool_use` line reached disk
only after the tool result returned, carrying a *timestamp* from creation
time (`16.277`) that hides the late write. The one success was a lucky
unrelated flush landing mid-window (caught by a 50 ms disk watcher: line
on disk ~0.3 s before the result).

The write is usually **causally after** the result — the server would be
waiting for an event that is waiting for the server. No retry window fixes
that ordering.

## Solution

- The scan stays, as *opportunistic verification*: when it wins, a tail
  match is still proof (the id is unique by construction). Retries trimmed
  to 2 × 150 ms — waiting longer buys nothing.
- Explicit `session_id` is the **primary** path: the Stop-hook input JSON
  carries the session id, so the debrief flow never needs the race.
- The debrief skill no longer tells the agent to "pick the cwd-matching
  candidate" on ambiguity (`cwd_match` is true for every candidate in a
  single-project dir) — it asks the user instead of guessing.

## Evidence trail

Line timestamps record event creation, not disk arrival — mtime is the
only honest write clock. Fail-closed behavior held in all four attempts
(candidates listed, zero misidentifications), which is the property that
actually had to survive contact with reality.

## Rule

An identification mechanism may only *rely on* data whose write ordering
it controls or has measured. Data written by the counterparty on its own
schedule can verify — it cannot discover.
