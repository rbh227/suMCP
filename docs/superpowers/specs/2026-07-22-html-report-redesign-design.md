# HTML report redesign: flat utilitarian, review-first

Date: 2026-07-22. Status: approved (brainstorm + grill session).
Scope: `crates/sumcp-core/src/html.rs`, small pure additions in core for new
derivations, README reframe, screenshot regeneration.

## Problem

The current report renders good data with no grid and no hierarchy: eight
misaligned totals lead the page, the timeline ships unexplained marks, the
ranking shows internal jargon and an unexplained score, blind spots are bare
counts, and every section box lays itself out independently on a full-bleed
teal desktop. It reads as "blob spam" (user's words) and it leads with trivia
instead of the one thing the product is for: which agent-written code needs
human eyes.

## Direction

Keep Win95's layout discipline (single window, strict grid, grouped sections,
status bar); drop the dated chrome. "Flat utilitarian": white page, one
centered column (max 920px), 8px spacing grid, near-black ink, classic navy
`#000080` as the only accent, red reserved for errors, sharp corners, 1px
hairline borders, no shadows or gradients or rounding, system font stack,
monospace only for paths, commands, and numbers (tabular, thousands-separated).
Self-contained single file, zero network, JS optional, print-clean,
deterministic byte-identical output. Every dynamic string through `esc()`.

## Page structure (top to bottom)

1. **Header band** (flat navy, white text): `suMCP` wordmark left; project
   directory, short session id, date, duration, and a `auto-accept` chip when
   approval suppression is active.
   - Duration is **active time**: sum of inter-action gaps, each gap capped at
     5 minutes, rendered as `active ~1h 40m (span 14h 22m)`. The cap is stated
     in a tooltip and lives in config, not hardcoded.
2. **Needs review** (new lead section). Qualification is an **evidence
   floor**, never a score threshold: a file is listed only with 2+ findings
   total, or 1 finding of a high-signal kind (failure loop, flip,
   user-correction, blind-write attempt). Cap 3. Each row: path, strictly
   descriptive reason from a fixed vocabulary with counts attached
   ("rewritten 8x, re-read 4x, 1 failure loop"), jump link to its story box.
   The calm state is three-way, so the wording never claims more than the
   data shows: nothing ranked and no blind spots either ("No struggle
   signals. No blind spots."); nothing met the review bar but blind-spot
   findings exist ("No files met the review bar. Blind spots below still
   apply."); nothing met the review bar but the struggle-areas table is not
   empty ("No files met the review bar. Minor signals appear in struggle
   areas below."). No empty boxes in any case.
3. **Timeline**: Read/Edit/Bash lanes plus finding bands anchored in a labeled
   `findings` strip. X-axis is **ordinal (action sequence), declared in the
   legend**; a small break glyph marks any inter-action gap over 5 minutes.
   Legend row explains: tick, error tick, finding band, user turn, gap glyph.
   User-turn rules carry a tooltip with the first ~80 characters of the
   prompt, passed through the same secret-redaction pass as evidence.
   Click-band-to-open-evidence behavior stays.
4. **Struggle areas**: up to 10 rows plus a "+N more files with minor
   signals" line. Columns: rank, file, score, breakdown in plain language
   ("rewrites x8, re-reads x4"). Top-3 rows emphasized and anchor-linked.
   Footnote states the formula and the exact weights used (score = sum of
   weight x evidence count per category).
5. **File stories**: only for Needs-review files (0 to 3 boxes). Each box
   opens with the why (score + findings in plain language), then a compressed
   chronology: consecutive same-kind events collapse into runs
   ("Edits #158-163 x6"), failures highlighted in red with their error
   excerpt inline, middle elision marker kept, evidence collapsible kept.
   Failure loops show the repeated command itself.
6. **Blind spots**: one muted line when clean. When findings exist, each is
   listed with evidence links. Heuristic signals (approval latency,
   instant-accept) are shown and visibly tagged `heuristic` with a
   one-sentence meaning ("timing-based inference, not proof"). Suppression
   becomes a sentence ("approval timing not measured; session ran under
   auto-accept"). Exact signals (blind-write attempts, review-burden) carry
   their counts and evidence.
7. **Status bar** (flat, bottom): `cache hit 97% | output 254,703 tok |
   parsed 4,812 lines | deterministic, no LLM | suMCP v0.1`. The Context
   health section is removed; this line replaces it.

## Data provenance (why each element is defensible)

- Session id: transcript filename. Project: `cwd` in transcript lines.
- Start/end/date: first and last timestamped lines (ordering contract carries
  timestamps forward across untimestamped lines).
- Active duration: pure function over action timestamps with the 5-minute cap.
- Ranking and breakdown: `score.rs` weighted evidence counts; findings carry
  kind, tier, exact-vs-heuristic flag, confidence, and proving `Idx`s.
- Stories and evidence: `payloads::file_story` and `payloads::evidence`
  (existing caps and redaction), never recomputed in the HTML layer.
- Blind spots: `payloads::blind_spots`, same contract as the MCP tool.
- Prompt excerpts: `user_texts`, truncated ~80 chars, redacted.
- Parsed-lines trust line: ingest counters.

## New pure derivations (core, unit-tested)

- `active_duration(actions, cap) -> (active, span)`
- Needs-review qualification (evidence floor) + reason-sentence builder with
  the fixed category vocabulary (churn=rewritten, re_read=re-read,
  rework=reworked, failure_loop=failure loop, fumbles=blind-write attempt,
  flip=flip, user_corrected=user-corrected)
- Run compression for story events
- Gap-glyph positions for the timeline
- Number formatting (thousands separators)

## README changes (this rework only)

New hero screenshot (regenerated via `scripts/render_demo_report.sh`). Hero
line and "Why I built this" rewritten around review targeting: which
agent-written code needs human eyes, backed by cited evidence. The token-ratio
chart is demoted to a supporting point. Install and Limitations untouched
until the strategy session resolves distribution and validation.

## Testing

Existing structural tests keep passing (doctype, self-containment, escaping,
determinism, lanes/ticks/bands, empty states). New tests: active-duration cap,
evidence-floor qualification (incl. the high-signal single-finding case),
reason-sentence vocabulary, run compression, calm-state rendering, redaction
applied to prompt tooltips, thousands formatting, status-bar trust line.
Screenshot script re-tuned after layout settles.

## Out of scope

Verified-after-last-edit signal (follow-up), distribution/packaging, the
strategy questions paused in the product brainstorm (anchor moment, validity
study, plugin distribution), MCP payload changes.
