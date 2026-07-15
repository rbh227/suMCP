# suMCP v0.1 — Task List

Source of truth for detail: `tasks/plan.md`. Check items off as they land.

## Phase 0 — Contract validation (no Rust)
- [x] T0.1 Freeze Report/payload JSON shapes (`docs/payload-schema.md` + `fixtures/mock-payloads/`, enforced by `scripts/check_payloads.py`)
- [x] T0.2 Debrief skill vs mock payloads, live-session test (~344/500 tokens, 3 files, 10 citations)
- [x] CHECKPOINT A — payload schema frozen at v0, narration contract proven (2026-07-15)

## Phase 1 — Foundation
- [x] T1.1 rustup + workspace scaffold (Rust 1.97; core/cli/mcp crates; fmt/clippy/test green; rmcp deferred to T4.1 to avoid pre-1.0 churn)
- [x] T1.2 `scripts/sanitize.py` (default-deny, scrubs keys+values) + fixtures (sanitized 2.1.210 donor, edge-cases); `check_sanitizer.py` canary test. NOTE: only 2.1.210 + edge fixtures so far (no local 2.1.56/183 donor); subagent child files unrecoverable (donor cwd was remote /media)

## Phase 2 — First vertical slice
- [x] T2.1 locate + ingest + model + overview + `sumcp --file` CLI (14 tests; runs on real fixture: 258 actions, 97% cache hit, 337 untimestamped handled; deterministic across runs). Deferred within T2.1: is_error wiring to tool_result, external-file-ref following, full A9 resource caps (next tasks)
- [~] CHECKPOINT B — parse gate: fixture parses clean + deterministic + unknown types counted. ccusage cross-check still TODO (ccusage reads ~/.claude, not arbitrary fixtures; verify on a live local session)

## Phase 3 — Signals
- [x] T3.1 Edit-shape signals: churn, rework (patch-hunk overlap), re-read thrash, blind-write attempts. Finding/Tier/Confidence types added; ingest now joins tool_result error text + structuredPatch hunks back to actions. Fires on real donor with evidence. (large single-shot writes moved to T3.4 where it pairs with approval latency → large_write_instant_accept, staying within the frozen payload enum)
- [x] T3.2 Failure signals: failure_loops with 4-step attribution chain (path-in-output → path-in-command → last-edit-within-5 → unattributed/dropped) + confidence tiers (PathMatch=High, Proximity=Low); tool_error_rates + validation_share helpers. Attribution matches only session-touched files (ADR A9, no fs access)
- [ ] T3.3 Dynamics signals (revert/flip/user_corrected, opening move, interruptions, loops)
- [ ] T3.4 Comprehension signals (approval latency, large-write-instant-accept)
- [ ] T3.5 Weights config + transparent ranking + six payload builders with caps
- [ ] CHECKPOINT C — Report reproduces gate-1 Python findings; weights echoed in payloads

## Phase 4 — MCP server
- [ ] T4.1 rmcp stdio server: six tools, memoized re-parse, session self-identification
- [ ] CHECKPOINT D — live DoD: debrief on real tools, 3 files, evidence, <500 tokens

## Phase 5 — Packaging & external validation
- [ ] T5.1 Static HTML report (`sumcp report --html`, self-contained, zero network)
- [ ] T5.2 `sumcp install`/`uninstall` + Stop hook (fresh-home test)
- [ ] T5.3 External validation (2–3 volunteers) + measured token ratio → README
- [ ] T5.4 OSS readiness (LICENSE, README, CONTRIBUTING, CHANGELOG, docs/metrics.md, rustdoc, issue templates)
- [ ] CHECKPOINT E — v0.1.0 tag (publishing = ask first)

## /ship review findings — RESOLVED 2026-07-15
Privacy leak (real project name/filenames in mocks+plan) scrubbed to synthetic;
both contract checkers hardened (kind enum, non-empty idxs, note-when-heuristic,
full error shape, ranking weights, chars/3.5; debrief idx cross-dereference,
real signal vocab). SPEC amendment 5 (untimestamped-event total-ordering) and
ADR A9 (untrusted-input allowlist: UUID/path validation, external-ref allowlist,
resource caps, secret redaction) added. Remaining ship items folded into task
acceptance: default-deny sanitizer (T1.2), resource-cap + path fixtures (T2.1),
HTML secret redaction (T5.1), cargo-audit CI (T4.1), SECURITY.md (T5.4).

## Codex review findings (both rounds) — RESOLVED 2026-07-15
All six folded into SPEC.md and tasks/plan.md: installer write contract (A8),
fail-closed session identity (A4), total ordering contract for subagent merge
(decision 2), action-level replay dedup (SPEC §1 amendment 2), approval
latency downgraded to explicit heuristic with suppression rules (decision 5),
external release gate without the tuning escape hatch (SPEC §6, T5.3).
