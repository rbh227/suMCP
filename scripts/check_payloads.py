#!/usr/bin/env python3
"""Validate mock MCP payloads against the payload contract (SPEC §2, T0.1).

Checks, per tool payload in fixtures/mock-payloads/:
  1. parses as JSON
  2. fits its token cap (chars/4 approximation)
  3. every finding-like object carries: kind, tier, exact, confidence, idxs
  4. envelope carries: v, session.identified_by (provenance, ADR A4), truncated

Exit 0 = contract satisfied. Used as the T0.1 acceptance test and kept as a
regression check for real payloads later.
"""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MOCK_DIR = ROOT / "fixtures" / "mock-payloads"

# token caps per SPEC §2 (tokens ≈ chars/4)
CAPS_TOKENS = {
    "session_overview": 1000,
    "struggle_areas": 1500,
    "file_story": 1500,
    "blind_spots": 1000,
    "context_health": 1000,
    "evidence": 1500,
}

FINDING_FIELDS = {"kind", "tier", "exact", "confidence", "idxs"}
TIERS = {"T1", "T2", "T3"}
CONFIDENCES = {"high", "medium", "low"}


def iter_findings(node):
    """Yield every dict that self-declares as a finding (has a 'kind' key)."""
    if isinstance(node, dict):
        if "kind" in node:
            yield node
        for v in node.values():
            yield from iter_findings(v)
    elif isinstance(node, list):
        for v in node:
            yield from iter_findings(v)


def check(path: pathlib.Path, cap_tokens: int) -> list[str]:
    errors = []
    raw = path.read_text()
    tokens = len(raw) / 4
    if tokens > cap_tokens:
        errors.append(f"over cap: ~{tokens:.0f} tokens > {cap_tokens}")
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as e:
        return errors + [f"invalid JSON: {e}"]

    if payload.get("v") != 0:
        errors.append("missing/wrong schema version 'v' (expected 0)")
    if "error" in payload:
        return errors  # error payloads have their own minimal shape

    session = payload.get("session", {})
    if session.get("identified_by") not in {"tool_use_id", "explicit", "cli_latest"}:
        errors.append("session.identified_by missing or invalid (ADR A4 provenance)")
    if "truncated" not in payload:
        errors.append("missing 'truncated' flag")

    for f in iter_findings(payload):
        missing = FINDING_FIELDS - f.keys()
        if missing:
            errors.append(f"finding {f.get('kind')}: missing {sorted(missing)}")
            continue
        if f["tier"] not in TIERS:
            errors.append(f"finding {f['kind']}: bad tier {f['tier']}")
        if f["confidence"] not in CONFIDENCES:
            errors.append(f"finding {f['kind']}: bad confidence {f['confidence']}")
        if not isinstance(f["exact"], bool):
            errors.append(f"finding {f['kind']}: 'exact' must be bool")
        if not (isinstance(f["idxs"], list) and all(isinstance(i, int) for i in f["idxs"])):
            errors.append(f"finding {f['kind']}: 'idxs' must be list of ints")
    return errors


def main() -> int:
    failures = 0
    for tool, cap in CAPS_TOKENS.items():
        path = MOCK_DIR / f"{tool}.json"
        if not path.exists():
            print(f"FAIL {tool}: {path.relative_to(ROOT)} does not exist")
            failures += 1
            continue
        errors = check(path, cap)
        if errors:
            failures += 1
            print(f"FAIL {tool}:")
            for e in errors:
                print(f"   - {e}")
        else:
            print(f"ok   {tool} (~{len(path.read_text()) / 4:.0f}/{cap} tokens)")

    err_path = MOCK_DIR / "error_ambiguous_session.json"
    if not err_path.exists():
        print("FAIL error_ambiguous_session.json missing (ADR A4 fail-closed shape)")
        failures += 1
    else:
        e = json.loads(err_path.read_text())
        if e.get("error") != "ambiguous_session" or "candidates" not in e:
            print("FAIL error payload must carry error='ambiguous_session' + candidates")
            failures += 1
        else:
            print("ok   error_ambiguous_session")

    print(f"\n{'CONTRACT SATISFIED' if failures == 0 else f'{failures} FAILURES'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
