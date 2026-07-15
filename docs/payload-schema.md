# suMCP payload schema v0 (T0.1 — frozen at Checkpoint A)

The contract for what the six MCP tools return. Canonical examples live in
`fixtures/mock-payloads/` and are enforced by `scripts/check_payloads.py`
(token cap ≈ chars/4, required fields, provenance). The Rust `report.rs`
builders must produce payloads that pass the same checker.

Format is **compact JSON** (ADR A5): agents parse it reliably, snapshot tests
diff it, caps are enforceable by construction. The tool returns evidence; the
connected agent narrates.

## Envelope (every non-error payload)

| field | contents |
|---|---|
| `v` | payload schema version, `0` |
| `session.id` | session uuid |
| `session.identified_by` | **provenance, ADR A4**: `tool_use_id` (verified self-identification), `explicit` (caller passed session_id), or `cli_latest` (CLI-only recency mode). MCP never emits a guess. |
| `truncated` | `true` whenever any cap trimmed content |

## Finding shape

Every finding-like object (anything with a `kind`) carries:

```json
{"kind":"rework","tier":"T2","exact":true,"confidence":"high","idxs":[102,141]}
```

- `kind` — churn | rework | failure_loop | thrash | fumble | blind_write_attempt |
  true_revert | flip | user_corrected | write_no_reread | read_unreferenced |
  large_write_instant_accept | opening_move
- `tier` — field-reliability tier T1–T3 (metrics-spec parser rules)
- `exact` — `true` = deterministic count; `false` = heuristic (attribution,
  latency); heuristics also carry a human-readable `note`
- `confidence` — high | medium | low (low counts ×0.5 in ranking)
- `idxs` — action indices proving the finding, dereferenceable via `evidence()`

## Tools, caps, truncation rules

| tool | cap (tokens) | truncation rule |
|---|---|---|
| `session_overview` | 1000 | fixed shape; `top_struggles` capped at 3 |
| `struggle_areas(n)` | 1500 | files capped at n, findings per file capped (`findings_per_file_cap`), tail-first |
| `file_story(path)` | 1500 | **middle-out**: head + tail kept, middle elided with `elided:{count,between}` marker |
| `blind_spots` | 1000 | each list tail-truncated |
| `context_health` | 1000 | `read_never_referenced` sampled, total count always present |
| `evidence(idxs)` | 1500 | ≤10 actions, excerpts ≤600 chars |

All ranking output shows the per-category `breakdown` and the `weights` used
(`source: defaults` or the config path) — never an opaque score.

## Error payload (fail-closed, ADR A4)

```json
{"v":0,"error":"ambiguous_session","message":"...","candidates":[{"id":"...","mtime":"...","cwd_match":true}],"hint":"pass session_id"}
```

Emitted when self-identification cannot verify the caller and no explicit
`session_id` was given. Listing candidates lets the agent recover in one turn.

## Suppression (heuristic honesty)

`blind_spots.suppression` reports whether approval-latency metrics are active;
when `permissionMode` grants auto-accept they are suppressed entirely rather
than reported as meaningless numbers.

## Versioning

`v` bumps on any breaking shape change; the checker and mock payloads update
in the same commit (they are the contract test).
