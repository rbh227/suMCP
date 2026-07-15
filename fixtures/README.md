# Fixtures

Sanitized transcripts and mock payloads used as the parser/report test corpus.
Raw donor transcripts live in `raw/` (gitignored); only structure-preserving,
hand-reviewed output of `scripts/sanitize.py` is committed here.

| fixture | source | exercises |
|---|---|---|
| `session-2_1_210-subagents.jsonl` | sanitized donor, Claude Code 2.1.210 | 1682 lines; a **mid-session harness upgrade** (2.1.207→2.1.210); event types `permission-mode` + `queue-operation` (unknown-type counters); **337 untimestamped lines** and **89 identical-timestamp collisions** (ordering contract, amendment 5); streaming `message.id` duplicates (usage dedup); 12 subagent spawns referenced via `toolUseResult.agentId`; 14 MB largest-fixture cap/timing target |
| `edge-cases.jsonl` | hand-built | a non-JSON line (counted, never fatal); a duplicate `uuid`+`message.id` replay line (action dedup); an unknown nested type; an untimestamped `permission-mode`; a normal read→result→edit chain |
| `mock-payloads/*.json` | hand-built from example-app gate-1 findings | the six tool payloads + the fail-closed `ambiguous_session` error; the frozen v0 contract (see `docs/payload-schema.md`), enforced by `scripts/check_payloads.py` |
| `mock-payloads/sample-debrief.md` | live debrief (T0.2) | the <500-token narration contract, enforced by `scripts/check_debrief.py` |

## Regenerating a fixture

```bash
python3 scripts/sanitize.py raw/<session>.jsonl <name>.jsonl
```

The sanitizer is **default-deny**: it preserves the identity/skeleton signals
key off (ids, timestamps incl. absence and ties, usage numbers,
structuredPatch shapes, harness error strings) and scrubs everything else,
including content stored in JSON *keys*. It prints unrecognized keys to stderr
for review. Always leak-sweep before committing:

```bash
grep -oE "/(Users|home|media)/[A-Za-z0-9_.-]+" <name>.jsonl   # expect empty
```

### Not recoverable

The donor's 12 subagent child transcripts are **not** on this machine (its
`cwd` was `/media` on a remote box), so decision-2's "both subagent layouts"
still needs a locally-captured subagent session to fully exercise the merge.
