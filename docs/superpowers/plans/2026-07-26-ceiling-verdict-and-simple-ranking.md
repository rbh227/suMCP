# Ceiling Verdict and Simple Ranking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Confirm that no weighting over observable features beats counting
edits, then replace the weighted score with a stated ordering rule (edited
files first, code before docs and config, then edit count, ties by path),
keeping every detector and all evidence untouched.

**Architecture:** `score::rank` stops computing a weighted sum and starts
sorting on four declared keys. A new pure `file_class` module supplies the class
key. `Weights` and its TOML config are deleted outright, because nothing except
ranking ever read them. The payload contract goes from `v: 0` to `v: 1` because
`score` and `weights` are removed from two payloads.

**Tech Stack:** Rust 2024 edition, `serde`/`serde_json` only in
`sumcp-core`; python3 stdlib for dev scripts.

**Source spec:** `docs/superpowers/specs/2026-07-26-ceiling-verdict-and-simple-ranking-design.md`

## Global Constraints

- MSRV is `1.88` (`rust-version` in `Cargo.toml`), enforced by a CI job. Do not
  use language features newer than 1.88.
- `sumcp-core` stays synchronous and pure with no dependencies beyond `serde`
  and `serde_json` (ADR A2). Add no crates.
- Dev scripts are **python3 stdlib only**, matching `sanitize.py`,
  `check_payloads.py`, and `validity_sweep.py`. Do not import numpy.
- **No em dashes** in any prose, doc, comment, or commit message. The repo was
  scrubbed of 24 of them in T5.4 and the gate is enforced by review.
- No real filesystem paths, project names, or prompt text in anything committed
  under `docs/` or `fixtures/`. Projects are anonymized `proj-01..proj-NN`.
- All output must be deterministic: sort every collection before use, seed every
  RNG. Two runs on unchanged input produce byte-identical output.
- CI runs `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `cargo test --workspace`, and `python3
  scripts/check_payloads.py` on every push. All four must pass before any commit
  is pushed.
- Public items in `sumcp-core` need rustdoc: the crate has
  `#![warn(missing_docs)]` at `crates/sumcp-core/src/lib.rs:1`.
- The corpus lives at `~/sumcp-corpus-archive/projects-2026-07-26/`. Never read
  `~/.claude/projects` for analysis; a 30-day cleanup is emptying it.

---

## File Structure

**Created:**
- `crates/sumcp-core/src/file_class.rs`: path to `FileClass` classification and
  ranking tier. Pure, no I/O, one responsibility.
- `scripts/ceiling_analysis.py`: the feasibility measurement. Imports
  `validity_sweep.py` for corpus assembly and holdout resolution rather than
  reimplementing either.
- `docs/validation/2026-07-26-ceiling-analysis.md`: the published negative
  result.

**Modified:**
- `crates/sumcp-core/examples/validity_dump.rs`: emit `score` and `breakdown`.
- `scripts/validity_sweep.py`: `CACHE_SCHEMA` 2 to 3, corpus directory override.
- `crates/sumcp-core/src/lib.rs`: register `file_class`.
- `crates/sumcp-core/src/score.rs`: new ordering, `FileScore` shape, delete
  `Weights`.
- `crates/sumcp-core/src/payloads.rs`: `v: 1`, `class`/`edits` instead of
  `score`, `ranking_rule` instead of `weights`.
- `crates/sumcp-core/src/html.rs`: render class and edits, new footnote.
- `crates/sumcp-cli/src/main.rs`: call-site updates and the terminal line.
- `crates/sumcp-mcp/src/main.rs`: delete the weights loader, warn on a stale
  config.
- `crates/sumcp-mcp/src/server.rs`: delete the `weights` field.
- `crates/sumcp-mcp/src/identify.rs`, `crates/sumcp-mcp/tests/stdio.rs`: `v`
  assertions.
- `fixtures/mock-payloads/*.json` (7 files): `v: 1`, and the two ranking mocks
  reshaped.
- `scripts/check_payloads.py`: v1 rules.
- `docs/payload-schema.md`, `docs/metrics.md`, `SPEC.md`, `README.md`,
  `tasks/todo.md`.

**Not modified, deliberately:** every file under
`crates/sumcp-core/src/signals/`, `ingest.rs`, `merge.rs`, `model.rs`,
`assemble.rs`, `locate.rs`, `redact.rs`, `report.rs`, and the installer. The
detectors and the evidence chain do not change.

---

### Task 1: Dump the missing features and stop reading the decaying corpus

**Files:**
- Modify: `crates/sumcp-core/examples/validity_dump.rs:106-114`
- Modify: `scripts/validity_sweep.py:66` (`PROJECTS_DIR`), `:84` (`CACHE_SCHEMA`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: each entry in the dump's `files` array gains `"score"` (float,
  1 decimal) and `"breakdown"` (object mapping category name to integer).
  `scripts/ceiling_analysis.py` in Task 2 reads both. `validity_sweep.py`
  exposes `PROJECTS_DIR` resolved from `SUMCP_CORPUS_DIR`.

- [ ] **Step 1: Add the two fields to the dump**

In `crates/sumcp-core/examples/validity_dump.rs`, replace the `json!` block at
lines 106-114 with:

```rust
            json!({
                "file": file,
                "edits": edits,
                "changed_lines": changed_lines.get(file).copied().unwrap_or(0),
                "kinds": kinds,
                "flagged_nr": nr_files.contains(file),
                "flagged_top3": top3_files.contains(file),
                "last_edit_verified": verified,
                // Ranking inputs, so the ceiling analysis can ask whether any
                // weighting over per-category MAGNITUDES (not just kind
                // presence) beats counting edits. Study-only: nothing in the
                // product reads these back.
                "score": ranked
                    .iter()
                    .find(|r| r.file.as_str() == *file)
                    .map(|r| (r.score * 10.0).round() / 10.0),
                "breakdown": ranked
                    .iter()
                    .find(|r| r.file.as_str() == *file)
                    .map(|r| r.breakdown.clone())
                    .unwrap_or_default(),
            })
```

`score` is `null` for a file that carries no ranking finding, which is correct:
`rank` never ranked it. `breakdown` is `{}` in that case.

- [ ] **Step 2: Build the dump and confirm both fields appear**

```bash
cargo build --release --example validity_dump -p sumcp-core
./target/release/examples/validity_dump fixtures/session-2_1_210-subagents.jsonl \
  | python3 -c "
import json,sys
d=json.load(sys.stdin)
f=d['files']
assert f, 'no files in dump'
assert all('score' in e and 'breakdown' in e for e in f), 'fields missing'
scored=[e for e in f if e['score'] is not None]
print(f'files={len(f)} with_score={len(scored)}')
print('sample:', json.dumps(scored[0], sort_keys=True)[:200] if scored else 'none scored')
"
```

Expected: a non-zero `with_score` count and a sample entry showing both
`breakdown` and `score`.

- [ ] **Step 3: Bump the cache schema**

In `scripts/validity_sweep.py`, change line 84 from `CACHE_SCHEMA = 2` to:

```python
# v3: added per-file `score` and `breakdown` (ranking magnitudes) for
# scripts/ceiling_analysis.py.
CACHE_SCHEMA = 3
```

This is mandatory, not cosmetic. The freshness check compares cache and
transcript mtimes and cannot see that the dump binary changed, so without the
bump every cached dump would keep its old shape and the reader's defaults would
render the missing fields as a plausible column of zeros.

- [ ] **Step 4: Point the corpus at the archive**

In `scripts/validity_sweep.py`, replace line 66:

```python
PROJECTS_DIR = Path.home() / ".claude" / "projects"
```

with:

```python
# The corpus is the ARCHIVE, not the live directory. Claude Code's
# `cleanupPeriodDays` defaults to 30, so `~/.claude/projects` deletes sessions
# out from under a longitudinal study: on 2026-07-26 the oldest surviving
# transcript was 31 days old and one frozen holdout project had already
# vanished. Refreshing the archive is a deliberate manual step so a run can
# never quietly pick up sessions that arrived mid-analysis.
DEFAULT_CORPUS_DIR = Path.home() / "sumcp-corpus-archive" / "projects-2026-07-26"
PROJECTS_DIR = Path(os.environ.get("SUMCP_CORPUS_DIR") or DEFAULT_CORPUS_DIR)
```

Add `import os` to the import block at lines 57-64, keeping alphabetical order
(before `import subprocess`).

- [ ] **Step 5: Print the resolved corpus so no number is ever unattributed**

In `scripts/validity_sweep.py`, find the `main()` print that reports the corpus
(the line printing `sessions` and `projects` counts, near line 1330) and add
immediately before it:

```python
    print(f"corpus: {PROJECTS_DIR}")
```

- [ ] **Step 6: Verify the sweep still runs on the archive**

```bash
python3 scripts/validity_sweep.py 2>&1 | head -20
```

Expected: a `corpus:` line naming the archive path, then the usual session and
project counts, then the comparison table. The holdout line must still resolve
`proj-04` and must not fail closed. If it prints `fail closed: NO frozen
held-out project is present`, the archive is wrong; stop and fix the path before
continuing.

Note the run regenerates the dump cache from scratch because of the schema
bump, so it takes longer than usual.

- [ ] **Step 7: Confirm the report did not change**

```bash
git diff --stat docs/validation/2026-07-22-predictive-validity.md
```

Expected: no output. The sweep rewrites that draft, and adding dump fields must
not move any published number. If it does, something about the corpus changed
and that must be understood before proceeding.

- [ ] **Step 8: Commit**

```bash
git add crates/sumcp-core/examples/validity_dump.rs scripts/validity_sweep.py
git commit -m "validity: dump ranking magnitudes, read the archived corpus

The study corpus was being deleted underneath the analysis: cleanupPeriodDays
is unset, so the 30-day default had already removed one frozen holdout project
and the oldest surviving transcript was 31 days old. The sweep now reads
~/sumcp-corpus-archive/projects-2026-07-26 (SUMCP_CORPUS_DIR overrides) and
prints which corpus produced its numbers.

The dump also emits per-file score and breakdown, so the ceiling analysis can
test weightings over per-category magnitudes rather than kind presence alone.
CACHE_SCHEMA 2 to 3 so every cached dump regenerates."
```

---

### Task 2: Measure the ceiling and run the gate

**Files:**
- Create: `scripts/ceiling_analysis.py`

**Interfaces:**
- Consumes: the Task 1 dump fields; `validity_sweep.py`'s `build_corpus`,
  `group_by_project`, `anonymize_projects`, `held_out_project_ids`,
  `window_sessions`, `outcome_in_window`.
- Produces: a printed report. No repo file is written by the script; its numbers
  are transcribed into `docs/validation/2026-07-26-ceiling-analysis.md` in
  Task 7.

- [ ] **Step 1: Write the script**

Create `scripts/ceiling_analysis.py`:

```python
#!/usr/bin/env python3
"""How good could ANY weighting over observable features be, at a fixed flag
budget? (spec 2026-07-26, Part 1b.)

The product ranks files by a weighted sum of finding magnitudes. The
2026-07-22 study showed that ranking does not beat sorting files by edit
count. This script asks the stronger question: could ANY weighting?

Method. Fit weights to maximize hits at a fixed flag budget ON THE VERY PAIRS
BEING SCORED. That is maximally overfit on purpose, so the result upper-bounds
what any honest rule could achieve. Two independent fitters, because a single
search could report a low number simply by failing to search hard enough,
which would bias the verdict toward "no headroom":

  1. Coordinate ascent on hits@budget, with fixed-seed restarts. Directly
     optimizes the target metric; non-convex, so it finds a local maximum and
     therefore a LOWER bound on the in-sample maximum.
  2. In-sample logistic regression, ranked by predicted probability. Convex,
     so no local-optimum risk, but it optimizes likelihood rather than
     hits@budget.

The verdict uses the better of the two. Agreement between them is the evidence
that the search was adequate.

Also reports leave-one-project-out, which estimates what would generalize.
With only three tune projects those folds are coarse; read it as a direction,
not a measurement.

Read-only. Tune split only. Primary outcome (strong recurrence, next 3
sessions) throughout, matching the preregistered outcome in the 2026-07-22
study. python3 stdlib only.

Runtime is a couple of minutes: the fitters are pure Python by design, to keep
this repo's dev scripts dependency-free.
"""

from __future__ import annotations

import importlib.util
import math
import random
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

_spec = importlib.util.spec_from_file_location("vs", REPO / "scripts" / "validity_sweep.py")
vs = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(vs)

# Flag budgets to report. 65 is what `top-3 by edit count` fires on over the
# tune split, 52 is what `top-3 by edit count, code files only` fires on, and
# 40 is roughly what the product's own `needs_review` fires on among code
# files. Fixed here, above any result.
BUDGETS = (40, 52, 65)

# Coordinate-ascent search budget. Restart seeds are a fixed list so two runs
# are byte-identical.
RESTART_SEEDS = tuple(range(12))
SWEEPS = 6
WEIGHT_GRID = (-4.0, -2.0, -1.0, -0.5, -0.25, 0.0, 0.25, 0.5, 1.0, 2.0, 4.0)

# Logistic-regression fit.
LOGIT_STEPS = 3000
LOGIT_LR = 0.5

CODE_EXT = {
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "h", "cc", "cpp",
    "hpp", "rb", "swift", "kt", "sh", "bash", "zsh", "sql", "vue", "svelte",
    "cs", "php", "lua", "scala", "dart", "ex", "exs", "clj", "hs", "ml", "m",
    "mm", "r", "pl",
}
WEB_EXT = {"html", "css", "scss", "sass", "less"}
DOCS_EXT = {"md", "mdx", "txt", "rst", "adoc", "tex"}
CONFIG_EXT = {
    "json", "toml", "yaml", "yml", "ini", "cfg", "conf", "env", "lock",
    "properties", "gradle", "xml", "plist",
}


def file_class(path: str) -> str:
    """Mirror of `sumcp_core::file_class::classify`. Kept in sync by the
    shared table above; if the Rust table changes, change this one."""
    lower = path.lower()
    name = lower.rsplit("/", 1)[-1]
    if name.startswith(".env"):
        return "config"
    if "/memory/" in lower or name.startswith("memory.") or "/notes" in lower:
        return "notes"
    if "." not in name:
        return "other"
    ext = name.rsplit(".", 1)[-1]
    if ext in CODE_EXT:
        return "code"
    if ext in DOCS_EXT:
        return "docs"
    if ext in CONFIG_EXT:
        return "config"
    if ext in WEB_EXT:
        return "web"
    return "other"


FEATURES = (
    "edits", "log_edits", "changed_lines", "log_lines",
    "churn_mag", "rework_mag", "re_read_mag", "fumble_mag", "failure_mag",
    "n_kinds", "is_code", "is_docs", "is_config", "is_notes", "verified",
    "product_score",
)


def featurize(p: dict) -> list[float]:
    b = p["breakdown"]
    c = p["class"]
    return [
        float(p["edits"]),
        math.log1p(p["edits"]),
        float(p["changed_lines"]),
        math.log1p(p["changed_lines"]),
        float(b.get("churn", 0)),
        float(b.get("rework", 0)),
        float(b.get("re_read", 0)),
        float(b.get("fumbles", 0)),
        float(b.get("failure_loops", 0)),
        float(len(p["kinds"])),
        1.0 if c == "code" else 0.0,
        1.0 if c == "docs" else 0.0,
        1.0 if c == "config" else 0.0,
        1.0 if c == "notes" else 0.0,
        1.0 if p["verified"] else 0.0,
        float(p["score"] or 0.0),
    ]


def build_pairs(groups, anon) -> list[dict]:
    """Every (session N, edited file) pair, retaining the fields
    `validity_sweep.build_pairs` drops (`file`, `kinds`, `breakdown`,
    `score`). Same last-session exclusion and same window logic, taken from
    the imported module so the two cannot drift."""
    pairs = []
    for real_proj in sorted(groups):
        ordered = groups[real_proj]
        for i, s in enumerate(ordered):
            if i == len(ordered) - 1:
                continue  # no window successor exists
            next3, _ = vs.window_sessions(ordered, i)
            for entry in sorted(s["dump"]["files"], key=lambda e: e["file"]):
                file = entry["file"]
                pairs.append({
                    "project": anon[real_proj],
                    "session_index": i,
                    "file": file,
                    "class": file_class(file),
                    "edits": entry["edits"],
                    "changed_lines": entry.get("changed_lines", 0),
                    "kinds": sorted(entry.get("kinds", [])),
                    "breakdown": entry.get("breakdown", {}),
                    "score": entry.get("score"),
                    "verified": entry.get("last_edit_verified", False),
                    "flagged_nr": entry["flagged_nr"],
                    "y": 1 if vs.outcome_in_window(next3, file, strong=True) else 0,
                })
    return pairs


def hits_at(scores: list[float], y: list[int], budget: int) -> int:
    """Hits among the top `budget` by score. Index breaks ties, so ordering
    luck cannot leak in."""
    order = sorted(range(len(scores)), key=lambda i: (-scores[i], i))
    return sum(y[i] for i in order[:budget])


def dot_columns(cols: list[list[float]], w: list[float]) -> list[float]:
    n = len(cols[0])
    out = [0.0] * n
    for j, wj in enumerate(w):
        if wj == 0.0:
            continue
        col = cols[j]
        for i in range(n):
            out[i] += wj * col[i]
    return out


def coordinate_ascent(cols, y, budget):
    """Maximize hits@budget over linear weights. Returns (best_hits, best_w).
    Scores are updated incrementally when one coordinate moves, which is what
    makes a pure-Python search affordable."""
    d = len(cols)
    best_h, best_w = -1, [0.0] * d
    for seed in RESTART_SEEDS:
        rng = random.Random(seed)
        w = [0.0] * d if seed == 0 else [rng.choice(WEIGHT_GRID) for _ in range(d)]
        scores = dot_columns(cols, w)
        h = hits_at(scores, y, budget)
        for _ in range(SWEEPS):
            improved = False
            for j in range(d):
                cur, best_g, best_gh = w[j], w[j], h
                for g in WEIGHT_GRID:
                    if g == cur:
                        continue
                    delta = g - cur
                    trial = [scores[i] + delta * cols[j][i] for i in range(len(y))]
                    gh = hits_at(trial, y, budget)
                    if gh > best_gh:
                        best_g, best_gh = g, gh
                if best_g != cur:
                    delta = best_g - cur
                    scores = [scores[i] + delta * cols[j][i] for i in range(len(y))]
                    w[j], h, improved = best_g, best_gh, True
            if not improved:
                break
        if h > best_h:
            best_h, best_w = h, list(w)
    return best_h, best_w


def logistic_fit(cols, y):
    """Plain gradient ascent on the log-likelihood. Convex, so the fixed
    starting point at zero is sufficient and no restarts are needed."""
    d, n = len(cols), len(y)
    w, b = [0.0] * d, 0.0
    for _ in range(LOGIT_STEPS):
        z = dot_columns(cols, w)
        gb, gw = 0.0, [0.0] * d
        for i in range(n):
            zi = z[i] + b
            zi = 30.0 if zi > 30.0 else (-30.0 if zi < -30.0 else zi)
            err = y[i] - 1.0 / (1.0 + math.exp(-zi))
            gb += err
            for j in range(d):
                gw[j] += err * cols[j][i]
        b += LOGIT_LR * gb / n
        for j in range(d):
            w[j] += LOGIT_LR * gw[j] / n
    return w


def standardize(rows):
    d = len(rows[0])
    cols = [[r[j] for r in rows] for j in range(d)]
    for j in range(d):
        col = cols[j]
        mu = sum(col) / len(col)
        var = sum((v - mu) ** 2 for v in col) / len(col)
        sd = math.sqrt(var) or 1.0
        cols[j] = [(v - mu) / sd for v in col]
    return cols


def per_session_top3(pairs, eligible=None):
    by_session = defaultdict(list)
    for p in pairs:
        by_session[(p["project"], p["session_index"])].append(p)
    picked = []
    for _k, fs in sorted(by_session.items()):
        cand = [f for f in fs if eligible is None or eligible(f)]
        cand.sort(key=lambda e: (-e["edits"], e["file"]))
        picked.extend(cand[:3])
    return picked


def main() -> int:
    sessions, counters = vs.build_corpus()
    groups = vs.group_by_project(sessions)
    anon = vs.anonymize_projects(groups)
    held_out, absent = vs.held_out_project_ids(groups, anon)

    every = build_pairs(groups, anon)
    # THE SPLIT, before any metric. Matches docs/validation/holdout.md.
    tune = [p for p in every if p["project"] not in held_out]
    withheld = len(every) - len(tune)

    print("=" * 78)
    print("CEILING ANALYSIS: can ANY weighting beat counting edits?")
    print("=" * 78)
    print(f"corpus:    {vs.PROJECTS_DIR}")
    print(f"sessions:  {counters['sessions']}   projects: {len(groups)}")
    print(f"held out:  {sorted(held_out)}  ({withheld} pairs withheld)")
    if absent:
        print(f"           absent frozen fingerprints: {absent}")
    y = [p["y"] for p in tune]
    print(f"tune:      {len(tune)} pairs, {sum(y)} positives, "
          f"base rate {sum(y) / len(tune):.3f}")
    print(f"outcome:   strong recurrence, next 3 sessions (preregistered)")
    print(f"features:  {len(FEATURES)} -> {', '.join(FEATURES)}")
    print()

    base = per_session_top3(tune)
    code = per_session_top3(tune, eligible=lambda p: p["class"] == "code")
    print("REFERENCE RULES")
    for label, flags in (("top-3 by edits", base),
                         ("top-3 by edits, code only", code),
                         ("PRODUCT needs_review", [p for p in tune if p["flagged_nr"]])):
        h = sum(p["y"] for p in flags)
        print(f"  {label:<32} flags={len(flags):>3}  hits={h:>3}  "
              f"precision={h / len(flags) if flags else 0:.3f}")
    print()

    cols = standardize([featurize(p) for p in tune])
    edits_col = [float(p["edits"]) for p in tune]
    projects = [p["project"] for p in tune]

    verdict_ok = True
    for budget in BUDGETS:
        eb = hits_at(edits_col, y, budget)
        ca_h, ca_w = coordinate_ascent(cols, y, budget)
        lg_w = logistic_fit(cols, y)
        lg_h = hits_at(dot_columns(cols, lg_w), y, budget)
        best = max(ca_h, lg_h)
        margin = best - eb

        pooled_h = pooled_n = 0
        for proj in sorted(set(projects)):
            tr = [i for i, p in enumerate(projects) if p != proj]
            te = [i for i, p in enumerate(projects) if p == proj]
            fold_budget = max(1, round(budget * len(te) / len(tune)))
            tr_cols = [[cols[j][i] for i in tr] for j in range(len(cols))]
            tr_y = [y[i] for i in tr]
            _h, w_tr = coordinate_ascent(tr_cols, tr_y,
                                         max(1, budget - fold_budget))
            te_cols = [[cols[j][i] for i in te] for j in range(len(cols))]
            te_y = [y[i] for i in te]
            pooled_h += hits_at(dot_columns(te_cols, w_tr), te_y, fold_budget)
            pooled_n += fold_budget

        print(f"BUDGET {budget} FLAGS")
        print(f"  edit count alone            hits {eb:>3}  precision {eb / budget:.3f}")
        print(f"  in-sample, coord ascent     hits {ca_h:>3}  precision {ca_h / budget:.3f}")
        print(f"  in-sample, logistic         hits {lg_h:>3}  precision {lg_h / budget:.3f}")
        print(f"  IN-SAMPLE BEST (bound)      hits {best:>3}  precision {best / budget:.3f}"
              f"   margin over edits: {margin:+d}")
        print(f"  leave-one-project-out       hits {pooled_h:>3}  "
              f"precision {pooled_h / pooled_n:.3f}  (n={pooled_n})")
        ranked_feats = sorted(range(len(FEATURES)), key=lambda j: -abs(ca_w[j]))[:5]
        print("  coord-ascent winner leaned on: "
              + ", ".join(f"{FEATURES[j]}={ca_w[j]:+.2f}" for j in ranked_feats))
        if margin > 4:
            verdict_ok = False
        print()

    print("=" * 78)
    print("GATE (spec 2026-07-26 Part 1d, fixed before this ran):")
    print("  CONFIRMED if the in-sample bound exceeds edit count by at most")
    print("  +4 hits at EVERY budget. An in-sample maximum is optimistic, so a")
    print("  rule that cannot reach +5 while fitting to the answers cannot")
    print("  generalize a win.")
    print()
    print("  RESULT: " + ("CONFIRMED, no headroom. Proceed to Part 2."
                          if verdict_ok else
                          "OVERTURNED. Do NOT proceed to Part 2; run the full "
                          "preregistered leave-one-project-out pass instead."))
    print()
    print("CAVEAT to carry into the report: leave-one-project-out has only")
    print("three coarse folds here, and its numbers are non-monotonic across")
    print("budgets. Read it as a direction, not a measurement. The verdict")
    print("rests on the in-sample bound.")
    print("=" * 78)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 2: Run it and capture the output**

```bash
python3 scripts/ceiling_analysis.py 2>&1 | tee /tmp/ceiling-run-1.txt
```

Expected: a `RESULT:` line. The brainstorm's preliminary run, on kind presence
without magnitudes, produced margins of +3, +2, +4 at budgets 40, 52, 65, so
`CONFIRMED` is the expected outcome. Runtime is a couple of minutes.

- [ ] **Step 3: Verify determinism**

```bash
python3 scripts/ceiling_analysis.py > /tmp/ceiling-run-2.txt 2>&1
diff /tmp/ceiling-run-1.txt /tmp/ceiling-run-2.txt && echo "DETERMINISTIC"
```

Expected: `DETERMINISTIC` with no diff output. If the two runs differ, an RNG
or an unsorted collection escaped; fix it before continuing, because a
non-reproducible verdict is not a verdict.

- [ ] **Step 4: Obey the gate**

If `RESULT` says `OVERTURNED`, **stop this plan here**. Report the margins to
the user. Part 2 is predicated on the verdict, and the spec's branch is to run
the full preregistered pass instead. Do not proceed to Task 3.

If `RESULT` says `CONFIRMED`, continue.

- [ ] **Step 5: Commit**

```bash
git add scripts/ceiling_analysis.py
git commit -m "validity: ceiling analysis, can any weighting beat edit count

Fits weights to maximize hits at a fixed flag budget on the same pairs it
scores. Maximally overfit on purpose, so the result upper-bounds any honest
rule. Two independent fitters (coordinate ascent on the target metric,
logistic regression for convexity) so a low number cannot be an artifact of
under-searching, which would bias the verdict toward finding no headroom.

Reuses validity_sweep's corpus assembly and holdout resolution by import, so
the split and the outcome logic cannot drift from the published study. Tune
split only, split before any metric, preregistered outcome throughout."
```

---

### Task 3: `file_class`

**Files:**
- Create: `crates/sumcp-core/src/file_class.rs`
- Modify: `crates/sumcp-core/src/lib.rs:9-20`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub enum FileClass { Code, Web, Notes, Docs, Config, Other }`
  deriving `Debug, Clone, Copy, PartialEq, Eq, Serialize` with
  `#[serde(rename_all = "snake_case")]`; `pub fn classify(path: &str) ->
  FileClass`; `pub fn tier(self) -> u8` on `FileClass`. Task 4 calls
  `classify` and `tier`.

- [ ] **Step 1: Write the failing tests**

Create `crates/sumcp-core/src/file_class.rs` containing only the test module
for now, so the first run fails to compile for the right reason:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_their_class() {
        assert_eq!(classify("/repo/src/main.rs"), FileClass::Code);
        assert_eq!(classify("/repo/app/page.tsx"), FileClass::Code);
        assert_eq!(classify("/repo/docs/guide.md"), FileClass::Docs);
        assert_eq!(classify("/repo/Cargo.toml"), FileClass::Config);
        assert_eq!(classify("/repo/site/styles.css"), FileClass::Web);
    }

    #[test]
    fn classification_is_case_insensitive() {
        // A transcript reports whatever the user typed, and READMEs are
        // routinely uppercase.
        assert_eq!(classify("/repo/README.MD"), FileClass::Docs);
        assert_eq!(classify("/repo/src/Main.RS"), FileClass::Code);
    }

    #[test]
    fn notes_beats_extension() {
        // A markdown file under a memory directory is a notes file, not
        // documentation: the two behave differently and the path says which.
        assert_eq!(classify("/home/u/.claude/memory/plan.md"), FileClass::Notes);
        assert_eq!(classify("/repo/notes/scratch.md"), FileClass::Notes);
        assert_eq!(classify("/repo/memory.md"), FileClass::Notes);
    }

    #[test]
    fn dotenv_is_config_including_suffixed_variants() {
        assert_eq!(classify("/repo/.env"), FileClass::Config);
        assert_eq!(classify("/repo/.env.local"), FileClass::Config);
        assert_eq!(classify("/repo/.env.production"), FileClass::Config);
    }

    #[test]
    fn extensionless_and_unknown_are_other() {
        assert_eq!(classify("/repo/Makefile"), FileClass::Other);
        assert_eq!(classify("/repo/LICENSE"), FileClass::Other);
        assert_eq!(classify("/repo/assets/hero.jpg"), FileClass::Other);
        assert_eq!(classify("/repo/.gitignore"), FileClass::Other);
    }

    #[test]
    fn bare_filename_without_a_directory_still_classifies() {
        assert_eq!(classify("main.rs"), FileClass::Code);
        assert_eq!(classify("notes.md"), FileClass::Docs);
    }

    #[test]
    fn tiers_order_code_above_notes_above_docs_above_config() {
        assert!(FileClass::Code.tier() < FileClass::Notes.tier());
        assert!(FileClass::Notes.tier() < FileClass::Docs.tier());
        assert!(FileClass::Docs.tier() < FileClass::Config.tier());
        // Web ranks with code; Other ranks with config.
        assert_eq!(FileClass::Web.tier(), FileClass::Code.tier());
        assert_eq!(FileClass::Other.tier(), FileClass::Config.tier());
    }

    #[test]
    fn serializes_as_snake_case() {
        let j = serde_json::to_value(FileClass::Code).unwrap();
        assert_eq!(j, serde_json::json!("code"));
        let j = serde_json::to_value(FileClass::Other).unwrap();
        assert_eq!(j, serde_json::json!("other"));
    }
}
```

Register the module by adding `pub mod file_class;` to
`crates/sumcp-core/src/lib.rs`, in alphabetical position between
`pub mod assemble;` and `pub mod html;`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p sumcp-core file_class`
Expected: FAIL to compile, with errors like
`cannot find type FileClass in this scope` and
`cannot find function classify in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `crates/sumcp-core/src/file_class.rs`:

```rust
//! File classification for ranking (spec 2026-07-26 §2a).
//!
//! Pure: classification reads the path string only, never the filesystem
//! (ADR A9). Every input is an untrusted path out of a transcript.
//!
//! **Why classes exist.** On the 2026-07-26 tune split of the author's own
//! corpus, documentation files were 192 of 552 (session, file) pairs and
//! carried 1 of 39 recurrence outcomes; config files were 37 pairs and
//! carried none. Code files were 285 pairs and carried 34. Ranking code above
//! documentation cut flagged files from 65 to 52 with an identical hit count.
//!
//! **What the tiers are and are not.** Only the code-versus-docs-and-config
//! boundary rests on adequate data. `Notes` (19 pairs, 3 outcomes) showed a
//! HIGHER outcome rate than code, 0.158 against 0.119, which on three
//! outcomes is far too thin to promote it above code, so it sits directly
//! below code rather than beside documentation. `Web` (7 pairs) is grouped
//! with code because web files are code-like, not because 7 pairs measured
//! anything. Read the tier order as a declared judgment on thin data
//! everywhere except that one boundary.

use serde::Serialize;

/// What kind of file a path names, for ranking purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileClass {
    /// Source code.
    Code,
    /// Markup and stylesheets. Ranks with [`FileClass::Code`].
    Web,
    /// Running notes and agent memory files.
    Notes,
    /// Prose documentation.
    Docs,
    /// Configuration, lockfiles, and environment files.
    Config,
    /// Anything unrecognized, including extensionless files and binaries.
    Other,
}

impl FileClass {
    /// Ranking tier, lower sorts first. Not the enum's declaration order:
    /// tiers are deliberately coarse so that two classes can tie.
    pub fn tier(self) -> u8 {
        match self {
            FileClass::Code | FileClass::Web => 0,
            FileClass::Notes => 1,
            FileClass::Docs => 2,
            FileClass::Config | FileClass::Other => 3,
        }
    }
}

const CODE_EXT: &[&str] = &[
    "rs", "py", "ts", "tsx", "js", "jsx", "go", "java", "c", "h", "cc", "cpp",
    "hpp", "rb", "swift", "kt", "sh", "bash", "zsh", "sql", "vue", "svelte",
    "cs", "php", "lua", "scala", "dart", "ex", "exs", "clj", "hs", "ml", "m",
    "mm", "r", "pl",
];
const WEB_EXT: &[&str] = &["html", "css", "scss", "sass", "less"];
const DOCS_EXT: &[&str] = &["md", "mdx", "txt", "rst", "adoc", "tex"];
const CONFIG_EXT: &[&str] = &[
    "json", "toml", "yaml", "yml", "ini", "cfg", "conf", "env", "lock",
    "properties", "gradle", "xml", "plist",
];

/// Classify a path. Precedence matters and is tested:
///
/// 1. A basename starting with `.env` is [`FileClass::Config`], so
///    `.env.local` and `.env.production` are caught alongside `.env`.
/// 2. A memory or notes path is [`FileClass::Notes`], checked BEFORE
///    extensions so `memory/plan.md` is notes rather than documentation.
/// 3. Extension tables, in the order code, docs, config, web.
/// 4. Everything else is [`FileClass::Other`].
pub fn classify(path: &str) -> FileClass {
    let lower = path.to_ascii_lowercase();
    // `rsplit('/')` always yields at least one item, so a bare filename with
    // no directory component still lands here.
    let name = lower.rsplit('/').next().unwrap_or(lower.as_str());

    if name.starts_with(".env") {
        return FileClass::Config;
    }
    if lower.contains("/memory/") || name.starts_with("memory.") || lower.contains("/notes") {
        return FileClass::Notes;
    }

    // `rsplit_once` on the BASENAME, so a dot in a parent directory cannot be
    // mistaken for an extension. A leading-dot file like `.gitignore` yields
    // ("", "gitignore"), which matches no table and falls through to Other.
    let Some((_stem, ext)) = name.rsplit_once('.') else {
        return FileClass::Other;
    };
    if CODE_EXT.contains(&ext) {
        FileClass::Code
    } else if DOCS_EXT.contains(&ext) {
        FileClass::Docs
    } else if CONFIG_EXT.contains(&ext) {
        FileClass::Config
    } else if WEB_EXT.contains(&ext) {
        FileClass::Web
    } else {
        FileClass::Other
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p sumcp-core file_class`
Expected: PASS, 8 tests.

- [ ] **Step 5: Verify the whole workspace and the lints**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all three clean. `file_class` is not yet used by anything, which is
intentional: it lands reviewable on its own.

- [ ] **Step 6: Commit**

```bash
git add crates/sumcp-core/src/file_class.rs crates/sumcp-core/src/lib.rs
git commit -m "core: file_class, path to Code/Web/Notes/Docs/Config/Other

Pure path classification, no filesystem access (ADR A9). Used by nothing yet.

Motivated by the 2026-07-26 tune split: documentation was 192 of 552
(session, file) pairs and carried 1 of 39 recurrence outcomes, config 37 pairs
and none, code 285 pairs and 34. The rustdoc records which tier boundaries the
data actually supports (code versus docs and config) and which are declared
judgments on thin cells (notes at 19 pairs, web at 7)."
```

---

### Task 4: The new ordering

**Files:**
- Modify: `crates/sumcp-core/src/score.rs:78-89` (`FileScore`), `:143-186`
  (`rank`)
- Test: `crates/sumcp-core/src/score.rs` test module

**Interfaces:**
- Consumes: `file_class::{FileClass, classify}` from Task 3.
- Produces: `FileScore` gains `pub class: FileClass` and `pub edits: u64` and
  keeps `score: f64` for now, so every existing caller still compiles. `rank`
  keeps its `&Weights` parameter for now. Task 5 removes both.

This task changes ordering only. Keeping `score` and `Weights` for one more
task is what makes it independently compilable and testable.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/sumcp-core/src/score.rs`:

```rust
    /// Read-only files carry ReRead findings and so enter the ranking with
    /// zero edits. On the demo fixture a never-edited `.jpg` ranked FOURTH,
    /// above a `.py` file whose commands were failing, purely for having been
    /// read four times. A review queue is about changes, so anything unedited
    /// sorts last regardless of class.
    #[test]
    fn edited_files_outrank_unedited_ones() {
        let read = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Read","input":{{"file_path":"{file}"}}}}]}}}}"#
            )
        };
        let mut lines: Vec<String> = (0..4)
            .map(|i| read(&format!("r{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hero.jpg"))
            .collect();
        // One edited code file, fewer signals than the read-thrashed image.
        for i in 0..2 {
            lines.push(edit(
                &format!("e{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].file, "/a/main.rs", "edited file first");
        assert_eq!(ranked[0].edits, 2);
        assert_eq!(ranked.last().unwrap().file, "/a/hero.jpg");
        assert_eq!(ranked.last().unwrap().edits, 0, "never edited");
    }

    #[test]
    fn code_outranks_docs_even_with_fewer_edits() {
        let mut lines = Vec::new();
        // Docs edited 5x, code edited 2x. Code still wins on class.
        for i in 0..5 {
            lines.push(edit(
                &format!("d{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/a/NOTES-FOR-RELEASE.md",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("c{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/main.rs",
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].file, "/a/main.rs");
        assert_eq!(ranked[0].class, crate::file_class::FileClass::Code);
        assert_eq!(ranked[1].file, "/a/NOTES-FOR-RELEASE.md");
        assert_eq!(ranked[1].class, crate::file_class::FileClass::Docs);
    }

    #[test]
    fn within_a_class_more_edits_ranks_first_and_path_breaks_ties() {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(&format!("h{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hot.rs"));
        }
        for i in 0..2 {
            lines.push(edit(&format!("m{i}"), &format!("2026-01-01T00:01:0{i}Z"), "/a/mid.rs"));
        }
        // Same edit count as mid.rs, so only the path can separate them.
        for i in 0..2 {
            lines.push(edit(&format!("z{i}"), &format!("2026-01-01T00:02:0{i}Z"), "/a/also.rs"));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let files: Vec<&str> = rank(&s, &Weights::default())
            .iter()
            .map(|f| f.file.as_str())
            .collect();
        assert_eq!(files, vec!["/a/hot.rs", "/a/also.rs", "/a/mid.rs"]);
    }

    #[test]
    fn edits_counts_writes_as_well_as_edits() {
        let write = |id: &str, ts: &str, file: &str| {
            format!(
                r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Write","input":{{"file_path":"{file}","content":"x"}}}}]}}}}"#
            )
        };
        let raw = format!(
            "{}\n{}\n{}",
            write("w1", "2026-01-01T00:00:01Z", "/a/main.rs"),
            edit("e1", "2026-01-01T00:00:02Z", "/a/main.rs"),
            edit("e2", "2026-01-01T00:00:03Z", "/a/main.rs"),
        );
        let s = ingest_str(&raw, Lane::Main);
        let ranked = rank(&s, &Weights::default());
        assert_eq!(ranked[0].edits, 3, "Write counts toward edits");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p sumcp-core score::tests`
Expected: FAIL to compile with `no field edits on type FileScore` and
`no field class on type FileScore`.

- [ ] **Step 3: Add the two fields to `FileScore`**

Replace `crates/sumcp-core/src/score.rs:78-89` with:

```rust
/// One file's place in the ranking, with the evidence that explains it.
#[derive(Debug, Clone, Serialize)]
pub struct FileScore {
    /// The file path.
    pub file: String,
    /// What kind of file this is. First ranking key after edited-ness.
    pub class: crate::file_class::FileClass,
    /// How many Edit or Write actions targeted this file. Second ranking key.
    pub edits: u64,
    /// The weighted score. Retained for one task only; the ranking no longer
    /// consults it (spec 2026-07-26 §2b removes it next).
    pub score: f64,
    /// Per-category magnitudes (churn/rework/failure_loops/re_read/fumbles/action_loops).
    pub breakdown: BTreeMap<String, u64>,
    /// The findings backing this file, in a stable order.
    pub findings: Vec<Finding>,
}
```

- [ ] **Step 4: Count edits and change the sort**

Add above `rank` in `crates/sumcp-core/src/score.rs`:

```rust
/// Edit/Write actions per file. Not a signal: the ranking's second key and a
/// displayed number, so it counts ATTEMPTS exactly as `Overview::edits` does
/// rather than only confirmed successes.
fn edit_counts(s: &Session) -> BTreeMap<&str, u64> {
    let mut out: BTreeMap<&str, u64> = BTreeMap::new();
    for a in &s.actions {
        if matches!(a.kind, ActionKind::Edit | ActionKind::Write)
            && let Some(f) = a.file_path.as_deref()
        {
            *out.entry(f).or_insert(0) += 1;
        }
    }
    out
}
```

Add `ActionKind` to the `use crate::model::{...}` list at
`crates/sumcp-core/src/score.rs:18`.

Then in `rank`, immediately after the `let mut acc: BTreeMap<String, Acc> =
BTreeMap::new();` line, add:

```rust
    let edits = edit_counts(s);
```

Replace the `.map(...)` closure that builds each `FileScore` with:

```rust
        .map(|(file, (score, breakdown, findings))| {
            let edits = edits.get(file.as_str()).copied().unwrap_or(0);
            FileScore {
                class: crate::file_class::classify(&file),
                edits,
                file,
                score,
                breakdown,
                findings,
            }
        })
```

Replace the `scores.sort_by(...)` call and its comment with:

```rust
    // The ranking rule, in full. Four keys, each one checkable by hand
    // against the rendered report (spec 2026-07-26 §2b):
    //   1. edited files before never-edited ones, because a file with no
    //      change has nothing to review;
    //   2. class tier, because documentation and config churn does not
    //      predict recurrence (see file_class's module doc);
    //   3. edit count, descending;
    //   4. path, so the order is total and stable.
    // Deliberately NOT a weighted sum: fitting weights to maximize hits with
    // the outcomes in hand bought at most 4 hits out of 39 on the only corpus
    // this has been measured against, and the fit put maximum weight on edit
    // count anyway (docs/validation/2026-07-26-ceiling-analysis.md).
    scores.sort_by(|a, b| {
        (a.edits == 0)
            .cmp(&(b.edits == 0))
            .then_with(|| a.class.tier().cmp(&b.class.tier()))
            .then_with(|| b.edits.cmp(&a.edits))
            .then_with(|| a.file.cmp(&b.file))
    });
```

- [ ] **Step 5: Run the new tests**

Run: `cargo test -p sumcp-core score::tests`
Expected: PASS. The four new tests pass, and the pre-existing tests
`ranking_is_transparent_and_ordered`,
`tiny_relative_churn_halves_the_churn_contribution`, and
`action_loop_contributes_at_half_weight` still pass, because `score` is still
computed and their fixtures are all same-class files ordered by edit count.

- [ ] **Step 6: Run the whole workspace**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all clean. If an `html.rs` or `payloads.rs` test fails on ordering,
read the failure: it is telling you a fixture's expected order changed, which
is the intended behavior, and the assertion should be updated to the new order.

- [ ] **Step 7: Eyeball the fixture, which is the whole point**

```bash
cargo build --release -p sumcp-cli
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl 2>&1 \
  | sed -n '/struggle areas/,/^$/p'
```

Expected: the `.py` files now rank above the `.md` files, and the `.jpg` is
last. Before this change the order was `.py`, `.md`, `.md`, `.jpg`, `.py`.

- [ ] **Step 8: Commit**

```bash
git add crates/sumcp-core/src/score.rs
git commit -m "core: rank by edited-first, class, edit count, path

Replaces the weighted sum as the ORDERING key. The score field and Weights
survive one more commit so every caller still compiles; the next commit
removes both.

On the demo fixture the old order put two markdown files and a never-edited
JPEG above a .py file whose commands were failing. The JPEG ranked on
re-reads alone. Four declared keys fix that and can be checked by hand
against the report."
```

---

### Task 5: Remove the score and the weights, land payload contract v1

**Files:**
- Modify: `crates/sumcp-core/src/score.rs` (delete `Weights`,
  `REL_CHURN_CLAMP`, `category_weight`, `finding_multiplier`; drop the `score`
  field and the `&Weights` parameter)
- Modify: `crates/sumcp-core/src/payloads.rs:236`, `:326-390`, six `"v": 0`
  sites at `:273`, `:384`, `:459`, `:509`, `:550`, `:620`, test at `:784`, and
  the `struggle_areas` call sites at `:773`, `:833`, `:940`, `:983`, `:1076`
- Modify: `crates/sumcp-core/src/html.rs:46-50`, `:69`, `:511-578`, `:825`,
  `:974-975`, test at `:1226`
- Modify: `crates/sumcp-cli/src/main.rs:14`, `:220`, `:229`, `:248-261`
- Modify: `crates/sumcp-mcp/src/main.rs:13-52`, `:99`, tests at `:113-129`
- Modify: `crates/sumcp-mcp/src/server.rs:19`, `:44-46`, `:300`, `:306`, `:346`
- Modify: `crates/sumcp-mcp/src/identify.rs:296`,
  `crates/sumcp-mcp/tests/stdio.rs:218`, `:282`
- Modify: `crates/sumcp-core/examples/validity_dump.rs:20`, `:41`
- Modify: all 7 files in `fixtures/mock-payloads/*.json`
- Modify: `scripts/check_payloads.py:29-30`, `:85`, `:94-98`
- Modify: `docs/payload-schema.md:54-55`, and append a v1 section

**Interfaces:**
- Consumes: `FileScore` with `class` and `edits` from Task 4.
- Produces: `pub const RANKING_RULE: &str` in `score.rs`; `rank(s: &Session) ->
  Vec<FileScore>`; `render_html(s: &Session, ranked: &[FileScore], meta:
  &SessionMeta) -> String`; `struggle_areas(ranked: &[FileScore], meta:
  &SessionMeta, n: usize) -> Value`. `Weights` no longer exists.

This task is deliberately atomic. CI runs `check_payloads.py`, so the Rust
builders, the mocks, the checker, and the schema doc must move together or the
build is red.

- [ ] **Step 1: Write the failing payload tests**

In `crates/sumcp-core/src/payloads.rs`, replace the test
`struggle_areas_echoes_weights_and_breakdown` (at line 830) with:

```rust
    #[test]
    fn struggle_areas_echoes_the_ranking_rule_and_breakdown() {
        let s = churny_session();
        let p = struggle_areas(&rank(&s), &meta(), 5);
        assert_eq!(p["v"], 1);
        // SPEC §7: ranking output is never an opaque number. The rule that
        // produced the order ships with the order.
        assert_eq!(p["ranking_rule"], crate::score::RANKING_RULE);
        assert!(p["files"][0]["breakdown"].is_object());
        assert!(p["files"][0]["class"].is_string());
        assert!(p["files"][0]["edits"].is_u64());
        assert!(
            p["files"][0].get("score").is_none(),
            "the weighted score is gone, not renamed"
        );
        assert!(p.get("weights").is_none(), "weights are gone");
    }

    #[test]
    fn session_overview_top_struggles_carry_class_and_edits() {
        let s = churny_session();
        let ranked = rank(&s);
        let p = session_overview(&s, &ranked, &meta());
        assert_eq!(p["v"], 1);
        let top = &p["top_struggles"][0];
        assert!(top["class"].is_string());
        assert!(top["edits"].is_u64());
        assert!(top.get("score").is_none());
    }
```

If a helper named `churny_session()` does not already exist in that test
module, add it, reusing the module's existing `edit` helper:

```rust
    /// Two same-class files with different edit counts: enough to rank.
    fn churny_session() -> crate::model::Session {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(&format!("h{i}"), &format!("2026-01-01T00:00:0{i}Z"), "/a/hot.rs"));
        }
        for i in 0..2 {
            lines.push(edit(&format!("w{i}"), &format!("2026-01-01T00:01:0{i}Z"), "/a/warm.rs"));
        }
        crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main)
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p sumcp-core payloads::tests::struggle_areas_echoes_the_ranking_rule_and_breakdown`
Expected: FAIL to compile, because `struggle_areas` still takes four arguments
and `RANKING_RULE` does not exist.

- [ ] **Step 3: Strip `score.rs`**

In `crates/sumcp-core/src/score.rs`:

1. Delete the `Weights` struct, its `impl Default`, `REL_CHURN_CLAMP`,
   `category_weight`, and `finding_multiplier` entirely.
2. Delete `use serde::{Deserialize, Serialize};` and replace with
   `use serde::Serialize;` (`FileScore` still derives `Serialize`).
3. Remove `Confidence` from the `use crate::model::{...}` list if nothing else
   in the file uses it after the weighted sum is gone.
4. Replace the module doc at lines 1-16 with:

```rust
//! Ranking: a stated rule, not a score (spec 2026-07-26 §2b).
//!
//! Files are ordered by four keys, in this order:
//!
//! 1. edited files before never-edited ones, because a file with no change
//!    has nothing to review;
//! 2. [`crate::file_class::FileClass::tier`], because documentation and
//!    config churn does not predict recurrence;
//! 3. edit count, descending;
//! 4. path, so the order is total and stable.
//!
//! **Why there is no weighted score.** Until 2026-07-26 this module summed
//! `weight[category] * magnitude * confidence_factor` per file. Fitting those
//! weights to maximize hits WITH THE OUTCOMES IN HAND bought at most 4 hits
//! out of 39 on the only corpus it has been measured against, and the fit
//! assigned maximum weight to edit count regardless. A tuned sum that cannot
//! beat counting edits even while cheating is not worth the opacity it costs,
//! so the sum, the `Weights` type, and its TOML override (ADR A6) are gone.
//! See `docs/validation/2026-07-26-ceiling-analysis.md`.
//!
//! The findings stay. They are the explanation and the citations, and every
//! one still carries its tier, its exact-versus-heuristic flag, its
//! confidence, and the action indices that prove it.

/// The ranking rule, as one sentence. A constant so the payload, the HTML
/// report, and the terminal output cannot drift apart.
pub const RANKING_RULE: &str =
    "edited files first, then code before docs and config, then by edit count, ties by path";
```

5. Change the signature to `pub fn rank(s: &Session) -> Vec<FileScore>` and
   delete the `factor`, `contribution`, and `entry.0 += contribution` lines,
   the `Acc` tuple's `f64` slot, and the `score` field from the `FileScore`
   construction. `Acc` becomes
   `type Acc = (BTreeMap<String, u64>, Vec<Finding>);`.
6. Delete the `score: f64` field from `FileScore` and its doc comment.
7. Update every `rank(&s, &Weights::default())` and `rank(&s, &w)` inside this
   file's own tests to `rank(&s)`, and delete the tests
   `default_weight_order_matches_evidence_strength`,
   `tiny_relative_churn_halves_the_churn_contribution`, and
   `action_loop_contributes_at_half_weight`, which assert weighted-sum
   behavior that no longer exists. Change
   `ranking_is_transparent_and_ordered`'s final assertion from
   `assert!(ranked[0].score > ranked[1].score);` to
   `assert!(ranked[0].edits > ranked[1].edits);`.

- [ ] **Step 4: Update `payloads.rs`**

1. Line 10: `use crate::score::{FileScore, Weights};` becomes
   `use crate::score::FileScore;`.
2. Line 236, in `session_overview`'s `top` builder:

```rust
            json!({
                "file": elide_middle(&f.file, PATH_MAX),
                "class": f.class, "edits": f.edits,
                "breakdown": f.breakdown
            })
```

3. Replace the `struggle_areas` doc line 326 and signature at 335-353 with:

```rust
/// `struggle_areas(n)`: ranked files with breakdown, ranking rule, findings.
///
/// Three caps stack, in the order the schema advertises (tail-first):
/// `n` is clamped to `STRUGGLE_FILES_MAX`, findings per file are capped and
/// chosen to represent the breakdown (`representative_findings`), and then
/// the payload is rebuilt smaller until it fits `CAP_STRUGGLE`: lowest-ranked
/// files dropped first, and only once a single file is left do its findings
/// start going. Measured before this existed: `n=99` on an ordinary 12-file
/// session produced 2827 tokens, and a 200-file session 1.6M.
pub fn struggle_areas(ranked: &[FileScore], meta: &SessionMeta, n: usize) -> Value {
    // `n` arrives straight from an MCP caller and was honored verbatim.
    let n = n.min(STRUGGLE_FILES_MAX);
    let (session, id_cut) = session_block(meta);
```

The `weights_json` block and its `long_source` elision go away entirely:
`RANKING_RULE` is a compile-time constant, so there is no longer a
caller-controlled string in this payload to truncate.

4. Line 364, in the per-file entry:

```rust
                let mut entry = json!({
                    "rank": i + 1, "file": elide_middle(&f.file, PATH_MAX),
                    "class": f.class, "edits": f.edits,
                    "breakdown": f.breakdown,
                    "findings": kept.iter().map(|f| compact_finding(f)).collect::<Vec<_>>()
                });
```

5. Line 386: `"weights": weights_json,` becomes
   `"ranking_rule": crate::score::RANKING_RULE,`.
6. All six `"v": 0,` sites become `"v": 1,`.
7. Line 784: `assert_eq!(payload["v"], 0);` becomes `assert_eq!(payload["v"], 1);`.
8. Every `struggle_areas(&r, &w, &m, 10)`-shaped call in the test module drops
   the weights argument, and every `rank(&s, &w)` becomes `rank(&s)`. Delete
   any now-unused `let w = Weights::default();` bindings.

- [ ] **Step 5: Update `html.rs`**

1. Line 46-50: drop the `weights: &Weights` parameter, leaving
   `pub fn render_html(s: &Session, ranked: &[FileScore], meta: &SessionMeta) -> String`.
   Remove `Weights` from the file's `use` list.
2. Line 69: `h.push_str(&struggles_section(ranked, &review));`
3. Line 515: `struggles_section` drops its `weights: &Weights` parameter.
4. Lines 539-547, the row: replace the score cell with class and edits.

```rust
        let _ = write!(
            rows,
            "<tr{top}><td class=\"r\">{rank}</td><td>{file_cell}</td>\
             <td>{class}</td><td class=\"r\">{edits}</td><td>{phrases}</td></tr>",
            top = if i < 3 { " class=\"top\"" } else { "" },
            rank = i + 1,
            class = esc(&format!("{:?}", f.class).to_lowercase()),
            edits = f.edits,
            phrases = esc(&phrases.join(", ")),
        );
```

5. Lines 559-572, the footnote, becomes the rule itself:

```rust
    let footnote = format!(
        "<p class=\"foot\">ranked by: {rule}. No weighted score: on the only \
         corpus this has been measured against, no weighting over the \
         observable signals beat counting edits.</p>",
        rule = esc(crate::score::RANKING_RULE),
    );
```

6. Lines 573-578, the table header gains two columns:

```rust
    format!(
        "<section class=\"sec\"><h2>Struggle areas</h2>\
         <table class=\"tbl\"><thead><tr><th>#</th><th>file</th>\
         <th>class</th><th>edits</th><th>signals</th></tr></thead>\
         <tbody>{rows}</tbody></table>{overflow}{footnote}</section>"
    )
```

7. Line 824-827, the story `why_line`:

```rust
        let why_line = match c.ranked {
            Some(fs) => format!(
                "{} · edited {}x · {}",
                esc(&format!("{:?}", fs.class).to_lowercase()),
                fs.edits,
                esc(&why)
            ),
            None => esc(&why),
        };
```

8. Lines 974-975 in the test module: `rank(&s)` and
   `render_html(&s, &r, &meta())`.
9. Rename the test at line 1226 to
   `struggle_breakdown_is_plain_language_with_ranking_rule_footnote` and
   replace its `assert!(html.contains("rework 3"), "actual weights echoed");`
   with `assert!(html.contains(crate::score::RANKING_RULE), "rule echoed");`.

- [ ] **Step 6: Update the two binaries**

`crates/sumcp-cli/src/main.rs`:

- Line 14: `use sumcp_core::score::rank;`
- Line 220: `let ranked = rank(&session);`
- Line 229: `sumcp_core::html::render_html(&session, &ranked, &meta)`
- Lines 254-260, the terminal line:

```rust
            println!(
                "{}. {}  ({}, edited {}x: {})",
                i + 1,
                f.file,
                format!("{:?}", f.class).to_lowercase(),
                f.edits,
                cats.join(", ")
            );
```

`crates/sumcp-mcp/src/server.rs`:

- Line 19: `use sumcp_core::score::rank;`
- Lines 44-46: delete the `pub weights: Weights,` field and its doc comment,
  and change the struct doc at line 38 to
  `/// The server: project directory to scan and parsed-session cache.`
- Line 300: `let ranked = rank(&session);`
- Line 306: `payloads::struggle_areas(&ranked, &meta, n)`
- Line 346: delete `weights: Weights::default(),` from the test helper.

`crates/sumcp-mcp/src/main.rs`:

- Delete `load_weights_from` (lines 15-52), the `use sumcp_core::score::Weights;`
  at line 13, and the tests `missing_config_yields_defaults` and
  `partial_toml_overrides_and_records_source`.
- Keep `config_path`, `config_path_from`, and the
  `empty_or_relative_xdg_config_home_is_ignored` test: they now serve the
  notice below, and the XDG hardening they encode is still worth keeping.
- Add, replacing the deleted loader:

```rust
/// ADR A6 retired (spec 2026-07-26 §2f): ranking has no weights to configure,
/// so `~/.config/sumcp/config.toml` is no longer read. A user who wrote one is
/// told rather than silently ignored.
fn warn_if_stale_config(path: Option<PathBuf>) {
    if let Some(path) = path
        && path.exists()
    {
        eprintln!(
            "sumcp-mcp: {} is no longer read (ranking weights were removed; \
             see docs/validation/2026-07-26-ceiling-analysis.md)",
            path.display()
        );
    }
}
```

- Line 94-100, the server construction:

```rust
    warn_if_stale_config(config_path());
    let server = server::SumcpServer {
        // Claude Code launches project-scoped stdio servers with cwd = the
        // project root, so this resolves to the right transcript directory.
        project_dir: sumcp_core::locate::project_dir(&home, &cwd),
        store: store::SessionStore::new(),
    };
```

- If `toml` is now unused in `sumcp-mcp`, remove it from that crate's
  `Cargo.toml` dependencies and run `cargo update -p sumcp-mcp` so
  `Cargo.lock` stays in step. Verify with
  `grep -rn 'toml::' crates/sumcp-mcp/src` returning nothing first.

`crates/sumcp-core/examples/validity_dump.rs`:

- Line 20: `use sumcp_core::score::{all_findings, rank};`
- Line 41: `let ranked = rank(&session);`

- [ ] **Step 7: Update the remaining `v` assertions**

- `crates/sumcp-mcp/src/identify.rs:296`: `assert_eq!(p["v"], 1);`
- `crates/sumcp-mcp/tests/stdio.rs:218` and `:282`:
  `assert_eq!(overview["v"], 1);`

- [ ] **Step 8: Update the mock payloads**

In all 7 files under `fixtures/mock-payloads/`, change `"v":0` to `"v":1`.

In `fixtures/mock-payloads/struggle_areas.json`: delete the whole `"weights"`
line, add
`"ranking_rule":"edited files first, then code before docs and config, then by edit count, ties by path",`
in its place, and in each of the three file entries replace
`"score":63.5,` with `"class":"code","edits":24,`, `"score":38.0,` with
`"class":"code","edits":17,`, and `"score":22.5,` with
`"class":"code","edits":10,`. The edit counts match each entry's own
`breakdown.churn`, which is what churn magnitude counts, so the mock stays
internally consistent.

In `fixtures/mock-payloads/session_overview.json`, make the same
`score` to `class`/`edits` replacement in all three `top_struggles` entries.

- [ ] **Step 9: Update the payload checker**

In `scripts/check_payloads.py`:

- Line 29-30:

```python
# payloads whose top-level content is ranked: they must echo the RULE that
# produced the order, never an opaque score (SPEC §7)
RANKING_PAYLOADS = {"struggle_areas"}
```

- Line 85: `if payload.get("v") != 1:` and the message becomes
  `"missing/wrong schema version 'v' (expected 1)"`.
- Lines 94-98:

```python
    if name in RANKING_PAYLOADS:
        rule = payload.get("ranking_rule")
        if not (isinstance(rule, str) and rule.strip()):
            errors.append("ranking payload must echo a non-empty 'ranking_rule'"
                          " (SPEC §7, never opaque)")
        if "weights" in payload:
            errors.append("'weights' was removed in v1; ranking has no weights")
        files = payload.get("files", [])
        if not any("breakdown" in f for f in files):
            errors.append("ranking payload must show per-file 'breakdown'")
        for f in files:
            if "score" in f:
                errors.append("v1 removed the opaque per-file 'score'")
            for field in ("class", "edits"):
                if field not in f:
                    errors.append(f"ranked file missing '{field}'")
```

- In `check_error`, line 106: `if payload.get("v") != 1:`.
- Add, so the overview's ranked entries are held to the same rule:

```python
def check_overview_top_struggles(payload) -> list[str]:
    """session_overview embeds ranked entries too, and the same v1 rule
    applies to them: class and edits, never an opaque score."""
    errors = []
    for entry in payload.get("top_struggles", []):
        if "score" in entry:
            errors.append("v1 removed the opaque 'score' from top_struggles")
        for field in ("class", "edits", "breakdown"):
            if field not in entry:
                errors.append(f"top_struggles entry missing '{field}'")
    return errors
```

and call it from `check_success` when `name == "session_overview"`.

- [ ] **Step 10: Update the schema doc**

In `docs/payload-schema.md`, replace lines 54-55 with:

```markdown
All ranking output shows the per-category `breakdown` and the `ranking_rule`
that produced the order. There is no score: see the v1 section below.
```

Append at the end of the file:

```markdown
## 2026-07-26 BREAKING: `v` goes 0 to 1 (spec 2026-07-26)

The weighted score is gone, so two payloads change shape. Every payload's `v`
becomes `1`.

| payload | removed | added |
|---|---|---|
| `struggle_areas` | `weights` object, per-file `score` | `ranking_rule` string, per-file `class` and `edits` |
| `session_overview` | `top_struggles[].score` | `top_struggles[].class`, `top_struggles[].edits` |

`class` is one of `code`, `web`, `notes`, `docs`, `config`, `other`. `edits`
counts Edit and Write attempts against that file.

Why: fitting ranking weights to maximize hits with the outcomes in hand bought
at most 4 hits out of 39 on the only corpus this has been measured against, and
the fit assigned maximum weight to edit count anyway. The order is now four
declared keys a reader can check by hand, and `ranking_rule` ships alongside
the order so SPEC §7's "never an opaque number" holds more strongly than
before. Full method and tables in
`docs/validation/2026-07-26-ceiling-analysis.md`.
```

- [ ] **Step 11: Run everything**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_payloads.py
python3 scripts/check_narration.py
```

Expected: all five clean. `check_payloads.py` prints no errors. If
`check_narration.py` fails, the debrief mock references a removed field; update
`fixtures/mock-payloads/sample-debrief.md` so it cites `class` and `edits`
rather than a score.

- [ ] **Step 12: Confirm the real binaries still work end to end**

```bash
cargo build --release -p sumcp-cli -p sumcp-mcp
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl --json \
  | python3 -c "
import json,sys
d=json.load(sys.stdin)
assert d['v']==1, d['v']
t=d['top_struggles'][0]
assert 'score' not in t and 'class' in t and 'edits' in t, t
print('overview v1 OK:', json.dumps(t)[:120])
"
./target/release/sumcp --file fixtures/session-2_1_210-subagents.jsonl --html \
  | grep -c "edited files first, then code before docs" \
  && echo "html rule footnote OK"
```

Expected: `overview v1 OK` with a class and an edits count, and a non-zero
grep count for the footnote.

- [ ] **Step 13: Commit**

```bash
git add -A
git commit -m "core: remove the weighted score, payload contract v1

Deletes the weighted sum, the Weights type, and its TOML override (ADR A6
retired). Nothing but ranking ever read them: the detectors in signals/ never
consulted weights, so leaving a public configurable type would have advertised
a knob that changes nothing.

Payloads go v0 to v1: struggle_areas drops weights and per-file score for
ranking_rule plus class and edits, and session_overview's top_struggles does
the same. SPEC 7 holds more strongly than before, since the rule that produced
the order now ships with the order instead of six decimals that did not
explain it.

This also closes the CLI-versus-MCP divergence from the codex review: the CLI
always used Weights::default() while the server loaded the config, so the two
surfaces could rank the same session differently. There is now one rule and no
configuration to diverge on. A user with a stale config gets a notice."
```

---

### Task 6: Documentation claims

**Files:**
- Modify: `docs/metrics.md`, `SPEC.md`, `README.md`, `tasks/todo.md`

**Interfaces:**
- Consumes: `RANKING_RULE` and the Task 2 numbers.
- Produces: no code interface.

- [ ] **Step 1: Rewrite the weight column in `docs/metrics.md`**

Remove the weight column from the signal table, since no weights exist. For
each of the six previously-ranked signals, add what the 2026-07-26 tune split
showed, using exactly these figures:

| signal | pairs | outcomes | rate |
|---|---|---|---|
| churn | 242 | 33 | 0.136 |
| re_read | 97 | 21 | 0.216 |
| rework | 94 | 19 | 0.202 |
| blind_write_attempt | 24 | 0 | 0.000 |
| failure_loop | 4 | 2 | 0.500 |
| true_revert | 2 | 1 | 0.500 |

State these three things in prose, and no more than these:

- `blind_write_attempt` was weighted joint-highest on IDE-Bench's 63% figure
  and fired on 24 pairs with zero outcomes here. Zero in 24 rules out a large
  positive effect. It does not establish the signal is harmful, and it does not
  refute IDE-Bench, whose population is autonomous benchmark trajectories
  rather than interactive sessions.
- `failure_loop` and `true_revert` are too rare here to characterise: there
  were 58 confirmed failed commands across 24 tune sessions, a median of 1 per
  session. That is a property of this corpus, not of the detector.
- `re_read` had the best rate of the frequent kinds while being weighted below
  both `rework` and `fumble`. The literature-derived ordering did not reproduce
  on the only corpus it has been measured against.

Add a `class` row documenting `file_class`, and reproduce the honesty note from
that module's rustdoc verbatim: only the code-versus-docs-and-config boundary
rests on adequate data.

- [ ] **Step 2: Amend `SPEC.md`**

Following the file's existing amendment style, amend decision 6 (transparent
weighted ranking) and ADR A6 (TOML-optional weights). Decision 6 becomes the
four-key rule with `RANKING_RULE` quoted. ADR A6 is marked retired with the
date, the reason, and a pointer to the ceiling analysis. Do not delete the
original text; amend it, so the record of what was decided and why it changed
both survive.

- [ ] **Step 3: Rewrite "The numbers" in `README.md`**

Replace the section with a claim that is exactly what the evidence supports.
It must say all of:

- Flagged files really do recur more than unflagged ones. Every product row in
  the 2026-07-22 study had a relative risk well above 1 with an interval
  excluding 1.
- The ranking is a four-key rule a reader can verify by hand, not a score. No
  weighting over the observable signals beat counting edits, and the ceiling
  analysis measured that rather than assuming it.
- Restricting the queue to code cut flagged files from 65 to 52 for an
  identical hit count on the tune split, which is a 20% reduction in false
  alarms at no cost to recall.
- Every entry carries deterministic evidence: the exact actions, cited.

It must NOT claim the ranking is more accurate than any alternative. Keep the
existing token-reduction paragraph and the `Limitations` section, and add the
single-author-corpus and 30-day-cleanup caveats to `Limitations`.

- [ ] **Step 4: Close the decision in `tasks/todo.md`**

Tick "Decide what v0.1 claims" and record: option (b) was chosen, a feasibility
pass measured that the goal is unreachable on this corpus, the score was
demoted rather than retuned, and the evidence is in
`docs/validation/2026-07-26-ceiling-analysis.md`. Add a new unticked item for
refreshing the corpus archive before any future validation run, and note that
`cleanupPeriodDays` is still unset.

- [ ] **Step 5: Check the prose gates**

```bash
for f in docs/metrics.md SPEC.md README.md tasks/todo.md; do
  echo "$f em-dashes: $(grep -c '—' $f)"
done
grep -rn "/Users/" README.md docs/metrics.md SPEC.md | head
```

Expected: zero em dashes in every file, and no output from the path grep.

- [ ] **Step 6: Commit**

```bash
git add docs/metrics.md SPEC.md README.md tasks/todo.md
git commit -m "docs: signal evidence instead of weight tiers, close the v0.1 claim

metrics.md drops the weight column, which describes a mechanism that no
longer exists, and records what each signal actually did on the 2026-07-26
tune split. The blind-write row states the limit of its own evidence: zero
outcomes in 24 pairs rules out a large positive effect and does not refute
IDE-Bench, whose population is autonomous trajectories rather than
interactive sessions.

SPEC decision 6 amended to the four-key rule; ADR A6 marked retired. README
'The numbers' now claims what is supported: the flags are predictive, the
order is a rule you can check by hand, restricting to code cut flags 65 to 52
for the same hits, and every entry is cited. No accuracy claim."
```

---

### Task 7: Publish the negative result and run the release gate

**Files:**
- Create: `docs/validation/2026-07-26-ceiling-analysis.md`
- Modify: `docs/validation/2026-07-22-predictive-validity.md` (pointer only)
- Modify: `docs/assets/report-hero.png`

**Interfaces:**
- Consumes: the Task 2 output.

- [ ] **Step 1: Write the validation report**

Create `docs/validation/2026-07-26-ceiling-analysis.md` containing, in this
order: status and scope line matching the other validation docs; the question;
the method including why the fit is deliberately overfit and why there are two
fitters; the corpus line (sessions, projects, tune pairs, positives, held-out
project id, pairs withheld); the reference-rule table; one table per budget with
edit count, both fitters, the in-sample bound, the margin, and
leave-one-project-out; the gate as it was written before the run and the result;
then the caveats.

Caveats must include all five: single-author single-machine corpus;
leave-one-project-out has three coarse folds and non-monotonic numbers so it is
a direction not a measurement; an in-sample bound is optimistic by construction
which is what makes it usable as a bound; failure signals are scarce because
failures are scarce (58 across 24 sessions, median 1); and the corpus is a
30-day rolling window that was actively losing sessions until it was archived,
with one frozen holdout project already lost.

Copy the numbers from the Task 2 run output. Do not retype them from this plan:
Task 2 adds magnitude features that the brainstorm's preliminary run did not
have, so its numbers will differ.

- [ ] **Step 2: Add the forward pointer**

At the top of `docs/validation/2026-07-22-predictive-validity.md`, under the
existing `Status:` block, add:

```markdown
Superseded in part by `docs/validation/2026-07-26-ceiling-analysis.md`, which
answers the question this study raised: no weighting over the observable
features beats counting edits, so the weighted score was removed rather than
retuned. The numbers below stand as recorded.
```

- [ ] **Step 3: Regenerate the hero screenshot**

The ranking order changed, so `docs/assets/report-hero.png` is stale and the
README's main image would misrepresent the tool.

```bash
cargo build --release -p sumcp-cli
./target/release/sumcp --file fixtures/demo/*.jsonl --html > /tmp/hero.html
open /tmp/hero.html
```

This step needs a human: screenshot the report at 820px wide to match the
README's `width="820"`, and save over `docs/assets/report-hero.png`. Confirm
the new image shows code files above documentation. If no demo fixture exists
at that glob, use `fixtures/session-2_1_210-subagents.jsonl`.

- [ ] **Step 4: Verify the report is committable**

```bash
F=docs/validation/2026-07-26-ceiling-analysis.md
echo "em dashes: $(grep -c '—' $F)"
grep -cE '/Users/|raphaelhaytene' $F
grep -oE 'proj-[0-9]+' $F | sort -u
```

Expected: zero em dashes, zero real-path matches, and only anonymized
`proj-NN` identifiers.

- [ ] **Step 5: Commit**

```bash
git add docs/validation/2026-07-26-ceiling-analysis.md \
        docs/validation/2026-07-22-predictive-validity.md \
        docs/assets/report-hero.png
git commit -m "validation: publish the ceiling analysis, refresh the hero

The negative result, with the gate quoted as it was written before the run.
Records all five caveats, including that leave-one-project-out has three
coarse folds and non-monotonic numbers so it is a direction rather than a
measurement, and that the corpus was a 30-day rolling window actively losing
sessions until it was archived.

Hero screenshot regenerated: the old one showed two markdown files and a
never-edited JPEG above a .py file whose commands were failing."
```

- [ ] **Step 6: Run the held-out gate, once**

This is the last step, and it runs exactly once. Never during development.

```bash
python3 scripts/validity_sweep.py --release-eval
cat .superpowers/sdd/validity-heldout-eval.json
```

Per the spec's pre-commitment: the configuration ships regardless of what this
says, and the number is published whatever it says. Because Part 1 forecloses
any accuracy claim, no claim attaches to it. Add the held-out precision and
flag count to the ceiling analysis report as a short closing section, labelled
as a single-project observation, and commit that.

- [ ] **Step 7: Final full verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/check_payloads.py
python3 scripts/check_narration.py
git status --short
```

Expected: all five clean, and a clean tree apart from untracked scratch. Report
the test count so the change in totals from the deleted weight tests is
visible.

---

## Self-Review

**Spec coverage.** Every numbered spec section maps to a task: 1a to Task 1
steps 1-2; 1b to Task 2; 1c to Task 1 steps 4-6; 1d to Task 2 steps 2-4; 2a to
Task 3; 2b to Tasks 4 and 5; 2c to Task 6 step 1; 2d to Task 5 steps 4, 8, 9,
10; 2e to Task 5 steps 5-6; 2f to Task 5 step 6; Part 3 to Tasks 6 and 7. The
spec's testing section is distributed across each task's own verification
steps, and its "docs-only session must not produce an empty report" requirement
is covered by `code_outranks_docs_even_with_fewer_edits` in Task 4 plus the
`FileClass::Docs` tier being a sort key rather than a filter, which is why no
file is ever excluded.

**Known gap, deliberate.** The spec asks for a regression test pinning the demo
fixture's new order. Task 4 step 7 eyeballs it and Task 7 step 3 checks the
screenshot, but there is no automated assertion, because the fixture path
depends on which demo fixture survives and a brittle pin on a large fixture
would fail for unrelated reasons. If the implementer wants one, add it to
`crates/sumcp-cli/tests/html_report.rs`, asserting only that the first ranked
row's class is `code`.

**Type consistency.** `FileClass`, `classify`, and `tier` are named identically
in Tasks 3, 4, and 5. `RANKING_RULE` is defined in Task 5 step 3 and consumed in
steps 4, 5, and 6 and in the Task 5 step 1 test. `rank(s: &Session)` and
`struggle_areas(ranked, meta, n)` are used consistently after Task 5. The
Python `file_class` in Task 2 duplicates the Rust tables on purpose and says so
in its docstring, because the analysis script must not depend on a Rust binary
it does not build.

**Ordering hazard.** Task 4 keeps `score` and `Weights` alive so that task
compiles on its own. Task 5 removes them. Do not merge the two tasks: the point
of the split is that Task 4's ordering change is reviewable without the 15-file
contract break attached to it.

---

## Execution Handoff

Plan complete and saved to
`docs/superpowers/plans/2026-07-26-ceiling-verdict-and-simple-ranking.md`.
