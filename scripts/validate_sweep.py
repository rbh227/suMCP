#!/usr/bin/env python3
"""T5.3 internal validation sweep (read-only).

Runs the release `sumcp` binary over a diverse sample of the author's OWN real
Claude Code transcripts (~/.claude/projects/**), one top-level session per row,
and reports:

  - robustness: crashes / parse failures / zero-fire on an edit-heavy session
  - the token ratio: raw-transcript tokens vs structured-debrief tokens
  - the top-3 struggle files it named (for later predict-then-check)

Token approximation matches scripts/check_payloads.py: chars / 3.5. The debrief
numerator uses the COMPACT session_overview payload (what an agent actually
consumes); the transcript denominator uses the main transcript file's bytes only
(excludes merged subagent files) — a conservative choice that UNDERstates
suMCP's advantage rather than inflating it.

Nothing is written. Subagent transcripts are discovered/merged by sumcp itself.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

CHARS_PER_TOKEN = 3.5  # keep in lockstep with check_payloads.py
PROJECTS = Path.home() / ".claude" / "projects"
SUMCP = Path(__file__).resolve().parent.parent / "target" / "release" / "sumcp"
PER_PROJECT = 3   # newest N top-level sessions per project
TOTAL_CAP = 15    # overall sample ceiling
EDIT_HEAVY = 3    # a session with >= this many edits should usually fire a signal


def discover() -> list[tuple[str, Path]]:
    """Return (project_label, session_path) pairs, newest-first per project,
    spread across projects up to TOTAL_CAP. Top-level *.jsonl only (subagent
    files live in <uuid>/subagents/ and are excluded)."""
    if not PROJECTS.is_dir():
        return []
    per_project: list[tuple[str, list[Path]]] = []
    for proj in sorted(PROJECTS.iterdir()):
        if not proj.is_dir():
            continue
        mains = sorted(
            proj.glob("*.jsonl"), key=lambda p: p.stat().st_mtime, reverse=True
        )
        if mains:
            # A readable-ish label: last path segment of the decoded project dir.
            label = proj.name.split("-")[-1] or proj.name
            per_project.append((label, mains))
    # Sort projects by how many sessions they have (richest first) for a stable
    # sample, then take up to PER_PROJECT each until we hit the cap.
    per_project.sort(key=lambda t: len(t[1]), reverse=True)
    out: list[tuple[str, Path]] = []
    for label, mains in per_project:
        for p in mains[:PER_PROJECT]:
            out.append((label, p))
            if len(out) >= TOTAL_CAP:
                return out
    return out


def analyze(path: Path) -> dict:
    """Run sumcp --json on one session; return a compact result dict."""
    raw_bytes = path.stat().st_size
    transcript_tokens = raw_bytes / CHARS_PER_TOKEN
    try:
        proc = subprocess.run(
            [str(SUMCP), "--file", str(path), "--json"],
            capture_output=True,
            timeout=60,
        )
    except subprocess.TimeoutExpired:
        return {"error": "timeout", "transcript_tokens": transcript_tokens}
    if proc.returncode != 0:
        msg = proc.stderr.decode(errors="replace").strip().splitlines()
        return {"error": f"exit {proc.returncode}: {msg[-1] if msg else '?'}",
                "transcript_tokens": transcript_tokens}
    try:
        d = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        return {"error": f"bad json: {e}", "transcript_tokens": transcript_tokens}

    # Compact re-serialize = realistic on-wire debrief size.
    compact = json.dumps(d, separators=(",", ":"))
    debrief_tokens = len(compact) / CHARS_PER_TOKEN
    totals = d.get("totals", {})
    edits = totals.get("edits", 0)
    struggles = d.get("top_struggles", []) or []
    top3 = [os.path.basename(s.get("file", "?")) for s in struggles[:3]]
    zero_fire = (not struggles) and edits >= EDIT_HEAVY
    return {
        "error": None,
        "actions": totals.get("actions", 0),
        "edits": edits,
        "transcript_tokens": transcript_tokens,
        "debrief_tokens": debrief_tokens,
        "ratio": (transcript_tokens / debrief_tokens) if debrief_tokens else 0,
        "top3": top3,
        "zero_fire": zero_fire,
        "flags": d.get("flags", {}),
    }


def main() -> int:
    if not SUMCP.exists():
        print(f"release binary missing: {SUMCP}\nrun: cargo build --release", file=sys.stderr)
        return 1
    sample = discover()
    if not sample:
        print("no transcripts found under ~/.claude/projects", file=sys.stderr)
        return 1

    print(f"suMCP internal validation sweep — {len(sample)} sessions "
          f"(chars/{CHARS_PER_TOKEN} tokens)\n")
    header = f"{'project':<14} {'session':<10} {'acts':>4} {'edits':>5} " \
             f"{'transcript':>11} {'debrief':>8} {'ratio':>7}  top-3 struggle files / error"
    print(header)
    print("-" * len(header))

    ratios, errors, zero_fires, ok = [], [], [], 0
    for label, path in sample:
        r = analyze(path)
        sid = path.stem[:8]
        if r["error"]:
            errors.append((label, sid, r["error"]))
            print(f"{label:<14} {sid:<10} {'':>4} {'':>5} "
                  f"{r['transcript_tokens']:>10.0f}t {'':>8} {'':>7}  ERROR: {r['error']}")
            continue
        ok += 1
        ratios.append(r["ratio"])
        if r["zero_fire"]:
            zero_fires.append((label, sid, r["edits"]))
        top = ", ".join(r["top3"]) if r["top3"] else "(no signals)"
        zf = "  ⚠ZERO-FIRE" if r["zero_fire"] else ""
        print(f"{label:<14} {sid:<10} {r['actions']:>4} {r['edits']:>5} "
              f"{r['transcript_tokens']:>10.0f}t {r['debrief_tokens']:>7.0f}t "
              f"{r['ratio']:>6.0f}x  {top}{zf}")

    print("\n── summary ──")
    print(f"ran clean: {ok}/{len(sample)}   errors: {len(errors)}   "
          f"zero-fire (edits≥{EDIT_HEAVY}, no signal): {len(zero_fires)}")
    if ratios:
        ratios.sort()
        median = ratios[len(ratios) // 2]
        print(f"token ratio  median {median:.0f}x   min {min(ratios):.0f}x   "
              f"max {max(ratios):.0f}x   (transcript ÷ debrief)")
    for label, sid, err in errors:
        print(f"  ERROR  {label}/{sid}: {err}")
    for label, sid, e in zero_fires:
        print(f"  ZERO-FIRE  {label}/{sid}: {e} edits, no struggle signal")
    return 0


if __name__ == "__main__":
    sys.exit(main())
