#!/usr/bin/env python3
"""Blind-arm pilot for the review-precision experiment (dev-only, stdlib only).

WHY THIS EXISTS. `scripts/power_estimate.py` powered the experiment on three
inputs that were assumed rather than measured, and an adversarial review
(2026-08-11) correctly rejected all three:

  1. findings per commit (assumed 3 or 6, never measured)
  2. within-commit clustering (ignored entirely; findings inside one commit
     share a diff, so treating each as an independent observation
     understates the required sample size)
  3. the baseline invalid share p_a (borrowed as 0.563 from arXiv:2607.03316,
     which measured different tooling on different repositories)

All three are measurable on this repository with the reviewer that will
actually be used, and measuring them needs NO new product code, because the
blind arm is just a reviewer looking at a diff.

THIS IS NOT A DETOUR. What this collects IS arm A of the real experiment.
Arm B reruns the same commits once `review_context` exists.

Usage:
  pilot_blind_arm.py collect --commits 20      # slow, one Codex run per commit
  pilot_blind_arm.py report

Output: .superpowers/sdd/pilot-blind-arm.json (git-ignored scratch; it holds
real finding text and real paths, so it must not be committed).
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

RAW = Path(".superpowers/sdd/pilot-blind-arm.json")

# The prompt is deliberately the plain blind review: a diff and nothing else.
# Arm B will differ from this by exactly one thing, the recorded context, so
# any wording change here must be mirrored there or the arms stop being
# comparable.
#
# Built by concatenation, NOT str.format: the prompt contains literal JSON
# braces describing the required output shape, and format() reads those as
# placeholders and raises KeyError('"findings"').
PROMPT_HEAD = (
    "Adversarially review the following commit. Report only material findings: "
    "correctness, security, data loss, and concurrency defects. Skip style and "
    "naming. For each finding give the file, the line range, and one sentence "
    "on what can go wrong.\n\n"
    "Return ONLY a JSON object on the final line, of the form "
    '{"findings": [{"file": "...", "line_start": 1, "line_end": 2, '
    '"summary": "..."}]}. An empty findings list is a valid and expected '
    "answer for a clean commit.\n\n"
)


def build_prompt(sha: str, message: str, diff: str) -> str:
    return f"{PROMPT_HEAD}COMMIT {sha}\n{message}\n\nDIFF:\n{diff}"


def load() -> dict:
    if RAW.exists():
        return json.loads(RAW.read_text())
    return {"commits": {}}


def save(state: dict) -> None:
    RAW.parent.mkdir(parents=True, exist_ok=True)
    RAW.write_text(json.dumps(state, indent=1, sort_keys=True))


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], capture_output=True, text=True, check=True
    ).stdout


def extract_findings(stdout: str) -> list[dict] | None:
    """Pull the findings array out of Codex's output.

    Returns None when no parsable object was found, which is recorded
    distinctly from an empty list: "the reviewer said nothing was wrong" and
    "we could not read the reviewer's answer" are different facts, and
    conflating them would silently bias the finding-count distribution
    downward.
    """
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            d = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(d, dict) and isinstance(d.get("findings"), list):
            return d["findings"]
    return None


def cmd_collect(args: argparse.Namespace) -> None:
    state = load()
    shas = git("log", "--format=%H", f"-{args.commits}", args.rev).split()
    print(f"{len(shas)} commits to review blind", file=sys.stderr)

    for i, sha in enumerate(shas, 1):
        if sha in state["commits"]:
            continue  # never re-spend a Codex run
        message = git("log", "-1", "--format=%s", sha).strip()
        diff = git("show", "--format=", "--unified=10", sha)
        if not diff.strip():
            state["commits"][sha] = {"skipped": "empty diff"}
            save(state)
            continue
        # A giant diff would blow the reviewer's context and produce a
        # non-comparable review, so it is recorded as skipped rather than
        # silently reviewed under different conditions.
        if len(diff) > args.max_diff_chars:
            state["commits"][sha] = {
                "skipped": f"diff {len(diff)} chars over cap {args.max_diff_chars}"
            }
            save(state)
            print(f"  [{i}/{len(shas)}] {sha[:8]} SKIP (large)", file=sys.stderr)
            continue

        prompt = build_prompt(sha, message, diff)
        try:
            out = subprocess.run(
                ["codex", "exec", "--skip-git-repo-check", prompt],
                capture_output=True,
                text=True,
                timeout=args.timeout,
            )
            findings = extract_findings(out.stdout)
        except (OSError, subprocess.TimeoutExpired) as e:
            state["commits"][sha] = {"error": str(e)[:200]}
            save(state)
            print(f"  [{i}/{len(shas)}] {sha[:8]} ERROR {e}", file=sys.stderr)
            continue

        state["commits"][sha] = {
            "subject": message,
            "diff_chars": len(diff),
            "findings": findings,
            "unparsed": findings is None,
        }
        save(state)
        n = "unparsed" if findings is None else len(findings)
        print(f"  [{i}/{len(shas)}] {sha[:8]} findings={n}", file=sys.stderr)


def cmd_report(args: argparse.Namespace) -> None:
    state = load()
    counts, skipped, unparsed = [], 0, 0
    for rec in state["commits"].values():
        if "skipped" in rec or "error" in rec:
            skipped += 1
        elif rec.get("unparsed"):
            unparsed += 1
        else:
            counts.append(len(rec["findings"]))

    print(f"reviewed:  {len(counts)} commits")
    print(f"skipped:   {skipped}   unparsed: {unparsed}")
    if not counts:
        print("\nNo usable reviews yet. Run `collect` first.")
        return

    counts.sort()
    n = len(counts)
    mean = sum(counts) / n
    zeros = sum(1 for c in counts if c == 0)
    # Variance matters as much as the mean here: the rejected power estimate
    # assumed a FIXED yield per commit, and the spread is what determines
    # whether a commit count can be derived at all.
    var = sum((c - mean) ** 2 for c in counts) / n
    print(f"\nfindings per commit: mean {mean:.2f}  variance {var:.2f}")
    print(f"  min {counts[0]}  median {counts[n // 2]}  max {counts[-1]}")
    print(f"  commits with ZERO findings: {zeros}/{n} ({100 * zeros / n:.0f}%)")
    print(f"  total findings collected: {sum(counts)}")
    print("\nWhat this replaces:")
    print(f"  assumed 3 or 6 findings/commit -> measured {mean:.2f}")
    print("  assumed fixed yield            -> measured spread above")
    print("  p_a = 0.563 (external)         -> needs adjudication of these")
    print("                                    findings to measure locally")
    print("\nNEXT: adjudicate these findings valid/invalid to get this repo's")
    print("real p_a, then power the experiment by SIMULATION over the measured")
    print("distribution, not a closed-form independent-samples formula.")


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(required=True)
    c = sub.add_parser("collect")
    c.set_defaults(fn=cmd_collect)
    c.add_argument("--commits", type=int, default=20)
    c.add_argument("--rev", default="HEAD")
    c.add_argument("--timeout", type=int, default=900)
    c.add_argument("--max-diff-chars", type=int, default=60000)
    r = sub.add_parser("report")
    r.set_defaults(fn=cmd_report)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
