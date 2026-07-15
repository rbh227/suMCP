#!/usr/bin/env python3
"""T0.2 acceptance test: the debrief narration contract.

Validates a debrief (produced by a live agent following skills/debrief/SKILL.md
from the mock payloads) against the DoD:
  1. under 500 tokens (chars/4 approximation)
  2. names all top-3 struggle files from struggle_areas.json
  3. cites action idxs in brackets (dereferenceable via evidence()) — >= 3
  4. grounds claims in at least two distinct signal categories
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
DEBRIEF = ROOT / "fixtures" / "mock-payloads" / "sample-debrief.md"
STRUGGLES = ROOT / "fixtures" / "mock-payloads" / "struggle_areas.json"


def main() -> int:
    errors = []
    if not DEBRIEF.exists():
        print(f"FAIL: {DEBRIEF.relative_to(ROOT)} does not exist")
        return 1
    text = DEBRIEF.read_text()

    tokens = len(text) / 4
    if tokens > 500:
        errors.append(f"over budget: ~{tokens:.0f} tokens > 500")

    expected = [f["file"] for f in json.loads(STRUGGLES.read_text())["files"]]
    for path in expected:
        basename = path.rsplit("/", 1)[-1]
        if basename not in text:
            errors.append(f"missing struggle file: {basename}")

    citations = re.findall(r"\[[\d,\s–-]+\]", text)
    if len(citations) < 3:
        errors.append(f"only {len(citations)} evidence citations, need >= 3")

    categories = ["rework", "failure", "re-read", "churn", "edit", "fumble",
                  "blind", "revert"]
    hits = [c for c in categories if c in text.lower()]
    if len(hits) < 2:
        errors.append(f"grounded in {len(hits)} signal categories, need >= 2")

    if errors:
        print("FAIL sample-debrief:")
        for e in errors:
            print(f"   - {e}")
        return 1
    print(f"ok   sample-debrief (~{tokens:.0f}/500 tokens, "
          f"{len(citations)} citations, files: {len(expected)}/{len(expected)})")
    print("\nNARRATION CONTRACT SATISFIED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
