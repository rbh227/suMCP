# Error autopsy: misses and false alarms (tune-split only)

Status: DRAFT, internal. Held-out projects (proj-02, proj-05; see
`docs/validation/holdout.md`) are excluded from every count below. Frozen
default weights throughout; no tuning performed. Flag definition:
`flagged_nr` (`review::needs_review`). Outcome: strong. Window: next 3
sessions. Sampling: sorted by (project, session, file), first 25 taken.

## Corpus for this pass

Tune-split (session, edited-file) pairs: 473 (4 projects: proj-01, proj-03,
proj-04, proj-06). Of these:

- MISSES (outcome=1, flagged_nr=0): 14 total, all 14 sampled (fewer than 25 exist).
- FALSE ALARMS (flagged_nr=1, outcome=0): 34 total, 25 sampled.

## Misses: 14 sampled, by label

| label | count |
|---|---|
| below_floor | 8 |
| genuinely_quiet | 5 |
| no_signal_computed | 1 |

### below_floor (8)

Two distinct sub-mechanisms showed up, both worth separating for the
detector-idea ranking below.

**High edit-count, single-category churn (3 of 8).** Three files were
rewritten 8-10 times in session N with the *only* recorded finding kind
being churn (no accompanying rework/re_read/etc.), so `findings.len() == 1`
and churn alone is not a solo-qualifying kind -- the file never clears
`needs_review`'s floor no matter how many times it was rewritten. All three
recurred strongly (as `flagged_nr` review candidates) in a later session.
This is a genuine floor gap, not a data problem: heavy same-file churn with
no co-occurring signal is exactly the case the floor was not built to catch.

**Qualifying findings excluded by the 3-file cap (2 of 8).** Two files had
2+ distinct finding kinds (churn + rework) and would satisfy the floor on
their own, but the session already had 3 higher-ranked files fill
`needs_review`'s fixed cap before them. Both recurred strongly next session.
This is a capacity limit, not an evidence-quality problem.

**Genuinely below the floor (3 of 8).** Three files had exactly one
low-count churn finding (2-3 edits) in an otherwise quiet session (well
under the cap), and legitimately did not have enough evidence to qualify.
One of these (a reference doc under a skills plugin) was also never
verified after its last edit (`last_edit_verified: false`).

### genuinely_quiet (5)

Five misses trace to single, verified, zero-finding edits (`edits: 1`,
`kinds: []`, `last_edit_verified: true`) that later became busy files
again. Two clusters: a batch of backend scaffolding files (a websocket
handler, a server entrypoint) each touched once during a large multi-file
build-out session, then legitimately reworked 2-4 sessions later as the
same feature continued -- this reads as healthy iterative development, not
a struggle recurring, and it is the strong-outcome proxy (any `needs_review`
recurrence counts, not just literal bug repeats) that is conflating the
two. The other cluster is single quiet touches to files that later racked
up a recognizable failure pattern (repeated blind-write attempts on the
same constants file across three separate sessions) -- interesting as a
cross-session behavioral note, but not something session N's own evidence
could have flagged on a file's first appearance.

### no_signal_computed (1)

One memory/notes file was edited once, never re-read or exercised by a
follow-up command in the same session (`last_edit_verified: false`), and
was then heavily reworked in two later sessions. This is the one sampled
miss the unverified-ending field (Task 3 below) would concretely have
caught: a detector on "edited once, never verified, in a document/notes
class of file" -- exactly the case the hypothesis test targets, though at
n=1 in this sample it is anecdotal, not evidence on its own.

## False alarms: 25 sampled, by label

| label | count |
|---|---|
| resolved_verified | 16 |
| activity_artifact | 5 |
| resolved_unverified | 3 |
| window_too_short | 1 |
| other | 0 |

**resolved_verified (16, the majority).** Most sampled false alarms are
files that legitimately struggled in session N (churn/rework/re_read, one
with a true self-revert, one with a genuine failure loop) and were then
followed by a Read, a passing build/test command, or a command referencing
the file, with no further trouble afterward -- `last_edit_verified: true`
in all 16. Read narrowly, precision (0.23-0.25) undercounts the detector's
practical value: a large share of "false alarms" are struggles that were
caught, worked through, and confirmed fixed within the same session, which
is a legitimate use of the flag even though the strong-outcome metric
scores it as a non-event.

**activity_artifact (5).** All five are non-code files -- a planning doc, a
skill instruction doc, two running notes/memory files, and a `.env`
config -- where churn+rework reflects mechanical drafting or config-tweak
iteration rather than a correctness struggle. None recurred.

**resolved_unverified (3).** Struggle (churn/rework, one with re_read) that
simply stopped without any following readback or command referencing the
file, and never recurred in the remaining project history. Plausibly fine
in practice, but unconfirmed by session-N evidence alone.

**window_too_short (1).** One file (a frequently-touched UI entrypoint) was
flagged in session N (churn+rework, `last_edit_verified: true`), showed no
strong signal in the next 3 sessions, then recurred strongly in exactly the
4th following session -- one session past the window's cutoff. A single
example, but a clean one: the 3-session window is a real, visible source of
false alarms at the margin, not just noise.

## Detector ideas ranked by misses caught (of the 14 sampled)

1. **Solo-qualify high-count churn** (e.g. churn count above a threshold,
   with no co-occurring finding required) -- catches 3 of 14 sampled misses
   outright (the high-edit-count churn-only files). A looser threshold would
   catch more (up to 6 of 14, folding in the low-count churn-only misses
   too), at an unmeasured cost to false-alarm precision that would need its
   own predict-then-check pass on held-out data before adopting.
2. **Raise or remove the fixed 3-file review cap** when more than 3 files
   already qualify under the existing floor -- catches 2 of 14 sampled
   misses (both had 2+ qualifying findings but lost the cap to
   higher-ranked files).
3. **Unverified-final-edit flag** (a file edited once with no findings, and
   never read back or exercised by a command before the session ended) --
   catches 1 of 14 sampled misses directly; independently investigated at
   the population level in the hypothesis test below.

## Task 3: the unverified-ending hypothesis

Extended `crates/sumcp-core/examples/validity_dump.rs` with a per-file,
analysis-only `last_edit_verified: bool` field (documented in the source as
a study heuristic, not a product signal): true when, strictly after the
file's last Edit/Write action, the session contains a Read of that file, a
non-failing Bash action whose command contains the file's basename, or a
non-failing Bash action matching a common test/build runner (`cargo test`,
`cargo build`, `pytest`, `npm test`, `npm run build`, `go test`, `make`).
Rebuilt, deleted and regenerated `.superpowers/sdd/validity/`, and extended
`scripts/validity_sweep.py` with a tune-split-only table (strong outcome,
next-3-sessions window) that never touches `render_draft`/the main report.

Unverified-ending hypothesis table (tune-split pairs: 473):

| last_edit_verified | n | RR | precision | a | b | c | d |
|---|---|---|---|---|---|---|---|
| True | 346 | 7.27 | 0.26 | 9 | 26 | 11 | 300 |
| False | 127 | 10.55 | 0.27 | 3 | 8 | 3 | 113 |

Standalone signal (unverified vs. verified, ALL edited files, `flagged_nr`
ignored):

| n | RR | precision | a | b | c | d |
|---|---|---|---|---|---|---|
| 473 | 0.82 | 0.05 | 6 | 121 | 20 | 326 |

**Reading.** Within already-flagged files, verification status barely moves
precision (0.26 vs 0.27) and the RR difference (7.27 vs 10.55) sits on cells
too small to trust (only 14 flagged-and-unverified pairs total, a=3/c=3).
Standalone, the field points the *wrong* way (RR 0.82, precision 0.05):
files whose last edit went unverified recur *less* often than verified
ones, most likely because "verified" here is picking up on busy, actively-
developed files (which get test runs simply because they are being worked
hard) rather than on correctness confidence, confounding the very effect
the hypothesis predicted. Net: this pass does not support building a
detector on `last_edit_verified` as defined; the one miss it would have
caught in the sample above is better read as anecdotal than as validated
signal.

Determinism: two consecutive `scripts/validity_sweep.py` runs produced a
byte-identical `hypothesis_unverified_ending` block in
`.superpowers/sdd/validity-raw.json`, and the main report
(`docs/validation/2026-07-22-predictive-validity.md`) was unchanged by
either run (verified via `git status`).
