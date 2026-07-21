#!/usr/bin/env python3
"""T5.3 convergent-validity check (read-only).

A struggle file backed by MULTIPLE independent signal categories (churn AND
rework AND re-reads) is far more credible than one driven by a single noisy
metric. This measures, across the same real-session sample as validate_sweep,
how many distinct signal categories back each top struggle finding.

This is *convergent/internal* evidence — the signals agreeing with each other —
NOT criterion validity (matching your memory). High multi-signal share means the
ranking isn't an artifact of one confound (e.g. mechanical churn alone).
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import validate_sweep as vs  # reuse discover() + SUMCP + token divisor


def categories(finding: dict) -> set[str]:
    return set((finding.get("breakdown") or {}).keys())


def main() -> int:
    if not vs.SUMCP.exists():
        print("run: cargo build --release", file=sys.stderr)
        return 1
    sample = vs.discover()

    top1_sizes: Counter[int] = Counter()   # distinct-category count for rank-1 files
    pooled_sizes: Counter[int] = Counter()  # same, pooled over top-3
    cooccur: Counter[frozenset] = Counter()  # which category sets co-occur
    per_cat: Counter[str] = Counter()
    findings_analyzed = 0

    for _label, path in sample:
        try:
            proc = subprocess.run(
                [str(vs.SUMCP), "--file", str(path), "--json"],
                capture_output=True, timeout=60,
            )
            if proc.returncode != 0:
                continue
            d = json.loads(proc.stdout)
        except Exception:
            continue
        struggles = d.get("top_struggles") or []
        if not struggles:
            continue
        # rank-1
        c1 = categories(struggles[0])
        if c1:
            top1_sizes[len(c1)] += 1
        # pooled top-3
        for f in struggles[:3]:
            cats = categories(f)
            if not cats:
                continue
            findings_analyzed += 1
            pooled_sizes[len(cats)] += 1
            cooccur[frozenset(cats)] += 1
            for c in cats:
                per_cat[c] += 1

    def share_multi(counter: Counter[int]) -> tuple[int, int, float]:
        total = sum(counter.values())
        multi = sum(v for k, v in counter.items() if k >= 2)
        return multi, total, (100 * multi / total if total else 0.0)

    print("suMCP convergent-validity — distinct signal categories per struggle finding\n")

    m, t, pct = share_multi(top1_sizes)
    print(f"Rank-1 files (one per session): {t} findings")
    for k in sorted(top1_sizes):
        print(f"    {k} categor{'y' if k == 1 else 'ies'}: {top1_sizes[k]}")
    print(f"    → multi-signal (≥2): {m}/{t} = {pct:.0f}%\n")

    m, t, pct = share_multi(pooled_sizes)
    print(f"Top-3 findings pooled: {t} findings")
    for k in sorted(pooled_sizes):
        print(f"    {k} categor{'y' if k == 1 else 'ies'}: {pooled_sizes[k]}")
    print(f"    → multi-signal (≥2): {m}/{t} = {pct:.0f}%\n")

    print("Most common category combinations:")
    for combo, n in cooccur.most_common(8):
        print(f"    {n:>2}×  {{{', '.join(sorted(combo))}}}")

    print("\nPer-category appearance across all top-3 findings:")
    for cat, n in per_cat.most_common():
        print(f"    {n:>3}  {cat}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
