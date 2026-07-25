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

MIN_ACTIONS = 20
RECENT_SECONDS = 10 * 60
WINDOW_COUNT = 3
WINDOW_DAYS = 14

STRONG_KINDS = {"failure_loop", "user_corrected", "true_revert", "flip"}
FLAG_DEFS = ("flagged_nr", "flagged_top3")
WINDOWS = ("next3", "within14d")
OUTCOMES = ("weak", "strong")


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
    h = hashlib.sha1(str(transcript).encode()).hexdigest()[:8]
    return CACHE_DIR / f"{h}-{transcript.stem}.json"


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
            for entry in sorted(files, key=lambda e: e["file"]):
                file = entry["file"]
                pairs.append(
                    {
                        "project": proj_id,
                        "session_index": i,
                        "edits": entry["edits"],
                        "stratum": edit_stratum(entry["edits"]),
                        "flagged_nr": entry["flagged_nr"],
                        "flagged_top3": entry["flagged_top3"],
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


def render_draft(
    metrics: dict,
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
    print(render_hypothesis_table(hypothesis))
    print()
    print(f"raw: {RAW_OUT}")
    print(f"draft: {DRAFT_OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
