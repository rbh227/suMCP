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

### The whole thing in four paragraphs

**What we build.** A local binary that gives a reviewing agent five things
about a commit, quoted verbatim with citations: what was asked, what was
decided (including the options it beat), what was tried and failed, what was
left unfinished, and what the agent claimed it did. Two MCP tools and a CLI
command. No diff parsing, no scoring. Later, gated on the result: the same
material accumulated into a per-file project memory, plus every review finding
the human dismissed, read by the reviewer to avoid repeating itself and by the
builder before it edits.

**The design.** suMCP retrieves; the agent judges. It never says "this is
fine" or "this is risky," only "here is what was recorded, at this index."
That is why it needs no LLM, has nothing to calibrate, and can be wrong about
nothing except whether it found the right passage.

**Why it works.** A reviewer holding only the diff can check whether code is
internally consistent, but not whether it does what was wanted, because it
infers the intent from the code under review. Meta proved that fixing this
works at production scale using exactly this data source. The measured pain is
not missed bugs but false alarms: 56.3% of agentic review comments are
rejected, mostly for being out of scope or misaligned with intent. Neither is
a property of code; both are written down in the transcript. Meta must infer
intent and is right 86% of the time. Quoting is right always.

**What would kill it.** It is absorbable by whoever owns the transcript
format, which is not this project. And it is unproven that this moves a real
reviewer's precision, which is why the first build is an experiment on the
author's own commits with kill criteria fixed before it runs, not a launch.

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

## The project memory layer (designed, deferred)

**Nothing in this section is in the first build.** It is designed here so the
first build does not foreclose it, and gated on the precision result: durable
storage for a signal that does not work is worse than no storage. Read this as
direction, not scope.

### What it holds

A per-project index keyed by **file path**, holding four kinds of entry, all
already extracted by the first build:

| entry | why it is worth remembering |
|---|---|
| `DECIDED` | a recorded human choice and the options it beat |
| `REJECTED` | a review finding the human dismissed |
| `TRIED` | an approach attempted and abandoned, with the recorded failure |
| `HISTORY` | session and revert counts for the file |

```
crates/sumcp-core/src/score.rs
  DECIDED    2026-07-26  weights removed in favour of a fixed rule
             (rejected: keep weights, tune on holdout)   [session 3145f2f3]
  REJECTED   2026-07-28  reviewer flagged "ranking rule is arbitrary";
                         dismissed, see validation doc    [review r-014]
  HISTORY    4 sessions, 2 reverts
```

`REJECTED` is the entry with no substitute anywhere else. A dismissal is
durable knowledge, it compounds with every review, and it attacks the measured
56% rejection rate directly.

### It is an index, not a graph

Every entry above is keyed by location, and every query is "what do I know
about this file?" That is a dictionary. A graph earns its complexity only when
a query needs multi-hop traversal, and no such query has been identified. If
one appears later, edges can be added to an index; an unused graph cannot be
simplified back down. Build the index.

### Relevance is delegated, not computed

The hard problem is not storage, it is knowing when a stored entry applies.
"Is this the same objection the human rejected in July?" cannot be answered by
exact string match, and answering it semantically would require embeddings,
which would mean a model inside suMCP and the loss of its one advantage over
ARCTIC.

The invariant resolves it. suMCP retrieves by **exact key** (this file, later
this region), returns the small complete set it holds for that location, and
the consuming agent decides which entries bear on the finding in front of it.
That judgement is trivial for a model already reading both the code and the
finding, and impossible to make deterministically. No embeddings, no
similarity search, no model, and the memory is still not dumb.

### Consumers

The same index serves both agents, which is the point:

- **the reviewer**, to suppress objections already settled
- **the builder**, queried before it edits a file, to avoid re-litigating
  decisions and re-attempting known failures

The builder-side consumer is the lower-friction product (it needs no workflow
change and no second agent) and is also the only place in this design where
the Rust choice is load-bearing: a `PreToolUse` hook fires synchronously on
every edit, and 27 ms of interpreter startup would consume most of the latency
budget before any work began (measured: 2.3 ms for the Rust binary, 26.5 ms for
`python3 -c pass`, 27.6 ms for `node -e ''`).

### Storage and growth

Transcripts are **append-only and immutable once a session ends**, so each is
processed once and never re-processed. Corpus growth (measured at roughly
1 GB/year: 154 MB live plus 252 MB already archived across 2,141 files)
therefore does not force a re-derivation cost, in any language.

The one thing that must persist is the **raw transcripts**, which Claude Code
deletes after 30 days. `scripts/archive_corpus.py` already does this.

File paths change, so the index must follow renames. `git log --follow` is the
mechanism.

### Open before this is built

- **Capture of `REJECTED`.** Recording a dismissal requires the human to signal
  it. Whether that is a CLI call, a hook, or parsing a review thread is
  unresolved and must be settled first.
- **Region-level keying.** File-level is the first version. Region-level is
  strictly better and needs durable region identity, for which `git log -L`
  (measured at 160 ms on this repository) is the mechanism rather than
  something to invent.

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

## Language choice, stated honestly

An earlier draft of this spec claimed that Rust "buys the ability to have no
database," because re-derivation would take tens of seconds in an interpreted
language. **That claim was measured and is false.** A Python implementation of
this spec's full extraction, over an entire project, runs in 0.34 s (35 files,
44 MB) and 0.40 s (29 files, 52 MB). The relevant content is roughly 1% of a
transcript, which is why extraction is cheap in any language.

The claim is withdrawn rather than quietly softened, because the same
overclaiming is what produced the earlier "8.9x lift" that a one-line rule
matched.

What actually justifies Rust here, in order:

1. **Distribution.** A single static binary with no runtime, no virtualenv, and
   no dependency resolution. This matters *more* in this design than any
   previous one, because the consumer is an external agent: registering a
   self-contained binary in another tool's MCP config is one line and cannot
   break on an interpreter version.
2. **The deferred builder-side hook.** Cold start of 2.3 ms against 26.5 ms
   (`python3 -c pass`) and 27.6 ms (`node -e ''`) is decisive only for
   something that fires synchronously on every edit. That is the memory
   layer's builder consumer and nothing else in this spec.
3. **The codebase already exists**, is well tested, and rewriting it would be
   irrational.

What does **not** justify Rust: throughput, corpus scale, or the no-database
architecture. The one workload in this project that genuinely needed Rust's
speed was content-matching edit fragments against a diff, and that workload was
deliberately dropped (see "Rejected alternatives").

Rust should therefore be treated as an implementation detail and a
distribution property, not as a product claim. Every time it has been pitched
as a performance advantage in this project's documentation, the benchmark has
failed to support it.

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
- **A memory *graph*.** Every memory entry identified is keyed by location and
  every query is "what do I know about this file," which is a dictionary. No
  multi-hop traversal query has been identified to justify edges. An index can
  gain edges later; an unused graph cannot be simplified back down.
- **Semantic retrieval (embeddings) for memory relevance.** It would answer "is
  this the same objection as before" well, and it would put a model inside
  suMCP, forfeiting the only advantage this design holds over ARCTIC.
  Relevance is delegated to the consuming agent instead.

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
Questions belonging to the deferred memory layer (rejected-finding capture,
region-level keying) are recorded in that section rather than repeated here.

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
