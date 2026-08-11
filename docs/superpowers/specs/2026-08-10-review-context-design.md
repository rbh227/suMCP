# Design: review context for reviewing agents

Date: 2026-08-10
Status: proposed, not yet planned or implemented
Supersedes: the review-brief / hunk-attribution direction explored earlier the
same day (never written to a spec; recorded here in "Rejected alternatives" so
the reasoning is not lost)

## Summary

suMCP becomes a deterministic supplier of recorded session context to a
reviewing agent, with the goal of **raising the precision of that agent's
findings**. It quotes what was asked, what was decided, what was abandoned,
and what was left unfinished, each with a citation into the transcript. It
renders no judgment of its own.

One line: *a deterministic local tool that reads Claude Code transcripts and
gives a reviewing agent the recorded context it needs to stop reporting things
that are not problems.*

## Why this, and why now

### The workflow being served

Claude Code writes and commits. A second agent (today, Codex via
`codex:adversarial-review`) reviews the commit. The reviewer sees the final
state of the code and nothing else. It cannot see what was asked, what was
deliberately chosen, what was tried and rejected, or what was knowingly left
undone.

### The constraint is precision, not recall

The project's earlier thesis was review targeting: tell someone *where to
look*. That thesis assumed the reviewer has a scarce attention budget, which
is true of a human and much less true of an agent that can read the whole
diff.

The measured constraint in the field is the opposite. In a study of 31,073
agentic review pairs across 10,191 pull requests, **56.3% of comments were
rejected** by developers, 36.4% accepted, 7.3% discussed. The stated rejection
reasons were false positives, redundancy, being **out of scope**, and being
**misaligned with what the developer intended**
([arXiv:2607.03316](https://arxiv.org/abs/2607.03316)).

Scope and intent are not properties of code. They are properties of the
conversation, and the conversation is on disk.

### The thesis is externally validated

Meta's ARCTIC ([arXiv:2607.29516](https://arxiv.org/html/2607.29516v1),
published 2026-07-31) derives intent from developer-AI conversation logs,
detects drift between intent and diff, and ranks regions for scrutiny. It ran
over 1 million API requests in production with 90.2% engineer approval of
intent predictions.

This removes novelty and supplies validity. The idea works. What ARCTIC does
not do:

- It is **not releasable**. The paper states the data "cannot be legally
  released even in an anonymized form." It is Meta-internal, on Meta
  infrastructure.
- It **infers** intent via LLM summarization, scoring 0.86 F1. Roughly one
  intent in seven is wrong. Its own stated construct-validity threat is
  reliance on LLM-as-a-Judge throughout.

suMCP's position is the complement: local, open, transcript-native, and
**quoting** ground truth rather than inferring it. This is the first point in
the project's life where the no-LLM commitment is an advantage rather than a
ceiling.

### The failure mode to design around

Supplying requirements to an LLM reviewer and asking it to verify conformance
induces **overcorrection**: the model assumes flaws exist and flags correct
code, and the effect worsens as prompts ask for more explanation and repair
([arXiv:2603.00539](https://arxiv.org/pdf/2603.00539); see also
[arXiv:2508.12358](https://arxiv.org/html/2508.12358v1)).

Two consequences, both binding on this design:

1. The full body of intent is **never pushed** into the reviewer's context. It
   is available behind a second call, pulled only when the reviewer is
   reasoning about something specific.
2. The default payload is framed as **constraints that rule findings out**,
   not as requirements to check conformance against. This pushes against
   overcorrection rather than into it.

## What ships

### Tool 1: `review_context(commit_range)`

The default payload. Small, pushed once at the start of a review.

| block | source | label |
|---|---|---|
| `scope` | the human's request, and the files actually touched in the sessions covered | exact |
| `decisions` | `AskUserQuestion` question, options, and the option chosen | exact, structured |
| `constraints` | approaches tried and abandoned, with the recorded failure | **heuristic** |
| `incomplete` | tasks created but never completed; test commands failing at session end | exact |
| `claims` | agent prose asserting what it did, for verification against the diff | exact extraction |

Every entry carries the action indices that prove it, so `evidence(idxs)`
(already built) resolves any of them to a raw transcript excerpt.

Extraction rules, all deterministic:

- **scope**: `type: user`, `isMeta: false`, `origin.kind == "human"` message
  text. Verbatim. No summarization. The file list is the set of paths actually
  acted on by the sessions in range, not an inference about what the request
  "meant" to cover.
- **decisions**: the `AskUserQuestion` `tool_use` input (questions, options,
  labels) joined by `tool_use_id` to its result (the selection). Fully
  structured in the transcript; nothing is inferred.
- **incomplete**: `TaskCreate` / `TaskUpdate` calls replayed to a final state
  per task. Any task not reaching `completed` is reported. Plus Bash actions
  matching the existing test/build regexes whose last invocation carried
  `is_error: true`.
- **claims**: the last assistant `text` block before each human turn.
  Extracted, never interpreted.
- **constraints**: the only heuristic block. Detected from existing signals:
  `true_revert` (content changed and changed back), files created then
  deleted, and errored Bash commands not subsequently repeated. Carries the
  repo's standard heuristic label and confidence, exactly as
  `signals/comprehension.rs` does today.

### Tool 2: `session_intent(commit_range, [max_tokens])`

The full verbatim human messages for the range, roughly 17k tokens on a large
session. Never pushed; pulled on demand. Exists so the reviewer can get depth
when reasoning about a specific hunk, without paying the overcorrection cost
of having it resident from the start.

### Tool 3: `evidence(idxs)`

Unchanged, already built. The drill-down path for every citation above.

### The invariant

> suMCP never asserts that something is acceptable. It reports what was
> recorded, verbatim, with a citation. The reviewing agent decides what that
> implies.

This is what keeps the tool out of the judgment business. It is also what makes
determinism a feature: there is nothing to calibrate, no threshold to tune, and
no weighting to defend.

## The project layer

Cross-session memory, aimed at precision rather than at hot-spot ranking:

> Remember which review findings were rejected, and suppress their recurrence.

When a reviewer raises a finding and the human dismisses it, that dismissal is
durable knowledge. It compounds with every review, it is uniquely available to
a tool sitting in this position, and it attacks the 56% directly.

Two things make this cheap:

- **Backfill, not accumulation.** A project's entire history re-derives from
  transcripts in about 1 second (measured: 35 transcripts / 44 MB in 0.99 s;
  29 transcripts / 52 MB in 0.81 s). There is no cold start.
- **No database.** Because re-derivation is that cheap, the transcripts are
  the store. No schema, no migrations, no staleness policy, no retention
  threat model. This is the architectural work the Rust choice actually does:
  in an interpreted language, re-derivation would take tens of seconds, which
  forces a persistent store and everything that comes with it.

The one thing that must persist is the **raw transcripts**, which Claude Code
deletes after 30 days. `scripts/archive_corpus.py` already does this.

**Nothing in this section is in the first build.** Rejected-finding memory is
the intended next step *after* the precision result comes back positive, and
its capture mechanism is still unresolved (see "Open questions"). A per-region
friction map is deferred further still. This section exists to record where the
design is heading, so the first build does not foreclose it, not to widen the
first build.

## Validation

Two arms over commits that already exist in the author's own repositories,
which makes the sample retrospective rather than accumulated forward.

- **Arm A, blind.** Codex adversarial review of the commit, diff only.
- **Arm B, contextualized.** Same commit, with `review_context` available.

Each finding is adjudicated valid or invalid, with invalid subdivided into
false positive, redundant, out of scope, and misaligned with intent, matching
the taxonomy in arXiv:2607.03316 so the result is comparable to its 56.3%
baseline.

Declared before any run, following the discipline in
`scripts/validity_sweep.py` that made the earlier negative result credible:

- **Primary metric**: the invalid share of findings in arm B versus arm A,
  with a 95% confidence interval.
- **Secondary metric**: true positives found in A but missed in B. This is the
  tunnel-vision cost and it is reported whatever it says.
- All secondary breakdowns reported in full regardless of how the primary
  comes out.

### Kill criteria, agreed in advance

- Arm B's invalid share is not lower than arm A's, CI including zero
  difference. The context does not improve precision. Stop.
- Arm B loses true positives that arm A found, at a rate exceeding the
  precision gain. The tool makes review net worse. Stop.
- Adjudication cannot be done consistently. The experiment cannot answer the
  question and must be redesigned before more code is written.

### Sample size

Flagged as the weakest part of the plan. Findings per commit are sparse and
Codex reviews are slow, so twenty commits will very likely produce intervals
too wide to conclude anything, exactly as the 39-outcome corpus did in
`docs/validation/2026-07-22-predictive-validity.md`. A power estimate is
required **before** implementation, not after results are in hand. The
mitigation is retrospective breadth: 176 commits in this repository alone,
plus other projects, all with transcripts still on disk or archived.

## Architecture

- `sumcp-core`: a new `context` module performing the five extractions above.
  It reads the existing `Session` model and adds no new parsing.
- `sumcp-core/src/ingest.rs`: extended to retain `AskUserQuestion` and
  `TaskCreate` / `TaskUpdate` tool inputs and results, which are currently
  parsed as ordinary tool calls and discarded.
- `sumcp-mcp`: two new tools registered alongside the existing six.
- `sumcp-cli`: `sumcp context <range>` emitting the same payload, so the
  feature works without any MCP wiring on the reviewer's side.

Git is needed for exactly one thing: resolving a commit range to the timestamps
that select which sessions are in scope. That is two `git log` invocations for
commit times, isolated in a small `git` module with no other responsibility.
There is **no diff parsing and no content matching**, which is what the
rejected hunk-attribution design would have required. The payload is
session-scoped throughout; the commit range is only a time selector.

The session-to-commit mapping is imperfect by nature and its failure mode is
disclosed in the payload rather than hidden, the same way `work_unit` grouping
already discloses itself (see "Open questions").

## Testing

Following the repo's existing TDD practice, with two additions:

- **Extraction fixtures** for each of the five blocks, including the awkward
  cases: an `AskUserQuestion` the user answered with "Other" free text, a task
  updated several times before its final state, an interrupted session whose
  last claim block is truncated.
- **A recount-style gate.** The repo's independent naive recounter caught a 3x
  undercount that 271 green tests missed. Extraction gets the same treatment:
  a second, deliberately simple extractor whose output must agree exactly on
  counts of decisions, incomplete tasks, and claims.

## Rejected alternatives

- **Hunk-level attribution of transcript edits to diff lines.** Designed in
  detail, then dropped. It was infrastructure for the churn and blind-write
  signals, and those signals are weak: "written blind" is close to meaningless
  when all authorship is agentic. The valuable material is session-scoped and
  needs none of this machinery. Recorded because if the precision result comes
  back positive and per-region history becomes worth building, this is the
  approach to revisit, along with its two known limits (`EDIT_CAP` truncating
  edit text at 2000 chars, and rebase or amend breaking the mapping).
- **A weighted risk score over the extracted context.** Rejected for the same
  reason the ranking weights were deleted on 2026-07-26: it introduces a knob
  with nothing to calibrate it against, and the consumer is a language model
  fully capable of weighing raw facts itself.
- **Summarizing intent instead of quoting it.** This is ARCTIC's approach and
  it scores 0.86 F1. Quoting scores 1.0 and requires no model. Summarization
  would trade the project's one genuine advantage for a worse version of a
  competitor's feature.
- **Pushing full intent into the reviewer's context by default.** Rejected on
  the overcorrection finding above.

## Open questions

- **Scope block boundaries.** A commit range maps to sessions by timestamp,
  but a session can span several commits and a commit can span several
  sessions. The mapping rule needs to be stated explicitly and its failure
  mode disclosed in the payload, the way `work_unit` grouping already is.
- **Claim extraction volume.** 60 prose blocks in one measured session,
  totalling 51k characters. Not all are claims worth verifying. Selecting
  among them is a judgment this design has committed to avoiding, so the first
  version reports all of them with a count and lets the reviewer choose. If
  that proves unusable, the selection rule becomes a real design problem
  rather than an afterthought.
- **Rejected-finding capture.** Recording a dismissal requires the human to
  signal it. Whether that is a CLI call, a hook, or parsing the review thread
  is unresolved and should be settled before that part is built.

## Risks carried forward

- **The transcript format is undocumented and owned by Anthropic.** Stated in
  SPEC.md already. Any release can break the parser, and there is no
  mitigation available to this project.
- **This is absorbable.** A platform owner could ship it natively with a
  documented format and better access. The reasons to build it are that it is
  useful to its author now and demonstrates unusually careful engineering, not
  that it establishes a defensible category position.
- **Precision improvement may not be attributable.** If arm B improves, it
  could be the context or it could be that any additional context shifts the
  reviewer. A third arm supplying irrelevant context of similar length would
  settle it, at additional cost. Deferred, and recorded here so it is a choice
  rather than an oversight.

## References

- [From Code Review to Code Critique: Intent, Drift, and Spotlight for AI-Generated Diffs at Scale](https://arxiv.org/html/2607.29516v1) (ARCTIC, Meta, 2026-07-31)
- [Is Agentic Code Review Helpful? Mining Developers' Feedback to CodeRabbit Reviews in the Wild](https://arxiv.org/abs/2607.03316)
- [Are LLMs Reliable Code Reviewers? Systematic Overcorrection in Requirement Conformance Judgement](https://arxiv.org/pdf/2603.00539)
- [Uncovering Systematic Failures of LLMs in Verifying Code Against Natural Language Specifications](https://arxiv.org/html/2508.12358v1)
