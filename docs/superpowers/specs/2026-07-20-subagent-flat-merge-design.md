# Design: Subagent flat-merge (SPEC decision #2)

**Status:** approved 2026-07-20. Implements the subagent ingestion that
`SPEC.md` decision #2 fully specified but that was never built. Closes the
blind spot surfaced at Checkpoint D: a debrief reported `subagents_excluded: 3`,
hiding ~104K tokens / 28 tool calls of real subagent review work from the same
session.

**Task slot:** a new Phase-5 task, ahead of T5.3 (external validation) — running
the release gate on a metric surface that omits subagent work would waste the
volunteers, who will hit the exact same blind spot.

Raphael is learning Rust: implementation must annotate new code heavily and
explain non-obvious constructs in plain language in comments.

---

## 1. What it does, and for whom

**What:** When suMCP analyzes a session that spawned subagents, it ingests the
subagent transcripts too, flat-merges their actions into the main session's
total order, and analyzes the combined whole. Today only the main transcript is
parsed; subagent spawns are merely *counted* and disclosed as
`subagents_excluded` — an honesty stopgap, not analysis.

**For whom:** the same two users as the rest of suMCP — the developer getting an
honest post-session debrief, and the agent querying its own ground truth. Both
are currently blind to everything a subagent did. A three-persona review, a
parallel research sweep, a delegated implementation — all invisible.

**Why now:** Checkpoint D proved the blind spot is real and material on this
project's own sessions. External validation (T5.3) must not run against a metric
surface known to omit a whole class of work.

---

## 2. Success criteria

1. A session with subagent transcripts on disk produces a single merged
   `Session` whose `actions` include both main-lane and subagent-lane actions,
   in one deterministic total order.
2. `session_overview` action counts reflect the merged whole; `evidence(idx)`
   dereferences indices that land in subagent lanes.
3. The `subagents_excluded` honesty counter is **removed**; a new
   `subagent_files_missing` counter honestly reports spawns whose transcript
   could not be resolved, read, or parsed to any actions.
4. No false-merges: a subagent transcript belonging to a *different* session is
   never merged into this one, under either on-disk layout.
5. Position- and line-number-based findings (`true_revert`, `flip`, failure-
   attribution proximity) compare **only within a single lane** — the merge
   never causes a subagent edit to read as a main-agent revert, nor a main
   failure to be attributed to a subagent edit. (Revised 2026-07-20 during
   planning: supersedes an earlier `order_uncertain` field, which lane-scoping
   makes redundant — see §5.)
6. Any subagent-side failure (missing dir, unreadable/oversized/corrupt file,
   too many files) degrades gracefully: the main-lane analysis still returns,
   and the failure is counted in `subagent_files_missing`.
7. The existing 107 tests stay green (with the `subagents_excluded` →
   `subagent_files_missing` assertions updated in lockstep across
   `check_payloads.py` and `docs/payload-schema.md`).

---

## 3. Non-goals

- **Recursion.** Subagents can spawn subagents. v0.1 merges exactly **one
  level** — the main session's direct spawns. A nested spawn appears in its
  parent subagent's transcript as an `Agent`/`Task` action and rolls into that
  subagent's `subagent_files_missing` contribution: disclosed, not analyzed.
  Recursion is v0.2.
- **Cross-session merge.** Only *this* session's own subagents. Never any other
  session's files.
- **New metrics.** No new signals or findings. The merged `Session` feeds the
  existing signal/ranking/payload stack unchanged.
- **New MCP surface.** The six tools are untouched.
- **Real-data end-to-end validation of the merge.** The donor's subagent child
  files are unrecoverable (T1.2 note: donor cwd was a remote `/media` mount), so
  the merged result is validated on crafted synthetic fixtures. Real merged-data
  validation is deferred until a volunteer session (T5.3) produces real subagent
  files.

---

## 4. Architecture

The change is additive. `ingest_str(raw, lane) -> Session` already parses one
file and its doc comment already anticipates the merge ("subagent files pass
their own lane so a later merge can interleave them"). The merge step is the
missing half.

| Unit | Location | Responsibility |
|---|---|---|
| Subagent discovery | `crates/sumcp-core/src/locate.rs` | Given the main transcript path + session id, return safety-checked candidate subagent file paths (per layout). Pure path/`read_dir`/canonicalize logic — no content parsing. |
| Session assembly | `crates/sumcp-core/src/locate.rs` (or a sibling `assemble.rs`) | Orchestrate: read + `ingest_str` each discovered file, validate the sessionId back-pointer where the layout provides one, and call `merge_sessions`. Shared by both binaries so the CLI and MCP paths never duplicate it. |
| Merge | `crates/sumcp-core/src/merge.rs` (new) | Pure function: main `Session` + N subagent `Session`s + a `files_missing` count → one merged `Session`. Sort, reassign `Idx`, aggregate counters, keep only main-lane `user_texts`. No I/O. |
| Signal lane-scoping | `crates/sumcp-core/src/signals/dynamics.rs`, `crates/sumcp-core/src/signals/failures.rs` | Add same-lane guards so `true_revert`/`flip` and attribution proximity compare only within one lane (see §5a). |
| Orchestration call site | `crates/sumcp-mcp/src/store.rs`, `crates/sumcp-cli/src/main.rs` | Replace the bare `ingest_str(&raw, Lane::Main)` with the shared assembly entry point. |

**Boundary note (correction to an earlier assumption):** `sumcp-core` is *not*
strictly filesystem-free — `locate.rs` already calls `canonicalize` for its
`is_within` check and is core's designated fs-boundary module. Discovery and
assembly therefore belong in `locate.rs` (fs-aware) while `merge.rs` stays pure
and fs-free, matching the existing split rather than inventing a new one.

Downstream is *almost* unchanged: `score.rs`, the six payload builders in
`payloads.rs`, and `report.rs` consume a generic `Session` / `Vec<Action>` and
need no edits. The **exception** is three signals in `signals/` that compare
actions by position or line number and were written single-lane; the merge
requires them to become lane-aware (§5a). `Lane` / `Idx` already exist for
exactly this merge.

---

## 5. Data model

### `Session` (in `model.rs`)

- **Remove** `subagent_spawns: u64`. The `subagents_excluded` payload field it
  fed was a stopgap for the exclusion gap; once subagents are actually analyzed,
  keeping it would be dishonest.
- **Add** `subagent_files_missing: u64`. Spawns whose transcript could not be
  turned into analyzed actions — see §6 for the exact definition.

### `Action` (in `model.rs`)

No field changes. (An earlier draft added `order_uncertain: bool`; planning
found it redundant once findings are lane-scoped — §5a — since determinism
already comes from the total sort key and no strict-order finding ever compares
cross-lane pairs. Dropped.)

### `merge_sessions(main: Session, subs: Vec<Session>, files_missing: u64) -> Session`

The signature takes `main` **separately** from `subs` (not one `Vec<Session>`)
so the merge can tell which input is the primary session without a positional
convention. This matters because main is privileged: only its `user_texts`
survive and only its permission mode drives latency suppression (below). A
`Session` carries lanes on its *actions*, not one lane for the whole struct, so
without an explicit `main` parameter the merge could not identify it.

- **Concatenate** all `actions` (main's + every sub's), then **sort** by the
  existing total-order key
  `(effective_ts, lane, line_no)`. `Lane`'s derived `Ord` already puts
  `Main` before every `Sub(id)`, then orders `Sub`s lexicographically — exactly
  the contract's tie-break.
- **Reassign `Idx`** sequentially post-sort so the invariant
  `actions[i].idx == Idx(i)` holds across the merged whole. This is the critical
  guarantee: every payload and `evidence()` dereferences `Idx` against the
  merged model, so indices are only meaningful after the merge.
- **`user_texts`:** keep **only** `main.user_texts`; drop every subagent's. A
  subagent transcript's "user" turn is the orchestrator's prompt, not the
  human's — feeding it to pushback/flip detection would misread an agent's own
  instruction as human pushback.
- **`auto_accept`:** take **`main.auto_accept`** only — do **not** OR in the
  subagents. (Correction to the Section-2 sketch, caught on self-review: OR is
  semantically wrong here.) `auto_accept` suppresses the comprehension-debt
  signals (#15 approval latency, #16 large-write-instant-accept), which measure
  whether *the human* reviewed agent output. Subagent edits are reviewed by no
  human, so a subagent running in auto-accept mode must not suppress latency
  signals on the main agent's human-approved edits. Corollary for the plan: the
  comprehension signals should only ever consider **main-lane** actions —
  subagent-lane latency is meaningless (no human in that loop).
- **Other counters:** `tokens`, `type_counts`, `parse_errors`,
  `untimestamped_lines`, `interrupts` are summed across `main` + all `subs`.
  `subagent_files_missing` is the passed-in `files_missing` (computed during
  assembly, which knows what did and didn't resolve — keeps `merge_sessions`
  fs-free).

### 5a. Signal lane-scoping (required by the merge)

Three findings were written when only one lane existed and compare actions by
position or by line number. Interleaving subagent actions into the flat list
breaks their assumptions, so each gains a same-lane guard. Without this the
merge would manufacture false findings — this is not optional polish.

- **`true_revert` / `flip` (`dynamics.rs`):** a revert matches an `earlier` and
  `later` edit on the same `file_path` with `later.edit_new == earlier.edit_old`.
  Add `earlier.lane == later.lane` to the match: a revert is one actor undoing
  its *own* earlier change, not a cross-process coincidence on a shared path.
  This also fixes a latent line-number bug — `flip` compares an edit's `line_no`
  against main-lane `user_texts` line numbers, but a subagent edit's `line_no`
  indexes the *subagent's* file, so a cross-lane `flip` would compare
  incomparable line spaces. Same-lane scoping means `flip` only ever considers
  main-lane edits against main-lane pushback (subagent lanes have no human
  pushback anyway, per the `user_texts` rule).
- **Failure attribution proximity (`failures.rs`):** Step 3 walks
  `s.actions[start..pos]` (up to `PROXIMITY_WINDOW` positions back) for the most
  recent Edit/Write. Filter that slice to `p.lane == a.lane` so a failure is
  only ever attributed to a prior edit *in the same lane*. Trade-off (documented
  in code): under heavy interleaving the effective same-lane window narrows
  slightly; correctness (no cross-lane misattribution) wins over window width.

Steps 1–2 of attribution (path-in-command / path-in-error) are already correct
across lanes — a literal path match is a real link regardless of who made the
edit — so they are left unchanged.

---

## 6. Data flow & discovery

**Orchestration sequence** (the shared assembly entry point):

1. Read + `ingest_str` the **main** transcript → main-lane `Session`. Yields the
   deduped spawn list and, for the legacy layout, the `agentId`s.
2. **Discover** subagent files (below) → safety-checked paths + a
   `files_missing` count for spawns that resolved to nothing usable.
3. Read + `ingest_str` each subagent file with `Lane::Sub(agent_id)`.
4. `merge_sessions(main, subs, files_missing)` → one merged `Session`.
5. Hand off to the unchanged signal/payload stack.

**Layout selection.** A session is a single Claude Code version, so it uses one
layout. Prefer 2.1.x: if the session's `subagents/` directory exists, use the
directory-driven path; otherwise fall back to legacy spawn-driven lookup.

### Layout 2.1.x — `<session-uuid>/subagents/agent-*.jsonl`

- The `subagents/` directory is namespaced under the session UUID, so listing it
  cannot reach another session's files. `read_dir`, filter to `agent-*.jsonl`.
- **Validate each** by confirming its `sessionId` back-pointer equals the
  current session id before accepting it. Belt-and-suspenders: the directory
  already guarantees ownership, but the file asserts it, so we check.
- `is_within` canonicalization check on each path (ADR A9 symlink-escape guard,
  same as the main transcript gets).
- **`files_missing` here** (as implemented) =
  `max(main.spawns.len(), candidates.len()).saturating_sub(merged_ok)`, where
  `candidates` is the **full discovered file list before ownership/parse
  filtering** and `merged_ok` is the number of child files that read, validated,
  AND parsed to ≥1 action. **Why the `max`, not a spawns-only count:** in the
  2.1.x directory layout the namespaced `subagents/` dir can hold *more* files
  than the main transcript recorded spawns (e.g. a foreign or wrong-`sessionId`
  file we later reject). A spawns-only count would leave such a
  rejected/foreign file **invisible** — subtracted from a base too small to
  account for it — so a dropped file would not register as missing. Taking the
  max of "spawns recorded" and "files actually on disk" makes every file we
  looked at but could not merge count as missing. Spawn count is already deduped
  (`seen_tool_ids`), so replays don't inflate it. `saturating_sub` floors at
  zero because a version/streaming artifact could surface more merges than
  attempted; flooring keeps it an honest lower bound.

### Legacy layout — sibling `agent-<agentId>.jsonl` in the shared project dir

- **Never** list the project directory — it holds *every* session's files, so a
  scan would false-merge unrelated sessions' subagents. This was the fatal flaw
  of the rejected directory-scan approach.
- **Spawn-driven:** for each deduped `Agent`/`Task` spawn in the main
  transcript, pair it with its `tool_result`, read `toolUseResult.agentId`,
  construct exactly `agent-<agentId>.jsonl`, and resolve that one filename.
- `is_within` check, then existence check.
- **`files_missing` here** = the same unified formula
  `max(main.spawns.len(), candidates.len()).saturating_sub(merged_ok)`. In the
  legacy layout `candidates` is spawn-driven — only siblings our own spawns name
  and that exist — so `candidates.len() ≤ spawns.len()` and the `max` collapses
  to `spawns.len()`: effectively direct per-spawn accounting (deduped spawns
  whose file failed to resolve, failed `is_within`, didn't exist, errored on
  read, or parsed to zero actions). The one formula is correct for both layouts;
  only the 2.1.x directory case exercises the `candidates.len() > spawns.len()`
  branch.

### Empty/corrupt parse (both layouts)

A file that resolves and is validly linked but parses to **zero actions**
(empty, corrupt, all-unknown lines) counts toward `subagent_files_missing`. A
spawn we resolved but got nothing usable from is, honestly, a spawn we could not
analyze — that is what the counter promises. (Accepted trade-off: this conflates
"no file" with "empty file"; both are truthfully "un-analyzed spawn.")

---

## 7. Error handling & resource limits

**Never fail the whole load.** The main transcript is the product; subagents are
enrichment. Any subagent-side failure — unreadable dir, unreadable/oversized/
non-regular/corrupt file — skips that file, increments `subagent_files_missing`,
and the main-lane analysis still returns. This lifts the existing "never fail
the file on one bad line" rule up to "never fail the session on one bad
subagent."

**Per-file caps (ADR A9(3)).** Each subagent file passes the *same*
`MAX_TRANSCRIPT_BYTES` (256 MiB) ceiling and non-regular-file rejection that
`store.rs` applies to the main transcript.

**File-count cap.** `MAX_SUBAGENT_FILES = 64` (~5× the observed real max of 12 on
the donor). Spawns/files beyond the cap count as `subagent_files_missing`. This
bounds worst-case work against an adversarial session naming thousands of
spawns.

**MCP cache freshness (`store.rs`).** The store currently keys cache freshness on
the **main** path's `(mtime, size)` only. With subagents, a subagent file
appended after cache time would go stale undetected. The freshness key must
therefore also stat each discovered subagent file's `(mtime, size)`; any change
in any of them triggers a re-assemble. (Implementation detail for the plan, not
a design choice — just noting the invariant so it isn't missed.)

**Idx stability caveat (documented, not a bug).** Merging changes `Idx` values
versus main-only parsing. `Idx` is a within-session handle dereferenced live by
`evidence()` against the *same* merged model, and no `Idx` is persisted across
the merge boundary, so this is internally consistent within any one server view.

---

## 8. Open items to pin during planning (not design blockers)

1. **Exact on-disk path resolution for the 2.1.x layout.** SPEC decision #2
   documents `<session-uuid>/subagents/agent-*.jsonl`, but the precise relation
   between the main transcript path (`<project-dir>/<session-uuid>.jsonl`) and
   the subagents directory must be confirmed against a real 2.1.x session during
   implementation (the donor's are unrecoverable). The design does not depend on
   which relation is correct — only that discovery reads from the session's own
   namespaced directory.
2. **CLI `--file` discovery.** The CLI takes an arbitrary `--file` path (often a
   flat fixture outside `~/.claude`). Discovery from such a path must derive the
   subagents location relative to the given file; when no subagents directory or
   sibling exists, the CLI simply analyzes main-only (zero subagents,
   `subagent_files_missing: 0`) — the common case for single-file fixtures.

---

## 9. Testing

Constraint (stated up front): no real multi-file donor exists (T1.2). The merge
is validated on **crafted synthetic fixtures** for both layouts, plus the
existing real main-transcript fixture for the spawn-counting side. Real
merged-data validation is deferred to T5.3.

**TDD discipline (superpowers register):** merge logic and discovery get
red-first tests — write the failing test, watch it fail, then implement. (Prior
tasks T4.2/T4.3 were flagged for code-first tests; this one goes test-first
where the logic is non-trivial.)

**Unit (`sumcp-core`):**
- `merge_sessions`: main + sub `Session` → merged `Idx` contiguous `0..n`; order
  follows `(effective_ts, lane, line_no)`; main `user_texts` kept, sub dropped;
  additive counters summed; `auto_accept` follows `main` only (a sub in
  auto-accept does **not** flip the merged flag).
- Empty subagent `Session` merged → zero actions contributed, no panic.
- Determinism: shuffle input `subs` order → byte-identical merged output.

**Lane-scoping (`signals/`):**
- `true_revert`: a main edit and a subagent edit on the same `file_path` with
  matching revert content → **no** finding (different lanes); the same pair
  within one lane → finding fires. (Red-first: the guard is the fix.)
- Attribution proximity: a main-lane failing Bash with the nearest prior
  Edit/Write in a **subagent** lane → not attributed to it (falls through to a
  same-lane prior edit or unattributed); a same-lane prior edit → attributed.

**Fixture (new synthetic fixtures under `fixtures/`):**
- **2.1.x:** a mini `<uuid>/` tree with a main transcript + two
  `subagents/agent-*.jsonl`, one with a matching `sessionId` back-pointer, one
  with a wrong one → only the matching file merges.
- **Legacy:** main transcript with two `Agent` spawns; a sibling
  `agent-<id>.jsonl` for one, none for the other → present one merges,
  `subagent_files_missing == 1`. Add a decoy `agent-<other>.jsonl` for a
  *different* session in the same dir → it is **not** merged (false-merge guard).

**Discovery/safety (`locate.rs`):**
- `is_within` symlink escape on a subagent path → rejected.
- Oversized subagent file → skipped, counted missing, main analysis survives.
- `MAX_SUBAGENT_FILES` overflow → capped, overflow counted missing.

**Integration (`sumcp-mcp` stdio):** extend the real-binary stdio suite with a
session that has subagent files on disk → `session_overview` action counts
reflect the merge and expose `subagent_files_missing`; `evidence(idx)`
dereferences an index landing in a **sub** lane.

**Regression:** existing 107 tests green; `subagents_excluded` →
`subagent_files_missing` updated in lockstep across `check_payloads.py` and
`docs/payload-schema.md`.

---

## 10. Downstream doc updates (part of the task)

- `SPEC.md` decision #2: mark implemented; note `subagent_files_missing` replaces
  the `subagents_excluded` stopgap; record the one-level-deep and
  `MAX_SUBAGENT_FILES` constraints.
- `docs/payload-schema.md`: the `session_overview.flags` row changes from
  `subagents_excluded` to `subagent_files_missing` with the new definition.
- `scripts/check_payloads.py`: assert the new field.
- `tasks/todo.md` / `tasks/plan.md`: add the task; note it precedes T5.3.
