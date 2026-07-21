# v0.2 idea — cross-session memory / context graph

Captured 2026-07-21 during T5.2 brainstorm. Parked until v0.1 external
validation (T5.3) confirms the single-session signals are trustworthy.

## The idea (Raphael's words, paraphrased)

Track session data consistently across an *entire project* — not one
transcript at a time — and accumulate it into a context/memory graph of the
results and overall data. Today every suMCP output is single-session: one
transcript in, one debrief out, nothing remembered between sessions. This idea
is the L4 cross-session altitude the SPEC left as an open seam
("persistence (SQLite, for L4 cross-session)").

## Why it's promising

- The data model was deliberately built for it: monotonic `Idx`, `agent_id`,
  a `Report` type, and a byte-offset ingest param already exist for later
  real-time / incremental use.
- It answers questions a single session can't: "you keep re-struggling with the
  same file across 5 sessions," "this project has a recurring revert pattern in
  auth code," "your context health degrades every session past ~2h."

## Two tensions to respect (why it's v0.2, not now)

1. **It's built on an unvalidated base.** A cross-session graph is an
   accumulation *of the single-session signals*. Until strangers confirm those
   signals are true (T5.3, the release gate), a graph just compounds noise
   across sessions while looking authoritative. Validate the atoms first.
2. **"Graph of results" drifts toward the crowded non-moat category.** The SPEC
   warns that live dashboards / viewers are crowded and non-moat. The moat is
   the deterministic, evidence-cited *signal extraction*. So the valuable part
   of this idea is **new cross-session signals** (recurring struggle, drift,
   cross-session revert loops), NOT a rendered graph. Rendering is a thin view;
   the signals are the product.

## Open design questions for when we pick this up

- Persistence: SQLite (the specced seam) vs append-only JSONL per project.
- Identity across sessions: project dir is stable; how to stitch resumed
  sessions and subagent lanes into one project timeline without double-count.
- What are the actual cross-session *signals*? (name 3–5 before building storage)
- Privacy: cross-session storage is a bigger data-retention surface than the
  current read-only, in-memory model — needs its own threat model.
- Feed it with real data: T5.3 volunteer transcripts are the first corpus to
  design these signals against, instead of guessing in a vacuum.
