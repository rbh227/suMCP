# Content substrings cannot identify the calling session

**Date:** 2026-07-17 · **Context:** T4.1 verify pass (three-persona review + real-data run) · **Status:** applied — fallback removed in `sumcp-mcp`

## Problem

The MCP server must know *which* session is calling it (ADR A4). T4.1's first
implementation had two identification paths: scan transcript tails for the
forwarded `tool_use` id, and — when no id was forwarded — a fallback scan for
our namespaced tool-name marker (`mcp__sumcp__`) with a 30-second freshness
window. The fallback also reported itself under the verified provenance label
`identified_by: "tool_use_id"`.

## Root cause

The fallback confused **presence of a string** with **evidence of an action**.
Transcript tails contain arbitrary *content*: a grep result, a diff of this
repo's own source, a fetched web page, a pasted log. In this repository nearly
every session's tail contains `mcp__sumcp__` without ever calling the server.
One such content match, landing while the real caller's flush was delayed
(exactly the window the retry loop exists for), yields `hits == 1` — and the
server confidently debriefs the wrong session. Both the correctness and safety
reviewers flagged it independently; ADR A4's own words ("a plausible-but-wrong
debrief is fatal for an honesty tool") condemned it.

The forwarded `tool_use` id does not have this flaw: it is unique by
construction and written to the caller's transcript *by the act of calling*,
so a tail match is proof of the call, not correlation with its vocabulary.

## Solution

Deleted the fallback. Identification is now exactly two paths, both proof-
grade: the forwarded `_meta["claudecode/toolUseId"]` (verified externally that
Claude Code sends this key on every MCP `tools/call`) scanned against tails,
or an explicit `session_id`. Anything else → `ambiguous_session` with a
candidates list — recoverable in one turn. The forwarded id is additionally
shape-validated (`toolu_`, ≤64 chars, alnum/`_`/`-`) so `_meta` can't be used
as a free-form content-existence probe over transcript tails.

## Why this way

The alternative (harden the fallback: structural `"name":"mcp__sumcp__`
marker, shorter window, distinct provenance label) shrinks the hole but keeps
the category error — matching on quotable text. Quoted JSON in content still
false-positives it. Fail-closed with a one-turn recovery costs almost nothing
now that the primary path is confirmed to work with the real client; a wrong
identification costs the product its only differentiator (trustworthiness).

**Rule for future identification/attribution paths (subagents, plugin
packaging, other clients):** the marker must be something only the *act being
attributed* can produce — a unique id, a nonce — never a name, label, or any
string that can be quoted into content by someone else.
