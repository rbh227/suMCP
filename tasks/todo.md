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
- [x] T3.3 Dynamics signals: opening_move (read-first vs patch-first), true_revert (later.new==earlier.old), flip (revert + intervening user pushback), user_corrected (userModified), read_before_edit_share stat, interrupts counter. ingest now captures user text, edit old/new (normalized+capped), userModified. Deferred: n-gram action-loop repetition (lower value)
- [x] T3.4 Comprehension signals: large_write_instant_accept (>=2000 chars accepted <=3s), all exact:false (heuristic). Suppressed entirely under auto-accept permissionMode. ingest computes approval_latency_s (same-day proposal->result delta, Edit/Write only) + detects auto_accept. approval_latency_active() for the suppression payload field
- [x] T3.5 Weights (defaults + Deserialize for TOML override, loaded by binaries to keep core serde-only) + transparent weighted ranking (Σ weight×magnitude×conf-factor, breakdown exposed, low-conf ×0.5) + all six payload builders with caps/truncated markers. CLI shows ranked struggles + `--json` emits session_overview. Real donor payload passes check_payloads.py (0 errors, 347/1000 tokens)
- [x] CHECKPOINT C — signals fire on real donor with evidence; weights echoed in struggle_areas; Rust payloads pass the Python contract checker

## Phase 4 — MCP server
- [x] T4.1 rmcp stdio server (rmcp 2.2.0, now post-1.0): six tools with readOnlyHint via manual ServerHandler; memoized re-parse on (mtime,size); ADR A4 fail-closed identify — explicit session_id, else tail-scan for forwarded _meta toolUseId (no window) or mcp__sumcp__ name marker (30s freshness window), 4×150ms bounded retry, 0-or-many matches → ambiguous_session with candidates; Weights TOML loader (malformed → defaults + stderr warning); registered in .mcp.json (cargo run -q). 24 new tests incl. 3 end-to-end stdio tests spawning the real binary on the real donor fixture (all caps enforced, evidence(idxs) dereferences a struggle finding). Live-session + two-concurrent-sessions manual tests deferred to CHECKPOINT D
- [x] T4.1-verify — three-persona review (20 findings) + real-data run, all dispositioned 2026-07-17. Fixed: evidence() redaction pass (new core redact.rs, A9(4)); name-marker fallback REMOVED (identification = forwarded claudecode/toolUseId scan or explicit session_id only — Claude Code verifiably sends the _meta key; content-steerable name scan reviewed out); is_within wired on both resolution paths (symlink escape test); bounded transcript read + non-regular-file rejection (A9(3)); toolu_ format validation on forwarded ids; strict session_id/idxs argument validation; evidence token cap enforced by construction (found busting 1500 on real data); XDG empty/relative guard; config read-error warning; path-leak-free errors; cargo-audit workflow added. Deferred (below): subagents_excluded surfacing, session-cache LRU cap. Rejected: replacing rfc3339_utc with epoch (mock fixtures pin RFC 3339). Process note: unit tests largely written with code, not red-first; stdio suite's canonicalization failure was the one observed red→green
- [ ] T4.2 (deferred from verify) — surface subagent exclusion honestly: count subagent spawn references during ingest, emit `subagents_excluded: N` in session_overview until the flat-merge lands; add LRU cap (~4 entries) to SessionStore for long-lived deployments
- [ ] CHECKPOINT D — live DoD: debrief on real tools, 3 files, evidence, <500 tokens; ALSO verifies live that Claude Code's _meta toolUseId self-identification picks the right session with two concurrent sessions open

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
