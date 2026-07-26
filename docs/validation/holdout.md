# Holdout freeze

Status: DRAFT, internal.

## Membership is frozen by fingerprint

The held-out projects are recorded as one-way fingerprints in
`docs/validation/holdout-snapshot.json` (`sha256(project path)`, first 16 hex
chars). That file is the authority; `scripts/validity_sweep.py` resolves it at
run time.

Two projects are frozen, matching the membership the original 2026-07-22
selection produced against the corpus as it stood at freeze time:

- one 1-session project (was proj-02)
- one 12-session project (was proj-05, now proj-04 after renumbering, and
  still correctly held out because resolution is by fingerprint)

The real-path to anonymized-id mapping is never committed. It lives only in
`.superpowers/sdd/validity-holdout-map.json` (scratch).

### Absent roster entries

A frozen project can legitimately leave the corpus (a project whose only
session drops below `MIN_ACTIONS` disappears). That is **not** contamination:
an absent project is in neither split, so it can leak nothing. The run
proceeds, records the absent fingerprints in the raw output, and prints them.
The entry stays in the snapshot, so the project is held out again
automatically if it ever returns.

The 1-session entry is currently absent. Note that it never contributed a
single pair even when present: `build_pairs` excludes each project's last
session (no window successor exists), so a 1-session project yields zero pairs
by construction. Its absence changes no number that was ever computed.

The run **does** fail closed when *no* frozen project resolves, because an
empty holdout would silently turn a held-out evaluation into a no-op that
still looks like it ran.

### Why not "sorted ids, indices 1 and 4"

The original rule sorted the anonymized ids (`proj-01..proj-NN`) and held out
positions 1 and 4. Those ids are assigned by sorting the *real* project names,
so adding a project that sorts earlier renumbers every project after it. The
rule was frozen but the **membership was not**: growing the corpus would
silently swap which projects were held out, contaminating the tune split with
previously held-out data and invalidating any earlier held-out evaluation.

A holdout guarantee is about membership, not about the procedure that first
produced it. Fingerprints make membership immutable under corpus growth.

## Development runs never see held-out outcomes

The tune/held-out split happens *before* any metric is computed. A normal run:

- computes every aggregate on the tune split only
- writes only tune-split pair records to `validity-raw.json`
- reports held-out projects by id and a withheld **count**, never their
  labels, outcomes, or aggregates

Previously the script computed metrics over all pairs and filtered afterwards,
then wrote every pair (with outcomes) plus the held-out ids and the
de-anonymization mapping into the same file. Held-out outcomes were therefore
recoverable on every development run, which defeats the once-per-release rule
no matter what the rule says.

## Scoring the holdout

Once per release gate, and deliberately never as a side effect of ordinary
work:

```
python3 scripts/validity_sweep.py --release-eval
```

This writes `.superpowers/sdd/validity-heldout-eval.json`. Any tuning must be
finalized on the tune split *before* this is run.
