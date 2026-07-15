#!/usr/bin/env python3
"""T0.2 acceptance test: the debrief narration contract.

Hardened per the /ship test-engineer finding: cited [idx] values are now
cross-checked against the idxs that actually appear in the mock payloads
(a debrief citing a fabricated [999] fails), and the signal-category check no
longer counts substrings like "edit" that any debrief-shaped text contains.

  1. under 500 tokens (chars/4 — prose approximation, fair here)
  2. names all top-3 struggle files from struggle_areas.json
  3. cites >= 3 distinct [idx] values, ALL of which exist in some payload
  4. grounds claims in >= 2 distinct signal categories (real signal words)
"""
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MOCK = ROOT / "fixtures" / "mock-payloads"
DEBRIEF = MOCK / "sample-debrief.md"


def all_payload_idxs() -> set[int]:
    """Every integer idx appearing anywhere in the mock payloads."""
    found = set()

    def walk(node):
        if isinstance(node, dict):
            for k, v in node.items():
                if k == "idxs" and isinstance(v, list):
                    found.update(i for i in v if isinstance(i, int))
                elif k == "idx" and isinstance(v, int):
                    found.add(v)
                else:
                    walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    for p in MOCK.glob("*.json"):
        walk(json.loads(p.read_text()))
    return found


def main() -> int:
    errors = []
    if not DEBRIEF.exists():
        print(f"FAIL: {DEBRIEF.relative_to(ROOT)} does not exist")
        return 1
    text = DEBRIEF.read_text()

    tokens = len(text) / 4
    if tokens > 500:
        errors.append(f"over budget: ~{tokens:.0f} tokens > 500")

    expected = [f["file"] for f in json.loads((MOCK / "struggle_areas.json").read_text())["files"]]
    for path in expected:
        if path.rsplit("/", 1)[-1] not in text:
            errors.append(f"missing struggle file: {path}")

    # collect cited idxs; each bracket may hold a comma-separated list
    cited = set()
    for group in re.findall(r"\[([\d,\s]+)\]", text):
        cited.update(int(n) for n in re.findall(r"\d+", group))
    if len(cited) < 3:
        errors.append(f"only {len(cited)} distinct evidence citations, need >= 3")
    real = all_payload_idxs()
    fabricated = cited - real
    if fabricated:
        errors.append(f"cites idxs not present in any payload: {sorted(fabricated)}")

    # real signal vocabulary — no generic words like "edit"
    categories = ["rework", "failure", "re-read", "churn", "fumble", "blind",
                  "revert", "flip", "thrash", "instant", "unread"]
    hits = [c for c in categories if c in text.lower()]
    if len(hits) < 2:
        errors.append(f"grounded in {len(hits)} signal categories, need >= 2")

    if errors:
        print("FAIL sample-debrief:")
        for e in errors:
            print(f"   - {e}")
        return 1
    print(f"ok   sample-debrief (~{tokens:.0f}/500 tokens, {len(cited)} citations "
          f"all dereferenceable, {len(hits)} signal categories)")
    print("\nNARRATION CONTRACT SATISFIED")
    return 0


if __name__ == "__main__":
    sys.exit(main())
