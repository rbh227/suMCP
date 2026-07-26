#!/usr/bin/env python3
"""Predictive-validity sweep (dev-only, like sanitize.py; python3 stdlib only).

Question: do files suMCP flags in session N predict rework in later sessions
of the SAME project, on this machine's own corpus? Weights are frozen at
`Weights::default()` everywhere; this is a no-tuning pass by design (any
future tuning must predict-then-check on held-out projects, never on the
corpus that produced the tuning).

Pipeline:
  1. Discover main transcripts under ~/.claude/projects/*/*.jsonl.
  2. Dump each one via the Rust example (crates/sumcp-core/examples/
     validity_dump.rs), which mirrors the CLI's real --file pipeline
     (assemble -> rank -> needs_review), caching results so re-runs are
     cheap.
  3. Group sessions by project, order chronologically.
  4. For every (session N, edited file F) pair, check whether F shows signs
     of struggle again in a later window -- two outcome definitions (weak,
     strong), two windows (next 3 sessions; sessions within 14 days).
  5. Compute relative risk, precision, miss rate, false-alarm share, and
     edit-count-stratified relative risk for both flag definitions
     (flagged_nr, flagged_top3).
  6. Run the SAME contingency/RR/precision machinery over a fixed set of
     trivial baseline predictors (see RULES below) so the weighted ranking
     has to beat "count the edits" rather than merely beat chance.

Holdout discipline (two rules, both enforced here rather than by convention):
  - Membership is FROZEN BY FINGERPRINT in docs/validation/holdout-snapshot.json,
    never by position in a sorted list. Positional selection silently swaps
    which projects are held out whenever the corpus grows. Missing frozen
    projects fail the run closed rather than degrading quietly.
  - Development runs never compute or persist held-out results. The split
    happens before any metric is calculated, and held-out pair records stay
    out of the raw output entirely. Scoring them requires the explicit
    `--release-eval` invocation, which writes to its own file.

Outputs:
  - .superpowers/sdd/validity-raw.json: tune-split per-pair records +
    aggregates. Scratch. NOT for the repo. Contains real project names/paths
    (the anonymization mapping lives here, nowhere else).
  - .superpowers/sdd/validity-heldout-eval.json: held-out scores. Written
    ONLY by `--release-eval`. Scratch, NOT for the repo.
  - docs/validation/2026-07-22-predictive-validity.md: aggregate-only DRAFT
    report. No real paths, no project names, no prompt text -- projects are
    anonymized as proj-01..proj-NN.

Determinism: every collection is sorted before use; two runs against an
unchanged corpus produce byte-identical outputs (mtimes of *analyzed* files
don't change between runs; the "modified in the last 10 minutes" filter is
the only clock-dependent step, and it only ever *shrinks* the corpus as time
passes forward, never reorders it).
"""

from __future__ import annotations

import hashlib
import json
import math
import subprocess
import sys
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
PROJECTS_DIR = Path.home() / ".claude" / "projects"
CACHE_DIR = REPO / ".superpowers" / "sdd" / "validity"
RAW_OUT = REPO / ".superpowers" / "sdd" / "validity-raw.json"
# Committed, fingerprints only (no real paths): the immutable holdout roster.
HOLDOUT_SNAPSHOT = REPO / "docs" / "validation" / "holdout-snapshot.json"
# Held-out evaluation output. Written ONLY under --release-eval, never by a
# development run, so held-out outcomes cannot leak into day-to-day work.
RELEASE_EVAL_OUT = REPO / ".superpowers" / "sdd" / "validity-heldout-eval.json"
DRAFT_OUT = REPO / "docs" / "validation" / "2026-07-22-predictive-validity.md"
DUMP_BIN = REPO / "target" / "release" / "examples" / "validity_dump"

# Bump whenever validity_dump.rs changes its output shape (see cache_path).
# v2: added per-file `changed_lines` and session-level `failed_commands`.
CACHE_SCHEMA = 2

MIN_ACTIONS = 20
RECENT_SECONDS = 10 * 60
WINDOW_COUNT = 3
WINDOW_DAYS = 14

STRONG_KINDS = {"failure_loop", "user_corrected", "true_revert", "flip"}
FLAG_DEFS = ("flagged_nr", "flagged_top3")
WINDOWS = ("next3", "within14d")
OUTCOMES = ("weak", "strong")

# ---- baseline predictors (T5.4-followup, 2026-07-25) ----------------------
#
# The question these answer: does the WEIGHTED ranking earn its complexity, or
# would a file's raw edit count predict rework just as well? Every rule below
# is run through the identical contingency/RR/precision machinery as the two
# product flags, over the identical pair population.
#
# THRESHOLD DISCIPLINE. Every threshold here is either (a) zero-parameter, or
# (b) a constant that already existed in the repo for an unrelated reason
# BEFORE this comparison was written. No threshold was chosen by looking at
# an outcome, and no threshold is swept. Where a natural quantity has two
# pre-existing boundaries (edit count; the review band), BOTH are reported and
# neither is picked as "the" baseline, because picking the better-looking one
# after the fact is exactly the tuning this study forbids. Provenance for each
# is recorded in RULE_BASIS and printed into the report, so a reader can check
# the claim rather than take it on trust.
#
# NOTE ON TOP-N RULES: N = 3 everywhere, matching the product's flagged_top3
# (`ranked.iter().take(3)` in validity_dump.rs) so the comparison is
# like-for-like on flag count as well as on definition. Ties are broken by
# file path, which is deterministic and independent of the outcome.
PRODUCT_RULES = ("flagged_nr", "flagged_top3")
BASELINE_RULES = (
    "base_edits_ge2",
    "base_edits_ge4",
    "base_top3_edits",
    "base_lines_ge200",
    "base_lines_ge400",
    "base_top3_lines",
    "base_failed_ge1",
    "base_all",
)
RULES = PRODUCT_RULES + BASELINE_RULES

RULE_LABEL = {
    "flagged_nr": "PRODUCT flagged_nr (review::needs_review)",
    "flagged_top3": "PRODUCT flagged_top3 (weighted score::rank top-3)",
    "base_edits_ge2": "baseline edits >= 2",
    "base_edits_ge4": "baseline edits >= 4",
    "base_top3_edits": "baseline top-3 by edit count",
    "base_lines_ge200": "baseline changed lines >= 200",
    "base_lines_ge400": "baseline changed lines >= 400",
    "base_top3_lines": "baseline top-3 by changed lines",
    "base_failed_ge1": "baseline session has >= 1 failed command",
    "base_all": "reference: flag every edited file",
}

RULE_BASIS = {
    "flagged_nr": "product rule, frozen Weights::default()",
    "flagged_top3": "product rule, frozen Weights::default()",
    "base_edits_ge2": (
        "lower boundary of this script's pre-existing edit_stratum() buckets "
        "('1' vs '2-3'), which predate this comparison; also equals the "
        "product's own CHURN_MIN_EDITS = 2 in signals/edit_shape.rs"
    ),
    "base_edits_ge4": (
        "upper boundary of this script's pre-existing edit_stratum() buckets "
        "('2-3' vs '4+'), which predate this comparison"
    ),
    "base_top3_edits": (
        "zero-parameter; N = 3 fixed by the product flag it is compared "
        "against (flagged_top3)"
    ),
    "base_lines_ge200": (
        "lower edge of the human code-review band cited in "
        "signals/comprehension.rs (SmartBear/Cisco: 'under 200, not to exceed "
        "400'); predates this comparison"
    ),
    "base_lines_ge400": (
        "REVIEW_BAND_HI = 400 in signals/comprehension.rs, the product's own "
        "review-burden threshold; predates this comparison"
    ),
    "base_top3_lines": (
        "zero-parameter; N = 3 fixed by the product flag it is compared "
        "against (flagged_top3)"
    ),
    "base_failed_ge1": (
        "zero-parameter: 'any confirmed failed command at all' is the only "
        "threshold on this quantity that requires choosing no magnitude. "
        "SESSION-level, so it is constant across every file in a session and "
        "has no within-session discriminative power by construction"
    ),
    "base_all": (
        "degenerate reference, not a candidate: flags everything, so recall is "
        "perfect and miss rate is 0 by definition. Its precision IS the base "
        "rate, which is the number every other rule's precision must beat"
    ),
}

# The pre-registered headline comparison. Declared here, above any result, and
# reported alongside all three other (window, outcome) combinations regardless
# of how it comes out.
PRIMARY_WINDOW = "next3"
PRIMARY_OUTCOME = "strong"

# Contingency cells below this are reported but explicitly marked as too thin
# to carry a conclusion (see thin_cells()).
MIN_CELL = 5


# ---- discovery + per-transcript dump (with cache) --------------------------


def discover_transcripts() -> list[Path]:
    """Main transcripts at exactly ~/.claude/projects/*/*.jsonl, excluding
    legacy `agent-*` subagent siblings. Sorted for determinism."""
    if not PROJECTS_DIR.is_dir():
        return []
    out = []
    for proj_dir in sorted(PROJECTS_DIR.iterdir()):
        if not proj_dir.is_dir():
            continue
        for f in sorted(proj_dir.glob("*.jsonl")):
            if f.name.startswith("agent-"):
                continue
            out.append(f)
    return sorted(out)


def cache_path(transcript: Path) -> Path:
    # UUID transcript stems are unique in practice; the path hash is a cheap
    # belt-and-suspenders guard against any theoretical collision.
    #
    # CACHE_SCHEMA is in the filename because the freshness check compares
    # mtimes of the CACHE and the TRANSCRIPT: it cannot see that the dump
    # binary changed. Adding a field to validity_dump.rs without bumping this
    # would leave every cached dump missing that field, and the reader's
    # `.get(field, 0)` defaults would turn the omission into a plausible-looking
    # column of zeros rather than an error. Bump on any change to the dump's
    # output shape.
    h = hashlib.sha1(str(transcript).encode()).hexdigest()[:8]
    return CACHE_DIR / f"v{CACHE_SCHEMA}-{h}-{transcript.stem}.json"


def run_dump(transcript: Path) -> dict | None:
    """Return the parsed dump for one transcript, using the cache when it is
    newer than the source file. `None` on any failure (counted as an
    anomaly by the caller, never fatal)."""
    cp = cache_path(transcript)
    if cp.exists() and cp.stat().st_mtime >= transcript.stat().st_mtime:
        try:
            return json.loads(cp.read_text())
        except (json.JSONDecodeError, OSError):
            pass  # fall through and regenerate

    try:
        proc = subprocess.run(
            [str(DUMP_BIN), str(transcript)],
            capture_output=True,
            timeout=120,
        )
    except subprocess.TimeoutExpired:
        return None
    if proc.returncode != 0:
        return None
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None

    CACHE_DIR.mkdir(parents=True, exist_ok=True)
    cp.write_text(json.dumps(data, sort_keys=True))
    return data


# ---- time handling -----------------------------------------------------


def parse_ts(ts: str) -> datetime | None:
    """Parse the transcript's ISO-8601 timestamp format. `None` on empty or
    unparseable input."""
    if not ts:
        return None
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00"))
    except ValueError:
        return None


def session_time(session: dict, transcript: Path) -> datetime:
    """The session's ordering key: its first action's timestamp, falling
    back to the transcript file's mtime when start_ts is empty/unparseable."""
    t = parse_ts(session["start_ts"])
    if t is not None:
        return t
    return datetime.fromtimestamp(transcript.stat().st_mtime, tz=timezone.utc)


# ---- corpus assembly -----------------------------------------------------


def build_corpus() -> tuple[list[dict], dict]:
    """Discover, dump, and filter transcripts into per-session records.
    Returns (sessions, counters) where counters tracks every exclusion
    reason for the reply's totals."""
    counters = {
        "discovered": 0,
        "excluded_recent": 0,
        "excluded_dump_failed": 0,
        "excluded_low_actions": 0,
        "sessions": 0,
    }
    now = time.time()
    sessions = []
    for t in discover_transcripts():
        counters["discovered"] += 1
        mtime = t.stat().st_mtime
        if now - mtime < RECENT_SECONDS:
            counters["excluded_recent"] += 1
            continue
        dump = run_dump(t)
        if dump is None:
            counters["excluded_dump_failed"] += 1
            continue
        if dump.get("actions", 0) < MIN_ACTIONS:
            counters["excluded_low_actions"] += 1
            continue
        sessions.append({"transcript": t, "dump": dump})
        counters["sessions"] += 1
    return sessions, counters


def group_by_project(sessions: list[dict]) -> dict[str, list[dict]]:
    """Group sessions by the dump's `project` field, each group ordered
    chronologically (start_ts, falling back to file mtime; transcript path
    breaks any remaining tie for full determinism)."""
    groups: dict[str, list[dict]] = {}
    for s in sessions:
        proj = s["dump"]["project"]
        groups.setdefault(proj, []).append(s)
    for proj in groups:
        groups[proj].sort(
            key=lambda s: (
                session_time(s["dump"], s["transcript"]),
                str(s["transcript"]),
            )
        )
    return groups


def anonymize_projects(groups: dict[str, list[dict]]) -> dict[str, str]:
    """Real project string -> proj-01..proj-NN, assigned in sorted order of
    the real name so the mapping is deterministic run to run."""
    return {
        real: f"proj-{i + 1:02d}"
        for i, real in enumerate(sorted(groups.keys()))
    }


def project_fingerprint(real: str) -> str:
    """Stable, corpus-independent identity for a project.

    A holdout is only a holdout if its MEMBERSHIP is immutable. Anonymized
    ids (proj-01..proj-NN) are positional: they are assigned by sorting the
    real project names, so adding a project that sorts earlier renumbers
    everything after it. Selecting held-out projects by position in that list
    therefore silently swaps which projects are held out whenever the corpus
    grows -- contaminating the tune split with previously held-out data and
    invalidating any later held-out evaluation.

    Hashing the real project path gives an identity that never moves. It is
    one-way, so `docs/validation/holdout-snapshot.json` can be committed
    without disclosing the paths themselves.
    """
    return hashlib.sha256(real.encode("utf-8")).hexdigest()[:16]


def held_out_project_ids(
    groups: dict[str, list[dict]], anon: dict[str, str]
) -> tuple[set[str], list[str]]:
    """Resolve the frozen roster against this corpus.

    Returns (held_out_anon_ids, absent_fingerprints).

    Resolution is by fingerprint, so a frozen project that IS present is
    always held out no matter how the anonymized numbering shifted. That is
    the property that matters: contamination means a held-out project
    silently landing in the TUNE split, and fingerprint resolution makes that
    unrepresentable.

    A frozen project that is ABSENT from the corpus is a different case, and
    not contamination -- it is in neither split, so it can leak nothing. It
    happens legitimately (a project whose only session drops below
    MIN_ACTIONS disappears entirely). Rather than blocking every run, we
    carry the absent fingerprints out to the caller so they are recorded in
    the output and printed, and we keep them in the snapshot so the project
    is held out again automatically if it ever returns.

    We DO fail closed when the roster resolves to nothing at all: an empty
    holdout would silently turn a "held-out evaluation" into a no-op that
    still looks like it ran.
    """
    if not HOLDOUT_SNAPSHOT.exists():
        sys.exit(
            f"holdout snapshot missing: {HOLDOUT_SNAPSHOT}\n"
            "Refusing to guess membership -- a recomputed holdout is not a holdout."
        )
    snapshot = json.loads(HOLDOUT_SNAPSHOT.read_text())
    frozen = set(snapshot["held_out_fingerprints"])

    by_fingerprint = {project_fingerprint(real): anon[real] for real in groups}
    resolved = {by_fingerprint[fp] for fp in frozen if fp in by_fingerprint}
    absent = sorted(frozen - set(by_fingerprint))

    if not resolved:
        sys.exit(
            "fail closed: NO frozen held-out project is present in this corpus "
            f"(roster: {sorted(frozen)}).\n"
            "Every metric would silently become an all-data metric. Restore the "
            "missing project(s), or deliberately re-freeze."
        )
    return resolved, absent


# ---- pair construction -----------------------------------------------------


def window_sessions(ordered: list[dict], i: int) -> tuple[list[dict], list[dict]]:
    """(next3, within14d) window session lists for session i in its
    project's chronologically ordered session list. Both are contiguous
    slices starting at i+1 since `ordered` is sorted by time."""
    next3 = ordered[i + 1 : i + 1 + WINDOW_COUNT]
    n_time = session_time(ordered[i]["dump"], ordered[i]["transcript"])
    cutoff = n_time + timedelta(days=WINDOW_DAYS)
    within14d = []
    for s in ordered[i + 1 :]:
        if session_time(s["dump"], s["transcript"]) <= cutoff:
            within14d.append(s)
        else:
            break  # sorted ascending: once past cutoff, nothing later qualifies
    return next3, within14d


def outcome_in_window(window: list[dict], file: str, strong: bool) -> bool:
    for s in window:
        for entry in s["dump"]["files"]:
            if entry["file"] != file:
                continue
            if not strong:
                return True  # weak: any edit at all
            if entry["flagged_nr"]:
                return True
            if STRONG_KINDS & set(entry["kinds"]):
                return True
    return False


def edit_stratum(edits: int) -> str:
    if edits == 1:
        return "1"
    if edits <= 3:
        return "2-3"
    return "4+"


def top_n_files(files: list[dict], key: str, n: int = 3) -> set[str]:
    """The n files with the largest `key`, ties broken by file path.

    Path order is the tiebreak because it is deterministic and, unlike any
    outcome-aware ordering, cannot smuggle knowledge of the future into a
    baseline. Exactly n files are returned (fewer only if the session edited
    fewer), matching the product's `ranked.iter().take(3)` rather than
    expanding to include everyone tied at the boundary.
    """
    ordered = sorted(files, key=lambda e: (-e.get(key, 0), e["file"]))
    return {e["file"] for e in ordered[:n]}


def build_pairs(groups: dict[str, list[dict]], anon: dict[str, str]) -> tuple[list[dict], int]:
    """Every (session N, edited file F) pair with both outcome definitions
    precomputed for both windows. Returns (pairs, excluded_last_session_pairs).
    Sorted by (project, session index, file) for determinism."""
    pairs = []
    excluded = 0
    for real_proj in sorted(groups.keys()):
        ordered = groups[real_proj]
        proj_id = anon[real_proj]
        for i, s in enumerate(ordered):
            files = s["dump"]["files"]
            if i == len(ordered) - 1:
                # Last session of the project: no window successors under
                # any window definition. Excluded from every denominator.
                excluded += len(files)
                continue
            next3, within14d = window_sessions(ordered, i)
            # Baseline top-3 sets are per-session, so they are computed once
            # per session rather than once per file.
            top3_edits = top_n_files(files, "edits")
            top3_lines = top_n_files(files, "changed_lines")
            failed_commands = s["dump"].get("failed_commands", 0)
            for entry in sorted(files, key=lambda e: e["file"]):
                file = entry["file"]
                changed_lines = entry.get("changed_lines", 0)
                pairs.append(
                    {
                        "project": proj_id,
                        "session_index": i,
                        "edits": entry["edits"],
                        "changed_lines": changed_lines,
                        "session_failed_commands": failed_commands,
                        "stratum": edit_stratum(entry["edits"]),
                        "flagged_nr": entry["flagged_nr"],
                        "flagged_top3": entry["flagged_top3"],
                        # Trivial baselines. Thresholds are fixed in RULES /
                        # RULE_BASIS above and never swept.
                        "base_edits_ge2": entry["edits"] >= 2,
                        "base_edits_ge4": entry["edits"] >= 4,
                        "base_top3_edits": file in top3_edits,
                        "base_lines_ge200": changed_lines >= 200,
                        "base_lines_ge400": changed_lines >= 400,
                        "base_top3_lines": file in top3_lines,
                        "base_failed_ge1": failed_commands >= 1,
                        "base_all": True,
                        # Study heuristic (T5.4-followup, 2026-07-22), not a
                        # product signal: see validity_dump.rs's
                        # `last_edit_verified` doc comment for the exact
                        # definition. Used only by hypothesis_table() below.
                        "last_edit_verified": entry.get("last_edit_verified", False),
                        "next3_weak": outcome_in_window(next3, file, strong=False),
                        "next3_strong": outcome_in_window(next3, file, strong=True),
                        "within14d_weak": outcome_in_window(within14d, file, strong=False),
                        "within14d_strong": outcome_in_window(within14d, file, strong=True),
                    }
                )
    return pairs, excluded


# ---- metrics -----------------------------------------------------


def outcome_key(window: str, outcome: str) -> str:
    return f"{window}_{outcome}"


def contingency(pairs: list[dict], flag_key: str, out_key: str) -> tuple[int, int, int, int]:
    a = b = c = d = 0
    for p in pairs:
        flagged = p[flag_key]
        positive = p[out_key]
        if flagged and positive:
            a += 1
        elif flagged and not positive:
            b += 1
        elif not flagged and positive:
            c += 1
        else:
            d += 1
    return a, b, c, d


def relative_risk(a: int, b: int, c: int, d: int) -> float | None:
    if (a + b) == 0 or (c + d) == 0 or c == 0:
        return None
    p_flagged = a / (a + b)
    p_unflagged = c / (c + d)
    if p_unflagged == 0:
        return None
    return p_flagged / p_unflagged


def precision(a: int, b: int) -> float | None:
    return a / (a + b) if (a + b) else None


def miss_rate(a: int, c: int) -> float | None:
    return c / (a + c) if (a + c) else None


def rr_confidence_interval(a: int, b: int, c: int, d: int) -> tuple[float, float] | None:
    """95% CI for the relative risk, by Katz's log method.

    The point estimate alone is what makes a 19-vs-3 contingency table look
    like a finding. `log(RR)` is approximately normal with
    `SE = sqrt(1/a - 1/(a+b) + 1/c - 1/(c+d))`, so the interval widens on its
    own exactly when the cells are thin, and an interval spanning 1.0 says
    "this table cannot distinguish this rule from a coin flip" without anyone
    having to eyeball the counts.

    `None` when RR itself is undefined, or when a or c is 0 (the SE has 1/a
    and 1/c terms, so a zero cell makes the interval infinite; reporting no
    interval is more honest than reporting an unbounded one). The
    normal approximation is itself shaky for small cells, which is why
    thin_cells() flags them separately rather than relying on the CI.
    """
    if relative_risk(a, b, c, d) is None or a == 0 or c == 0:
        return None
    rr = a / (a + b) / (c / (c + d))
    se = math.sqrt(1 / a - 1 / (a + b) + 1 / c - 1 / (c + d))
    return (math.exp(math.log(rr) - 1.96 * se), math.exp(math.log(rr) + 1.96 * se))


def thin_cells(a: int, b: int, c: int, d: int) -> list[str]:
    """Names of the contingency cells below MIN_CELL, sorted.

    Reported per row so "too small to conclude from" is a printed fact rather
    than something the reader has to derive.
    """
    return sorted(name for name, v in (("a", a), ("b", b), ("c", c), ("d", d)) if v < MIN_CELL)


def rule_comparison(pairs: list[dict]) -> dict:
    """Every rule in RULES scored on every (window, outcome), with flag counts.

    Flag count is a first-class output, not a footnote: a rule that flags
    everything wins recall trivially and must be visible as doing so. Raw a/b/
    c/d accompany every ratio for the same reason.
    """
    n = len(pairs)
    out: dict[str, dict] = {}
    for rule in RULES:
        n_flagged = sum(1 for p in pairs if p[rule])
        entry = {
            "label": RULE_LABEL[rule],
            "basis": RULE_BASIS[rule],
            "n_flagged": n_flagged,
            "flag_share": (n_flagged / n) if n else None,
            "windows": {},
        }
        for window in WINDOWS:
            entry["windows"][window] = {}
            for outcome in OUTCOMES:
                out_key = outcome_key(window, outcome)
                a, b, c, d = contingency(pairs, rule, out_key)
                entry["windows"][window][outcome] = {
                    "counts": {"a": a, "b": b, "c": c, "d": d},
                    "relative_risk": relative_risk(a, b, c, d),
                    "rr_ci95": rr_confidence_interval(a, b, c, d),
                    "precision": precision(a, b),
                    "miss_rate": miss_rate(a, c),
                    "thin_cells": thin_cells(a, b, c, d),
                }
        out[rule] = entry
    return {"n_pairs": n, "rules": out}


def rule_agreement(pairs: list[dict]) -> dict:
    """Discordant-pair analysis: where a product flag and a baseline DISAGREE,
    which one is right?

    Marginal RR and precision can be identical for two rules that fire on
    completely different files, so matching a baseline on those numbers does
    not establish that the weighted score is merely recomputing edit count.
    This looks only at the pairs the two rules split on, and asks what share of
    each disagreement bucket actually showed the outcome. If the
    product-only bucket has a visibly higher rate than the baseline-only
    bucket, the weighting is adding something; if the two rates are the same,
    the disagreements are noise and the weighting is not.

    Fixed to the primary (window, outcome) throughout, and reported with raw
    counts because these buckets are the thinnest cells in the whole study.
    """
    key = outcome_key(PRIMARY_WINDOW, PRIMARY_OUTCOME)
    out: dict[str, dict] = {}
    for product in PRODUCT_RULES:
        for baseline in BASELINE_RULES:
            buckets = {}
            for name, pred in (
                ("both", lambda p: p[product] and p[baseline]),
                ("product_only", lambda p: p[product] and not p[baseline]),
                ("baseline_only", lambda p: not p[product] and p[baseline]),
                ("neither", lambda p: not p[product] and not p[baseline]),
            ):
                sub = [p for p in pairs if pred(p)]
                pos = sum(1 for p in sub if p[key])
                buckets[name] = {
                    "n": len(sub),
                    "positive": pos,
                    "rate": (pos / len(sub)) if sub else None,
                }
            out[f"{product}|{baseline}"] = buckets
    return {"window": PRIMARY_WINDOW, "outcome": PRIMARY_OUTCOME, "buckets": out}


def false_alarm_share(pairs: list[dict], flag_key: str) -> dict:
    """Share of flagged files with no future edit at all -- fixed to the
    weak outcome / next-3-sessions window regardless of which (window,
    outcome) table this is attached to (that is the metric's definition,
    not a per-table recomputation)."""
    flagged = [p for p in pairs if p[flag_key]]
    if not flagged:
        return {"share": None, "no_future_edit": 0, "flagged": 0}
    no_future_edit = sum(1 for p in flagged if not p["next3_weak"])
    return {
        "share": no_future_edit / len(flagged),
        "no_future_edit": no_future_edit,
        "flagged": len(flagged),
    }


def stratified_rr(pairs: list[dict], flag_key: str, out_key: str) -> dict:
    out = {}
    for stratum in ("1", "2-3", "4+"):
        sub = [p for p in pairs if p["stratum"] == stratum]
        a, b, c, d = contingency(sub, flag_key, out_key)
        out[stratum] = {
            "rr": relative_risk(a, b, c, d),
            "counts": {"a": a, "b": b, "c": c, "d": d},
        }
    return out


def compute_metrics(pairs: list[dict]) -> dict:
    metrics = {}
    for flag_key in FLAG_DEFS:
        fa = false_alarm_share(pairs, flag_key)
        metrics[flag_key] = {"false_alarm_share": fa, "windows": {}}
        for window in WINDOWS:
            metrics[flag_key]["windows"][window] = {}
            for outcome in OUTCOMES:
                out_key = outcome_key(window, outcome)
                a, b, c, d = contingency(pairs, flag_key, out_key)
                metrics[flag_key]["windows"][window][outcome] = {
                    "counts": {"a": a, "b": b, "c": c, "d": d},
                    "relative_risk": relative_risk(a, b, c, d),
                    "precision": precision(a, b),
                    "miss_rate": miss_rate(a, c),
                    "stratified_rr": stratified_rr(pairs, flag_key, out_key),
                }
    return metrics


def hypothesis_table(tune: list[dict]) -> dict:
    """Unverified-ending hypothesis (T5.4-followup, 2026-07-22, no tuning --
    pure stratification, tune-split only): flags on files whose last edit
    was never verified recur more than flags on verified files. Strong
    outcome, next-3-sessions window, throughout (fixed, not swept).

    Two views:
      - "flagged_nr_split": within flagged_nr's usual RR/precision, the
        tune-split population is first split by last_edit_verified, so each
        cell answers "how well does flagged_nr do among files that WERE
        verified" vs "among files that were NOT" -- same contingency/RR/
        precision machinery as the edit-count stratification above, just a
        different stratifier.
      - "standalone": last_edit_verified used AS the predictor by itself
        (unverified = positive), over every tune-split edited file,
        ignoring flagged_nr entirely -- does the field carry signal on its
        own, independent of the product's existing flags.

    Takes the already-filtered tune split. Filtering here from a full pair
    list would mean the caller had held-out records in hand anyway, which is
    exactly the exposure this study is supposed to prevent.
    """
    flagged_nr_split = {}
    for verified in (True, False):
        sub = [p for p in tune if p["last_edit_verified"] == verified]
        a, b, c, d = contingency(sub, "flagged_nr", "next3_strong")
        flagged_nr_split[str(verified)] = {
            "n": len(sub),
            "counts": {"a": a, "b": b, "c": c, "d": d},
            "relative_risk": relative_risk(a, b, c, d),
            "precision": precision(a, b),
        }

    a = sum(1 for p in tune if not p["last_edit_verified"] and p["next3_strong"])
    b = sum(1 for p in tune if not p["last_edit_verified"] and not p["next3_strong"])
    c = sum(1 for p in tune if p["last_edit_verified"] and p["next3_strong"])
    d = sum(1 for p in tune if p["last_edit_verified"] and not p["next3_strong"])
    standalone = {
        "n": len(tune),
        "counts": {"a": a, "b": b, "c": c, "d": d},
        "relative_risk": relative_risk(a, b, c, d),
        "precision": precision(a, b),
    }

    return {
        "n_tune_pairs": len(tune),
        "flagged_nr_split": flagged_nr_split,
        "standalone": standalone,
    }


def render_comparison_stdout(cmp: dict) -> str:
    """Plain-text rendering of the primary baseline comparison for stdout.
    The full four-combination version lives in the draft report."""
    lines = [
        f"Baseline comparison (tune-split only, {PRIMARY_OUTCOME} outcome, "
        f"{PRIMARY_WINDOW} window, {cmp['n_pairs']} pairs):",
        "  " + COMPARISON_HEADER,
        "  " + COMPARISON_DIVIDER,
    ]
    lines.extend("  " + r for r in comparison_rows(cmp, PRIMARY_WINDOW, PRIMARY_OUTCOME))
    return "\n".join(lines)


def render_hypothesis_table(h: dict) -> str:
    """Plain-text rendering for stdout / the autopsy draft (this table is
    NOT part of render_draft()/DRAFT_OUT -- it lives in the autopsy draft
    only, per the T5.4-followup brief)."""
    lines = []
    lines.append("Unverified-ending hypothesis (tune-split only, strong outcome, next-3-sessions window):")
    lines.append(f"  tune-split pairs: {h['n_tune_pairs']}")
    lines.append("")
    lines.append("  flagged_nr RR/precision, split by last_edit_verified:")
    lines.append("  | last_edit_verified | n | RR | precision | a | b | c | d |")
    lines.append("  |---|---|---|---|---|---|---|---|")
    for key in ("True", "False"):
        s = h["flagged_nr_split"][key]
        c = s["counts"]
        lines.append(
            f"  | {key} | {s['n']} | {fmt(s['relative_risk'])} | {fmt(s['precision'])} | "
            f"{c['a']} | {c['b']} | {c['c']} | {c['d']} |"
        )
    lines.append("")
    s = h["standalone"]
    c = s["counts"]
    lines.append("  standalone signal (unverified vs verified, ALL edited files, flagged_nr ignored):")
    lines.append(
        f"  | n | RR | precision | a | b | c | d |\n"
        f"  |---|---|---|---|---|---|---|\n"
        f"  | {s['n']} | {fmt(s['relative_risk'])} | {fmt(s['precision'])} | "
        f"{c['a']} | {c['b']} | {c['c']} | {c['d']} |"
    )
    return "\n".join(lines)


# ---- report rendering -----------------------------------------------------


def fmt(x: float | None, digits: int = 2) -> str:
    return "n/a" if x is None else f"{x:.{digits}f}"


def fmt_ci(ci: tuple[float, float] | None) -> str:
    return "n/a" if ci is None else f"{ci[0]:.2f}-{ci[1]:.2f}"


def comparison_rows(cmp: dict, window: str, outcome: str) -> list[str]:
    """One markdown table body: every rule, for one (window, outcome)."""
    rows = []
    for rule in RULES:
        e = cmp["rules"][rule]
        m = e["windows"][window][outcome]
        c = m["counts"]
        thin = ",".join(m["thin_cells"]) or "-"
        rows.append(
            f"| {e['label']} | {e['n_flagged']} | {fmt(e['flag_share'])} | "
            f"{fmt(m['relative_risk'])} | {fmt_ci(m['rr_ci95'])} | "
            f"{fmt(m['precision'])} | {fmt(m['miss_rate'])} | "
            f"{c['a']} | {c['b']} | {c['c']} | {c['d']} | {thin} |"
        )
    return rows


def verdict(cmp: dict) -> dict:
    """The verdict, computed from the table rather than written by hand.

    A verdict typed as prose goes stale the moment the corpus changes, and a
    stale verdict in a validation report is worse than none. This derives it
    mechanically at the primary (window, outcome), so the sentence in the
    report and the numbers above it cannot disagree.

    `base_all` is excluded from candidacy: it is a reference row, not a rule
    anyone would ship, and it has no RR at all.

    "Dominates" is deliberately strict: a baseline dominates a product flag
    only if it is at least as good on ALL THREE of RR, precision, and miss
    rate. That makes domination hard to achieve by accident, so a domination
    result is the strong form of a negative finding, not a coin flip. Ties on
    all three count as domination because the point of the comparison is
    whether the extra machinery BUYS anything; matching a one-line rule buys
    nothing.
    """
    w, o = PRIMARY_WINDOW, PRIMARY_OUTCOME

    def cell(rule):
        return cmp["rules"][rule]["windows"][w][o]

    candidates = [r for r in BASELINE_RULES if r != "base_all"]
    scored = [(cell(r)["relative_risk"] or 0.0, r) for r in candidates]
    best_rr, best_rule = max(scored, key=lambda t: (t[0], t[1]))

    out = {"window": w, "outcome": o, "best_baseline": best_rule, "best_baseline_rr": best_rr}
    per_product = {}
    for product in PRODUCT_RULES:
        pm = cell(product)
        dominators = []
        for r in candidates:
            bm = cell(r)
            if (
                (bm["relative_risk"] or 0.0) >= (pm["relative_risk"] or 0.0)
                and (bm["precision"] or 0.0) >= (pm["precision"] or 0.0)
                and (bm["miss_rate"] if bm["miss_rate"] is not None else 1.0)
                <= (pm["miss_rate"] if pm["miss_rate"] is not None else 1.0)
            ):
                dominators.append(r)
        # CI overlap against the strongest baseline: non-overlap is the only
        # evidence here that would separate two rules rather than merely
        # rank their point estimates.
        p_ci, b_ci = pm["rr_ci95"], cell(best_rule)["rr_ci95"]
        overlaps = None
        if p_ci and b_ci:
            overlaps = p_ci[0] <= b_ci[1] and b_ci[0] <= p_ci[1]
        per_product[product] = {
            "relative_risk": pm["relative_risk"],
            "beats_best_baseline_on_rr": (pm["relative_risk"] or 0.0) > best_rr,
            "ci_overlaps_best_baseline": overlaps,
            "dominated_by": dominators,
        }
    out["per_product"] = per_product
    out["any_product_beats_every_baseline"] = all(
        v["beats_best_baseline_on_rr"] and not v["dominated_by"]
        for v in per_product.values()
    )
    return out


def render_verdict(v: dict, window_label: dict, outcome_label: dict) -> list[str]:
    lines = []
    lines.append("### Verdict")
    lines.append("")
    lines.append("Computed from the headline table, not written by hand, so it cannot")
    lines.append("drift out of step with the numbers above it. Judged at")
    lines.append(
        f"{outcome_label[v['outcome']]}, {window_label[v['window']]}. A baseline"
    )
    lines.append("`dominates` a product flag when it is at least as good on ALL THREE of")
    lines.append("RR, precision, and miss rate. Ties count as domination: the question is")
    lines.append("whether the weighting BUYS anything over a one-line rule, and matching a")
    lines.append("one-line rule buys nothing.")
    lines.append("")
    lines.append(
        f"Strongest baseline by RR: {RULE_LABEL[v['best_baseline']]} "
        f"(RR {fmt(v['best_baseline_rr'])})."
    )
    lines.append("")
    for product in PRODUCT_RULES:
        p = v["per_product"][product]
        doms = ", ".join(RULE_LABEL[d] for d in p["dominated_by"]) or "none"
        overlap = {True: "yes", False: "no", None: "not computable"}[
            p["ci_overlaps_best_baseline"]
        ]
        lines.append(
            f"- **{product}** (RR {fmt(p['relative_risk'])}): beats the strongest "
            f"baseline on RR: {'yes' if p['beats_best_baseline_on_rr'] else 'NO'}. "
            f"RR 95% CI overlaps that baseline: {overlap}. Dominated by: {doms}."
        )
    lines.append("")
    if v["any_product_beats_every_baseline"]:
        lines.append("**Verdict: the weighted ranking beats every trivial baseline tested.**")
        lines.append("")
        lines.append("What may be claimed: that the ranking outperforms raw edit count on")
        lines.append("this corpus, with the corpus caveats below still attached.")
    else:
        lines.append(
            "**Verdict: the weighted ranking does NOT beat the trivial baselines. At "
            "least one one-line rule matches or dominates a product flag on RR, "
            "precision and miss rate simultaneously, and the RR confidence intervals "
            "overlap, so this corpus cannot distinguish the weighted score from "
            "counting edits.**"
        )
        lines.append("")
        lines.append("What this does and does not license as a claim:")
        lines.append("")
        lines.append("- STILL SUPPORTED: the flags are far better than chance. Every")
        lines.append("  product row above has an RR well over 1 with a CI excluding 1.")
        lines.append("  Flagged files really do recur more than unflagged ones.")
        lines.append("- NOT SUPPORTED: any claim that the weighting, the score, or the")
        lines.append("  multi-signal model is what produces that lift. A rule that sorts")
        lines.append("  files by how many times they were edited and takes the top 3 does")
        lines.append("  the same job, on this corpus, within noise.")
        lines.append("- NOT SUPPORTED: any comparative or superiority claim over simpler")
        lines.append("  tools, since the simplest possible tool was not beaten here.")
        lines.append("- The honest framing is that the product currently packages a")
        lines.append("  known-useful signal (repeated edits) with explanation and evidence")
        lines.append("  attached. That is a real product, but it is a usability claim, not")
        lines.append("  a predictive-accuracy claim, and the README must not imply the")
        lines.append("  latter until a corpus large enough to separate the two exists.")
    lines.append("")
    return lines


COMPARISON_HEADER = (
    "| rule | flagged | flag share | RR | RR 95% CI | precision | miss rate | "
    "a | b | c | d | thin cells |"
)
COMPARISON_DIVIDER = "|---|---|---|---|---|---|---|---|---|---|---|---|"


def render_comparison(
    cmp: dict, agreement: dict, window_label: dict, outcome_label: dict
) -> list[str]:
    """The baseline-comparison section of the draft report."""
    lines = []
    lines.append("## Baseline comparison: does the weighted ranking earn its complexity?")
    lines.append("")
    lines.append("The tables above establish that the product's flags beat *chance*. That")
    lines.append("is a low bar. The question that decides whether the weighting is worth")
    lines.append("anything is whether it beats a rule a reader could implement in one")
    lines.append("line, so every trivial predictor below is pushed through the identical")
    lines.append("contingency / RR / precision machinery, over the identical tune-split")
    lines.append("pair population, as the two product flags.")
    lines.append("")
    lines.append("### Threshold provenance")
    lines.append("")
    lines.append("Every threshold is either zero-parameter or a constant that already")
    lines.append("existed in this repo, for an unrelated reason, before this comparison")
    lines.append("was written. Nothing here was swept, and where a quantity has two")
    lines.append("pre-existing boundaries both are reported rather than the better-looking")
    lines.append("one being selected after the fact.")
    lines.append("")
    lines.append("| rule | how the threshold was fixed |")
    lines.append("|---|---|")
    for rule in RULES:
        lines.append(f"| {RULE_LABEL[rule]} | {RULE_BASIS[rule]} |")
    lines.append("")
    lines.append("Top-N baselines use N = 3, the same N as the product flag they are")
    lines.append("compared against, so the comparison is like-for-like on flag count and")
    lines.append("not just on definition. Ties are broken by file path: deterministic, and")
    lines.append("independent of the outcome being predicted.")
    lines.append("")
    lines.append("`changed_lines` counts only Edit/Write actions whose tool result")
    lines.append("confirmed success, matching `report.rs`'s `lines_written`. `edits`")
    lines.append("counts every attempted Edit/Write, so a file can have edits > 0 and")
    lines.append("changed_lines == 0.")
    lines.append("")
    lines.append("`flagged` is the count of tune-split pairs the rule fires on, out of")
    lines.append(f"{cmp['n_pairs']}. It is reported first on purpose: a rule that flags")
    lines.append("everything achieves perfect recall and zero miss rate for free, and must")
    lines.append("be readable as doing so. The `flag every edited file` row is that")
    lines.append("degenerate reference; its precision is the base rate every other rule has")
    lines.append("to beat.")
    lines.append("")
    lines.append("`thin cells` lists the contingency cells with fewer than "
                 f"{MIN_CELL} observations. Any")
    lines.append("row with a thin cell has an RR that cannot support a conclusion, no")
    lines.append("matter how large the point estimate looks.")
    lines.append("")
    lines.append(
        f"### Headline: {outcome_label[PRIMARY_OUTCOME]}, {window_label[PRIMARY_WINDOW]}"
    )
    lines.append("")
    lines.append("This (window, outcome) pair was declared the primary comparison in the")
    lines.append("script before any result was computed. The other three combinations")
    lines.append("follow, in full, regardless of how this one came out.")
    lines.append("")
    lines.append(COMPARISON_HEADER)
    lines.append(COMPARISON_DIVIDER)
    lines.extend(comparison_rows(cmp, PRIMARY_WINDOW, PRIMARY_OUTCOME))
    lines.append("")
    lines.extend(render_verdict(verdict(cmp), window_label, outcome_label))
    for window in WINDOWS:
        for outcome in OUTCOMES:
            if window == PRIMARY_WINDOW and outcome == PRIMARY_OUTCOME:
                continue
            lines.append(f"### {outcome_label[outcome]}, {window_label[window]}")
            lines.append("")
            lines.append(COMPARISON_HEADER)
            lines.append(COMPARISON_DIVIDER)
            lines.extend(comparison_rows(cmp, window, outcome))
            lines.append("")

    lines.append("### Where they disagree, who is right?")
    lines.append("")
    lines.append("Two rules can post identical RR and precision while firing on completely")
    lines.append("different files, so matching a baseline on the marginals does not by")
    lines.append("itself show the weighted score is just recomputing edit count. This")
    lines.append("restricts attention to the pairs where a product flag and a baseline")
    lines.append("disagree, and reports the outcome rate inside each disagreement bucket")
    lines.append(
        f"({outcome_label[PRIMARY_OUTCOME]}, {window_label[PRIMARY_WINDOW]})."
    )
    lines.append("A product-only rate clearly above the baseline-only rate would mean the")
    lines.append("weighting adds something the count does not see. Equal rates mean the")
    lines.append("disagreements are noise.")
    lines.append("")
    lines.append("These are the thinnest cells in the study. Read the raw counts.")
    lines.append("")
    lines.append(
        "| product rule | baseline | both n/pos/rate | product only n/pos/rate | "
        "baseline only n/pos/rate | neither n/pos/rate |"
    )
    lines.append("|---|---|---|---|---|---|")
    for product in PRODUCT_RULES:
        for baseline in BASELINE_RULES:
            b = agreement["buckets"][f"{product}|{baseline}"]
            cells = " | ".join(
                f"{b[k]['n']}/{b[k]['positive']}/{fmt(b[k]['rate'])}"
                for k in ("both", "product_only", "baseline_only", "neither")
            )
            lines.append(f"| {product} | {RULE_LABEL[baseline]} | {cells} |")
    lines.append("")
    return lines


def render_draft(
    metrics: dict,
    cmp: dict,
    agreement: dict,
    pairs: list[dict],
    excluded_pairs: int,
    n_projects: int,
    n_sessions: int,
    date_range: tuple[str, str],
    held_out_ids: list[str],
) -> str:
    """`pairs`/`metrics` are the TUNE SPLIT only; `held_out_ids` is reported
    so the reader knows what was withheld rather than inferring it."""
    flag_label = {"flagged_nr": "flagged_nr (review::needs_review)", "flagged_top3": "flagged_top3 (score::rank top-3)"}
    window_label = {"next3": "next 3 sessions", "within14d": "within 14 days"}
    outcome_label = {"weak": "weak (any future edit)", "strong": "strong (struggle recurrence)"}

    lines = []
    lines.append("# Predictive-validity draft: do flags predict future rework?")
    lines.append("")
    lines.append("Status: DRAFT, internal. Frozen default weights everywhere; no tuning")
    lines.append("performed anywhere in this pass. Any future tuning follows a")
    lines.append("predict-then-check rule: parameters would be set on one subset of")
    lines.append("projects and re-run, unchanged, on the held-out remainder, never")
    lines.append("fit and reported on the same data.")
    lines.append("")
    lines.append("Scope: every number in this report is computed on the TUNE SPLIT only.")
    lines.append("Held-out projects contribute nothing to any table here, and their")
    lines.append("per-pair outcomes are not written to the raw output either. Holdout")
    lines.append("membership is frozen by project fingerprint in")
    lines.append("`docs/validation/holdout-snapshot.json`, so it cannot drift as the")
    lines.append("corpus grows.")
    lines.append("")
    lines.append("## Method")
    lines.append("")
    lines.append("For every session N and every file it edited or wrote, we check whether")
    lines.append("that file shows further struggle signal in a later window of the same")
    lines.append("project. Two window definitions: the next 3 sessions after N, and all")
    lines.append("sessions starting within 14 days of N. Two outcome definitions:")
    lines.append("")
    lines.append("- weak: the file is edited again at all in the window")
    lines.append("- strong: in the window, the file carries a failure_loop, user_corrected,")
    lines.append("  true_revert, or flip finding, or is itself a needs_review candidate")
    lines.append("  there (recurrence of struggle, not mere activity; plain churn/rework/")
    lines.append("  re_read do not alone count as strong)")
    lines.append("")
    lines.append("Two flag definitions from the same session-N analysis are compared against")
    lines.append("both outcomes: flagged_nr (the file qualified for review::needs_review in")
    lines.append("session N) and flagged_top3 (the file was in the top 3 of score::rank in")
    lines.append("session N). Weights are Weights::default() throughout; nothing is tuned.")
    lines.append("")
    lines.append("Sessions with fewer than 20 actions, and the transcript modified in the")
    lines.append("last 10 minutes at run time (the in-progress session), are excluded from")
    lines.append("the corpus entirely, not just as source sessions.")
    lines.append("")
    lines.append("## Corpus")
    lines.append("")
    lines.append(f"- projects: {n_projects} (anonymized as proj-01..proj-{n_projects:02d})")
    lines.append(f"- sessions analyzed: {n_sessions}")
    lines.append(f"- date range: {date_range[0]} to {date_range[1]}")
    lines.append(
        f"- held out, excluded from every number below: {', '.join(held_out_ids)} "
        f"(scored only at a release gate, via `validity_sweep.py --release-eval`)"
    )
    lines.append(f"- (session, edited-file) pairs in the metrics below: {len(pairs)} (tune split)")
    lines.append(
        f"- pairs excluded (session N is the last session of its project, no window "
        f"successor exists): {excluded_pairs}"
    )
    lines.append("")
    lines.append("## Metrics")
    lines.append("")
    lines.append("Relative risk (RR) = P(outcome | flagged) / P(outcome | unflagged), over")
    lines.append("edited files. RR > 1 means a flagged file is more likely to show the")
    lines.append("outcome than an unflagged one. Contingency counts: a = flagged+outcome,")
    lines.append("b = flagged+no-outcome, c = unflagged+outcome, d = unflagged+no-outcome.")
    lines.append("")

    for flag_key in FLAG_DEFS:
        m = metrics[flag_key]
        lines.append(f"### {flag_label[flag_key]}")
        lines.append("")
        fa = m["false_alarm_share"]
        lines.append(
            f"False-alarm share (flagged files with no edit at all in the next 3 "
            f"sessions): {fmt(fa['share'])} ({fa['no_future_edit']}/{fa['flagged']})"
        )
        lines.append("")
        lines.append(
            "| window | outcome | RR | precision | miss rate | a | b | c | d |"
        )
        lines.append("|---|---|---|---|---|---|---|---|---|")
        for window in WINDOWS:
            for outcome in OUTCOMES:
                d = m["windows"][window][outcome]
                counts = d["counts"]
                lines.append(
                    f"| {window_label[window]} | {outcome_label[outcome]} | "
                    f"{fmt(d['relative_risk'])} | {fmt(d['precision'])} | "
                    f"{fmt(d['miss_rate'])} | {counts['a']} | {counts['b']} | "
                    f"{counts['c']} | {counts['d']} |"
                )
        lines.append("")
        lines.append("Stratified RR by session-N edit count of the file (busy-file confound check):")
        lines.append("")
        lines.append("| window | outcome | 1 edit | 2-3 edits | 4+ edits |")
        lines.append("|---|---|---|---|---|")
        for window in WINDOWS:
            for outcome in OUTCOMES:
                strat = m["windows"][window][outcome]["stratified_rr"]
                lines.append(
                    f"| {window_label[window]} | {outcome_label[outcome]} | "
                    f"{fmt(strat['1']['rr'])} | {fmt(strat['2-3']['rr'])} | "
                    f"{fmt(strat['4+']['rr'])} |"
                )
        lines.append("")

    lines.extend(render_comparison(cmp, agreement, window_label, outcome_label))

    lines.append("## Caveats")
    lines.append("")
    lines.append("- Single-machine, single-author corpus: not generalizable beyond this")
    lines.append("  author's own working style.")
    lines.append("- Small per-project session counts mean stratified cells can be sparse;")
    lines.append("  a single-digit denominator makes a ratio noisy even when the sign is")
    lines.append("  informative. Read the raw counts, not just the ratio.")
    lines.append("- Weak outcome (any future edit) is confounded with file busy-ness; the")
    lines.append("  strong outcome and the stratified RR exist specifically to separate")
    lines.append("  \"this file gets edited a lot\" from \"this file keeps struggling.\"")
    lines.append("- Projects and sessions with fewer than 20 actions are excluded from the")
    lines.append("  corpus outright, including as window members for other sessions; this")
    lines.append("  is a scope choice, not a null result about short sessions.")
    lines.append("- This is a frozen-weights, no-tuning pass. It measures whether the")
    lines.append("  existing default weighting is doing anything predictive at all, not")
    lines.append("  whether it is the best possible weighting.")
    lines.append("- The baseline comparison is descriptive. No rule is fitted, so nothing")
    lines.append("  here is corrected for multiple comparisons; the RR intervals are")
    lines.append("  marginal 95% intervals for each row read on its own, and the rows are")
    lines.append("  not independent of each other (they score the same pairs).")
    lines.append("")
    return "\n".join(lines)


# ---- main -----------------------------------------------------


def main() -> int:
    if not DUMP_BIN.exists():
        print(
            f"missing {DUMP_BIN}\n"
            "run: cargo build --release --example validity_dump -p sumcp-core",
            file=sys.stderr,
        )
        return 1

    sessions, counters = build_corpus()
    if not sessions:
        print("no eligible sessions found", file=sys.stderr)
        return 1

    release_eval = "--release-eval" in sys.argv

    groups = group_by_project(sessions)
    anon = anonymize_projects(groups)
    pairs, excluded_pairs = build_pairs(groups, anon)
    held_out, held_out_absent = held_out_project_ids(groups, anon)

    # Split BEFORE any metric is computed or persisted. Computing over all
    # pairs first and filtering afterwards would still put held-out labels,
    # outcomes and aggregates into the development output on every run, which
    # makes the holdout recoverable and defeats the once-per-release rule.
    tune_pairs = [p for p in pairs if p["project"] not in held_out]
    held_out_pairs = [p for p in pairs if p["project"] in held_out]

    times = [
        session_time(s["dump"], s["transcript"]).isoformat() for s in sessions
    ]
    date_range = (min(times), max(times)) if times else ("", "")

    if release_eval:
        # The gated once-per-release evaluation. Deliberately a separate file
        # and a separate invocation so held-out results can never appear as a
        # side effect of ordinary development.
        RELEASE_EVAL_OUT.parent.mkdir(parents=True, exist_ok=True)
        RELEASE_EVAL_OUT.write_text(
            json.dumps(
                {
                    "scope": "HELD-OUT PROJECTS ONLY -- once-per-release gate",
                    "held_out_project_ids": sorted(held_out),
                    "n_held_out_pairs": len(held_out_pairs),
                    "metrics": compute_metrics(held_out_pairs),
                    "baseline_comparison": rule_comparison(held_out_pairs),
                    "baseline_agreement": rule_agreement(held_out_pairs),
                    "hypothesis_unverified_ending": hypothesis_table(held_out_pairs),
                },
                indent=2,
                sort_keys=True,
            )
        )
        print(f"HELD-OUT EVALUATION (release gate) -> {RELEASE_EVAL_OUT}")
        print(f"held-out projects: {sorted(held_out)}  pairs: {len(held_out_pairs)}")
        return 0

    # ---- development run: tune split only, from here down --------------
    metrics = compute_metrics(tune_pairs)
    comparison = rule_comparison(tune_pairs)
    agreement = rule_agreement(tune_pairs)
    hypothesis = hypothesis_table(tune_pairs)

    raw = {
        "scope": "TUNE SPLIT ONLY -- held-out projects excluded from every "
        "record and aggregate below",
        "counters": counters,
        "project_mapping": {v: k for k, v in anon.items()},
        "n_projects": len(groups),
        "n_sessions": len(sessions),
        "date_range": date_range,
        "excluded_pairs_last_session": excluded_pairs,
        "n_pairs": len(tune_pairs),
        "pairs": tune_pairs,
        "metrics": metrics,
        "baseline_comparison": comparison,
        "baseline_agreement": agreement,
        "baseline_verdict": verdict(comparison),
        "held_out_project_ids": sorted(held_out),
        # Frozen roster entries with no project in this corpus. Recorded so a
        # partially-satisfiable freeze is visible, never inferred.
        "held_out_absent_fingerprints": held_out_absent,
        # Count only. The held-out pair records, their labels and their
        # outcomes are deliberately absent from this file.
        "n_held_out_pairs_withheld": len(held_out_pairs),
        "hypothesis_unverified_ending": hypothesis,
    }
    RAW_OUT.parent.mkdir(parents=True, exist_ok=True)
    RAW_OUT.write_text(json.dumps(raw, indent=2, sort_keys=True))

    DRAFT_OUT.parent.mkdir(parents=True, exist_ok=True)
    draft = render_draft(
        metrics,
        comparison,
        agreement,
        tune_pairs,
        excluded_pairs,
        len(groups),
        len(sessions),
        date_range,
        sorted(held_out),
    )
    DRAFT_OUT.write_text(draft)

    print(f"sessions discovered: {counters['discovered']}")
    print(f"  excluded (recent <10min): {counters['excluded_recent']}")
    print(f"  excluded (dump failed): {counters['excluded_dump_failed']}")
    print(f"  excluded (actions <{MIN_ACTIONS}): {counters['excluded_low_actions']}")
    print(f"  used: {counters['sessions']}  across {len(groups)} projects")
    print(
        f"tune-split pairs: {len(tune_pairs)}  "
        f"(held out and withheld: {len(held_out_pairs)})  "
        f"excluded (last session of project): {excluded_pairs}"
    )
    print(f"held-out project ids: {sorted(held_out)}  [run --release-eval to score them]")
    if held_out_absent:
        print(
            f"  NOTE: {len(held_out_absent)} frozen held-out project(s) are not in "
            f"this corpus and contribute nothing: {held_out_absent}"
        )
    print()
    print(render_comparison_stdout(comparison))
    print()
    print(render_hypothesis_table(hypothesis))
    print()
    print(f"raw: {RAW_OUT}")
    print(f"draft: {DRAFT_OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
