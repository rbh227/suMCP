#!/usr/bin/env python3
"""Differential recount gate (dev-only, python3 stdlib only).

WHY THIS EXISTS
---------------
Every existing Rust test asserts against fixtures the same code path produced,
so a systematic scope error is invisible: the code and the fixture share the
bug. That is exactly how a 3x undercount survived 271 green tests. This is a
second, deliberately naive implementation whose only job is to disagree.

DELIBERATELY NAIVE. It must not import from, or mirror the structure of, the
Rust code. The value comes from the two implementations being written
differently. If this ever grows shared helpers with the analyzer, it stops
being a check and becomes a reimplementation.

WHAT IT CHECKS
--------------
Countable quantities only: edits, writes, reads, bash, file_ops,
files_touched, and the output / cache-read token totals, in two scopes.
First every transcript alone (`--file`, the v1 scope), then every work unit
the product discloses (`--work-unit`), recounting
exactly the member transcripts its payload names. Signal detection is out of
scope, because a second implementation of the signal logic would be a
reimplementation rather than an independent check.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SUMCP = REPO / "target" / "release" / "sumcp"

# Tool names that modify a file, and the totals key each contributes to.
EDIT_TOOLS = {"Edit"}
WRITE_TOOLS = {"Write"}

# What the committed fixtures must add up to as one unit. Asserted directly in
# --fixtures mode so a fixture edit cannot silently move the goalposts, and so
# a bug that zeroes BOTH implementations still fails instead of "agreeing".
# Tokens: 8 assistant messages, each with output_tokens 5 and no cache reads.
FIXTURE_UNIT_TOTALS = {"edits": 5, "writes": 1, "reads": 1, "bash": 1,
                       "file_ops": 6, "files_touched": 6,
                       "tokens_output": 40, "tokens_cache_read": 0}


def tool_calls(path: Path) -> list[dict]:
    """Every tool_use block in one transcript, deduped the way the parser
    rules require: by message id (streaming duplicates) and by uuid (resumed
    session replays). Last occurrence wins."""
    by_key: dict[str, dict] = {}
    order: list[str] = []
    for line_no, line in enumerate(path.read_text(errors="replace").splitlines()):
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        msg = e.get("message") or {}
        content = msg.get("content")
        if not isinstance(content, list):
            continue
        for b in content:
            if not isinstance(b, dict) or b.get("type") != "tool_use":
                continue
            # A tool_use id is unique per call; fall back to a positional key
            # so a block without one is still counted exactly once.
            key = b.get("id") or f"{path}:{line_no}:{b.get('name')}"
            if key not in by_key:
                order.append(key)
            by_key[key] = b
    return [by_key[k] for k in order]


def usage_totals(path: Path) -> tuple[int, int]:
    """(output, cache_read) token totals for one transcript, deduped the way
    the parser rule declares: one usage record per message id, last wins
    (streaming re-sends the same message id with growing usage)."""
    by_msg: dict[str, tuple[int, int]] = {}
    for line in path.read_text(errors="replace").splitlines():
        try:
            e = json.loads(line)
        except json.JSONDecodeError:
            continue
        msg = e.get("message") or {}
        mid = msg.get("id")
        usage = msg.get("usage")
        if not (isinstance(mid, str) and isinstance(usage, dict)):
            continue
        out = usage.get("output_tokens")
        cached = usage.get("cache_read_input_tokens")
        by_msg[mid] = (out if isinstance(out, int) else 0,
                       cached if isinstance(cached, int) else 0)
    return (sum(o for o, _ in by_msg.values()),
            sum(c for _, c in by_msg.values()))


def recount(paths: list[Path]) -> dict:
    """Totals over a set of transcripts (a work unit: the main transcripts
    plus every subagent transcript belonging to each)."""
    edits = writes = reads = bash = 0
    tokens_output = tokens_cache_read = 0
    touched: set[str] = set()
    for p in paths:
        for b in tool_calls(p):
            name = b.get("name")
            inp = b.get("input") or {}
            if name in EDIT_TOOLS:
                edits += 1
            elif name in WRITE_TOOLS:
                writes += 1
            elif name == "Read":
                reads += 1
            elif name == "Bash":
                bash += 1
            fp = inp.get("file_path")
            if isinstance(fp, str) and fp:
                touched.add(fp)
        out, cached = usage_totals(p)
        tokens_output += out
        tokens_cache_read += cached
    return {
        "edits": edits,
        "writes": writes,
        "reads": reads,
        "bash": bash,
        "file_ops": edits + writes,
        "files_touched": len(touched),
        "tokens_output": tokens_output,
        "tokens_cache_read": tokens_cache_read,
    }


def members_of(main: Path) -> list[Path]:
    """A main transcript plus its subagent children (both on-disk layouts)."""
    out = [main]
    stem = str(main)[: -len(".jsonl")]
    out += [Path(p) for p in sorted(glob.glob(f"{stem}/subagents/agent-*.jsonl"))]
    out += [Path(p) for p in sorted(glob.glob(f"{main.parent}/agent-*.jsonl"))]
    return out


def sumcp_payload(main: Path, work_unit: bool) -> dict | None:
    """The full JSON payload suMCP reports for the given scope."""
    flag = "--work-unit" if work_unit else "--file"
    try:
        r = subprocess.run(
            [str(SUMCP), flag, str(main), "--json"],
            capture_output=True, text=True, timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as e:
        print(f"  RUN FAILED {main.name}: {e}")
        return None
    if r.returncode != 0:
        print(f"  RUN FAILED {main.name}: exit {r.returncode}: {r.stderr[:200]}")
        return None
    try:
        return json.loads(r.stdout)
    except json.JSONDecodeError:
        print(f"  BAD JSON {main.name}")
        return None


def product_totals(payload: dict) -> dict:
    """The product's countable quantities, flattened: `totals` plus the two
    token sums from the `tokens` object, renamed to this script's keys."""
    out = dict(payload.get("totals") or {})
    toks = payload.get("tokens") or {}
    out["tokens_output"] = toks.get("output")
    out["tokens_cache_read"] = toks.get("cache_read")
    return out


def compare(label: str, mine: dict, theirs: dict) -> list[str]:
    """Every quantity where the two implementations disagree."""
    bad = []
    for k, want in mine.items():
        got = theirs.get(k)
        # file_ops in the product excludes unconfirmed operations, which this
        # naive counter cannot see, so it is compared only as an upper bound.
        if k == "file_ops":
            if got is not None and got > want:
                bad.append(f"{label}: {k} product {got} exceeds recount {want}")
            continue
        if got != want:
            bad.append(f"{label}: {k} recount {want} != product {got}")
    return bad


def unit_member_paths(main: Path, payload: dict) -> list[Path] | None:
    """Resolve the payload's disclosed unit members back to files on disk.

    An absent `work_unit` block is the schema's way of saying "one
    transcript, nothing excluded", so that case resolves to the requested
    transcript's own members rather than being an error.

    The payload shortens each session id to its first 8 characters, so each
    one is resolved by prefix against the main transcript's directory. Session
    ids are UUIDs, so a prefix collision effectively cannot happen; if one
    somehow does, this returns None and the caller reports it rather than
    guessing which file was meant.
    """
    if "work_unit" not in payload:
        return members_of(main)
    ids = (payload.get("work_unit") or {}).get("session_ids")
    if not isinstance(ids, list) or not ids:
        return None
    mains: list[Path] = []
    for sid in ids:
        hits = sorted(glob.glob(str(main.parent / f"{sid}*.jsonl")))
        hits = [h for h in hits if not Path(h).name.startswith("agent-")]
        if len(hits) != 1:
            print(f"  AMBIGUOUS id {sid}: {len(hits)} matches next to {main.name}")
            return None
        mains.append(Path(hits[0]))
    out: list[Path] = []
    for m in mains:
        out += members_of(m)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--archive", type=Path,
                    default=Path.home() / "claude-corpus-archive",
                    help="archive to check (default ~/claude-corpus-archive)")
    ap.add_argument("--fixtures", action="store_true",
                    help="check only the committed fixtures (what CI runs)")
    args = ap.parse_args()

    if not SUMCP.is_file():
        sys.exit(f"build first: cargo build --release ({SUMCP} missing)")

    if args.fixtures:
        mains = sorted((REPO / "fixtures" / "work-unit").glob("*.jsonl"))
        roots = "fixtures/work-unit"
    else:
        mains = [Path(p) for p in sorted(
            glob.glob(str(args.archive / "projects" / "*" / "*.jsonl")))
            if not os.path.basename(p).startswith("agent-")]
        roots = str(args.archive)

    if not mains:
        sys.exit(f"no transcripts found under {roots}")

    failures: list[str] = []
    checked = 0

    # Scope 1: each transcript alone, against `--file`.
    for m in mains:
        payload = sumcp_payload(m, work_unit=False)
        if payload is None:
            failures.append(f"{m.name}: could not run")
            continue
        failures += compare(m.name, recount(members_of(m)), product_totals(payload))
        checked += 1

    # Scope 2: each work unit the product discloses, against `--work-unit`.
    # Recounts exactly the member transcripts the payload names, so this
    # checks the merge arithmetic without reimplementing the grouping rule
    # (which is a declared rule with its own Rust tests, not a count).
    units_checked = 0
    seen_units: set[frozenset] = set()
    for m in mains:
        payload = sumcp_payload(m, work_unit=True)
        if payload is None:
            failures.append(f"unit({m.name}): could not run")
            continue
        members = unit_member_paths(m, payload)
        if members is None:
            failures.append(f"unit({m.name}): no resolvable work_unit disclosure")
            continue
        key = frozenset(str(p) for p in members)
        if key in seen_units:
            continue
        seen_units.add(key)
        label = f"unit({m.name}, {len(members)} file(s))"
        failures += compare(label, recount(members), product_totals(payload))
        units_checked += 1

    # The fixtures' expected totals are known by hand; hold the recount itself
    # to them, so agreement can never come from both sides reading zero.
    if args.fixtures:
        mine = recount([p for m in mains for p in members_of(m)])
        if mine != FIXTURE_UNIT_TOTALS:
            failures.append(f"fixture recount {mine} != expected {FIXTURE_UNIT_TOTALS}")

    print(f"recount: {checked} transcript(s), {units_checked} unit(s) checked under {roots}")
    if failures:
        print(f"  {len(failures)} DISAGREEMENT(S):")
        for f in failures[:25]:
            print(f"    {f}")
        return 1
    print("  exact agreement")
    return 0


if __name__ == "__main__":
    sys.exit(main())
