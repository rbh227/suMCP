# Design: retire the human-facing layer, prove nothing unused remains

Date: 2026-08-11
Status: approved in discussion, pending plan
Depends on: the review-context branch (`feat/review-context`, spec
`2026-08-10-review-context-design.md`), which must land first. This
simplification is expressed against that tree.

## Summary

suMCP's consumer is now a reviewing agent, not a human reading a report. This
design deletes the human presentation layer (HTML report, terminal prose
report, debrief skill, Stop hook), removes every script and asset that no
longer earns its place, and then runs a mechanical sweep that makes "no unused
code anywhere" a checked property rather than a hope.

One line: *a deletion of renderers and rituals, not of data. No payload, no
extraction rule, and no contract changes shape.*

## Scope decision, recorded

Three scopes were considered and the middle one was chosen by the human on
2026-08-11:

- Conservative (scripts only): rejected, leaves three products in a trench
  coat and every future change pays the maintenance tax.
- **Retire human presentation, keep the engine: CHOSEN.** The forensic
  signals and all 8 MCP tools stay, because struggle and secrets evidence is
  reviewer-relevant; it merely stops being rendered for humans.
- Review-context only (also delete signals/score/review and 6 tools):
  rejected, breaks the published study's reproducibility and discards
  evidence a reviewer can use.

## What gets deleted

| item | approx size | cascade handled |
|---|---|---|
| `crates/sumcp-core/src/html.rs` | 1,593 LOC | `--html` removed from the CLI; `redact.rs` keeps its other consumer (`payloads.rs`) |
| Human prose rendering in `report.rs` | ~300 of 560 LOC | `Overview::from_session` STAYS: `payloads.rs` builds `session_overview` totals from it. Only the terminal renderer goes |
| Debrief skill + Stop hook | install.rs chunk, `skills/debrief/` | `install` stops writing them; `uninstall` still removes them for v0.2 users because it is manifest-driven, and the upgrade path is tested (see Verification) |
| CLI `--html` flag | small | fails loudly: "removed in v0.3; the HTML report was retired with the human-facing layer". Never silently vanishes |
| Scripts: `sanitize.py`, `check_sanitizer.py`, `convergent_validity.py`, `check_debrief.py`, `render_demo_report.sh` | 5 files | one-time study tooling and debrief QA, superseded |
| Orphaned assets: `report-hero.png`, `diagram-ranking.svg`, `diagram-file-class.svg`, `diagram-tokens.svg`, `diagram-pipeline.mmd` | 5 files | verified unreferenced by the current README and every doc under `docs/` |
| Fixture: `fixtures/mock-payloads/sample-debrief.md` | 1 file | debrief artifact |
| Stale tracked scratch: `tasks/plan.md`, `tasks/todo.md` | 2 files | v0.1-era planning, superseded by `docs/superpowers/` |
| Untracked junk: `report.html`, `scripts/__pycache__/` | | delete; confirm `.gitignore` covers pycache |

## What survives, and why (two of these were originally marked for deletion)

- **`validity_sweep.py` and `validate_sweep.py` stay.** An earlier draft of
  this decision deleted all six frozen study scripts while also promising the
  published study stays reproducible, which is a contradiction: this pair IS
  the reproduction path for the 8.9x study the README links. Two frozen,
  clearly headed scripts are the price of every number in
  `docs/validation/` staying checkable.
- **The forensic engine stays whole**: `signals/`, `score.rs`, `review.rs`,
  `file_class.rs`, the `validity_dump` example, and the six v2 MCP tools.
  Chosen scope, plus `context.rs` already reuses
  `signals::failures::is_validation`.
- **CI gates stay**: `recount.py`, `check_payloads.py`, and the
  `context_recount.rs` real-data gate.
- **The install/uninstall machinery stays whole** (dry-run, backups,
  manifest, symlink defenses); it simply writes three fewer entries.
- `archive_corpus.py`, `power_estimate.py`, `pilot_blind_arm.py` stay: the
  first preserves transcripts past Claude Code's 30-day deletion, the other
  two are live artifacts of the pending precision experiment.

## CLI behavior after the cut

- Bare `sumcp` prints the `session_overview` JSON payload (what `--json`
  prints today). The consumer is an agent or a script, so JSON is the
  default, not the fallback.
- `--json` remains accepted and identical to the default, so nothing
  scripted breaks. `--html` errors with the removal message.
- `sumcp context`, `--intent`, `--range`, `--file`, `--work-unit`: unchanged.
- `install` registers the MCP server only. Windows and Unix now differ in
  exactly one documented way (file permissions); the missing-hook difference
  ceases to exist because the hook does.

## The README redesign and docs/INSTALL.md

The README is rebuilt as a landing page (shape chosen by the human on
2026-08-11: "show, then tell, then prove", ~170 lines), replacing the current
~390-line evidence-first essay. The distinctive honest voice ("what is and is
not established") is retained; the structure changes.

Order of sections:

1. Tagline and badges (unchanged tagline).
2. **What it does**, six lines plus a REAL trimmed `review_context` payload
   from an actual session (the session that built this feature: quoted
   request, a decision with `options_not_chosen`, an unfinished task, a
   claim, `totals`, `coverage`). Every field genuine, trimmed for the page.
3. **Try it**: a six-line install block (download, checksum verify,
   `install --apply`) linking to `docs/INSTALL.md` for everything else, then
   `sumcp context` usage.
4. **Why**: the 56.3% noise problem, ARCTIC, overcorrection. One short
   section, three links, no longer two screens.
5. **The eight tools**: table, unchanged content.
6. **Wiring a reviewer**, one snippet per consumer type, which documents on
   the page that the reviewer side is tool-agnostic: Codex
   (`~/.codex/config.toml`), Claude Code as the reviewer (`claude mcp add` /
   `.mcp.json`), and no-MCP (`sumcp context | jq` in a script or CI).
7. **What is and is not established**: kept, compressed to roughly 20 lines,
   two claims (forensic layer measured in both directions; review-context
   layer built and unvalidated, with the pilot's own numbers), linking to
   the validation docs.
8. **How it works**: the pipeline diagram plus the recount-gate paragraph
   with its two-tier independence phrasing.
9. Roadmap, brief: experiment redesign gated, memory layer gated, plus one
   direction bullet for writer-side adapters for other agents' session logs
   (Codex CLI already keeps JSONL sessions on disk, verified on this
   machine), framed as direction, not promise.
10. License.

`docs/INSTALL.md` is created and absorbs: the platform matrix, macOS
signing and quarantine, the GLIBC/musl choice, WSL, the Rosetta CI note,
MSRV, from-source instructions, the write contract pointer, and uninstall.
The README links to it; nothing is deleted, only relocated, except the
sections about the hook and debrief, which this spec deletes outright.

## The no-unused-code sweep (the part that makes "nothing unused" checkable)

Deleting consumers orphans helpers, and `rustc`'s `dead_code` lint cannot see
orphaned `pub` items in a library crate: a `pub fn` with zero callers warns
nowhere. So the sweep is mechanical, in this order:

1. **Visibility audit.** For every `pub` item in `sumcp-core`, find a
   consumer outside the crate (`sumcp-cli`, `sumcp-mcp`, integration tests,
   the `validity_dump` example). No consumer: demote to `pub(crate)` or
   private. After demotion, the compiler's `dead_code` lint becomes
   authoritative for the whole crate, and CI already runs clippy with
   `-D warnings`, so an orphan is a build failure from then on, not a hope.
2. **Delete every item the lint then flags**, recursively, until the build is
   warning-free. This catches the second-order orphans (a helper whose only
   caller was a helper whose only caller was `html.rs`).
3. **Dependency audit.** Every dependency in all three `Cargo.toml`s must
   have a compiled use; the list is small enough to verify by inspection
   (serde, serde_json, tempfile dev-only, and the mcp crate's rmcp/tokio).
4. **Test-helper and fixture audit.** Test-only helpers whose tests were
   deleted go with them; every file under `fixtures/` must be referenced by
   at least one test or documented in `fixtures/README.md` as a
   deliberately kept raw sample.
5. **Asset and doc-reference audit.** Every file under `docs/assets/` must be
   referenced by the README or a doc; every script under `scripts/` must be
   referenced by CI, a doc, or carry a header stating why it exists.

The sweep's end state is stated as an invariant in the plan's verification:
`cargo clippy --workspace --all-targets -- -D warnings` clean AFTER the
visibility demotions, which is a strictly stronger guarantee than today's.

## Verification

1. **Contract tests are the spine.** All stdio tests, `check_payloads.py`,
   and both recount gates pass byte-identically, because no payload changes.
   Workspace suite lands around 400-410 (HTML and debrief-install tests
   deleted, small flag-removal guards added).
2. **Install round-trip on a throwaway HOME**: fresh install writes no skill
   and no hook and registers the server; a simulated v0.2 manifest then
   uninstalls cleanly, proving the upgrade path for existing users.
3. **Live end-to-end**: release binaries against a real session, MCP and CLI,
   payload keys identical to pre-cut output.
4. **Docs sweep**: the README is rebuilt per its own section above (which
   also removes the hero, debrief, hook, and `--html` references and shrinks
   the Windows section to one difference); `docs/INSTALL.md` created;
   CHANGELOG entry; server instruction strings; `fixtures/README.md`. The
   payload snippet in the README must be verified against a real invocation
   of the built binary, not hand-typed. Checked during self-review:
   `docs/metrics.md` and `docs/metrics-spec.md` carry no debrief references,
   and the deleted scripts are referenced only by dated plans and specs
   under `docs/superpowers/`, which are historical records and are
   deliberately NOT edited.
5. **CI green on all three platforms**, with the Windows job's hook-related
   assertions deleted alongside the hook.

## Error handling

The only user-visible behavioral edge is `--html`, which must fail with a
clear one-line explanation rather than disappearing from the flag list
silently. Everything else is deletion of code paths that, once gone, cannot
be reached at all.

## Out of scope, explicitly

- No changes to extraction rules, payload contracts, `list_cap`,
  `CAP_REVIEW_CONTEXT`, or anything the pending precision experiment will
  measure. Simplification must not move what the experiment depends on.
- No changes to the v2/v3 payload version numbers.
- The Task 16 experiment redesign and the memory layer remain separate,
  gated work.
- Git history is not rewritten; deleted code remains recoverable from
  history, which is the reason deletion is cheap.

## Risks

- **A hidden consumer of a "human-facing" item.** Mitigated by the
  visibility audit running BEFORE deletion commits, and by the byte-identical
  contract-test spine.
- **v0.2 users' installed hooks referencing a deleted script.** The installed
  hook file lives under `~/.claude/sumcp/hooks/` and keeps working against
  the old binary until they upgrade; `uninstall` removes it via the manifest
  either way. The new `install` simply never writes it.
- **The README's published-study section loses its diagrams.** Accepted: the
  numbers and links stay; the three deleted SVGs illustrated a ranking story
  the README no longer leads with.
