#!/usr/bin/env python3
"""Validate mock MCP payloads against the payload contract (SPEC §2, T0.1).

Hardened per the /ship code-review + test-engineer findings: the checker now
enforces the schema's own rules that earlier passed silently (kind enum,
non-empty idxs, note-when-heuristic, full error shape, ranking payloads echo
weights + breakdown). It is the regression test the Rust `report.rs` builders
must also pass.

Token caps use chars/3.5 — compact JSON tokenizes hotter than prose, so
chars/4 undercounts in the unsafe direction for a hard cap.
"""
import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
MOCK_DIR = ROOT / "fixtures" / "mock-payloads"
CHARS_PER_TOKEN = 3.5  # conservative for compact JSON (was 4; see /ship finding)

CAPS_TOKENS = {
    "session_overview": 1000,
    "struggle_areas": 1500,
    "file_story": 1500,
    "blind_spots": 1000,
    "context_health": 1000,
    "evidence": 1500,
}
# payloads whose top-level content is ranked and must echo weights (SPEC §7)
RANKING_PAYLOADS = {"struggle_areas"}

FINDING_FIELDS = {"kind", "tier", "exact", "confidence", "idxs"}
TIERS = {"T1", "T2", "T3"}
CONFIDENCES = {"high", "medium", "low"}
# closed set from docs/payload-schema.md
KINDS = {
    "churn", "rework", "failure_loop", "thrash", "fumble", "blind_write_attempt",
    "true_revert", "flip", "user_corrected", "write_no_reread",
    "read_unreferenced", "large_write_instant_accept", "opening_move",
}


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


def check_finding(f) -> list[str]:
    errors = []
    missing = FINDING_FIELDS - f.keys()
    if missing:
        return [f"finding {f.get('kind', '<no kind>')}: missing {sorted(missing)}"]
    k = f["kind"]
    if k not in KINDS:
        errors.append(f"finding: unknown kind '{k}' (not in schema enum)")
    if f["tier"] not in TIERS:
        errors.append(f"finding {k}: bad tier {f['tier']}")
    if f["confidence"] not in CONFIDENCES:
        errors.append(f"finding {k}: bad confidence {f['confidence']}")
    if not isinstance(f["exact"], bool):
        errors.append(f"finding {k}: 'exact' must be bool")
    # heuristics must explain themselves (schema line: exact:false carries a note)
    if f["exact"] is False and not f.get("note"):
        errors.append(f"finding {k}: heuristic (exact:false) requires a 'note'")
    idxs = f["idxs"]
    # bool is an int subclass in Python — exclude it explicitly
    if not (isinstance(idxs, list) and idxs
            and all(isinstance(i, int) and not isinstance(i, bool) for i in idxs)):
        errors.append(f"finding {k}: 'idxs' must be a non-empty list of ints")
    return errors


def check_success(name, payload) -> list[str]:
    errors = []
    if payload.get("v") != 0:
        errors.append("missing/wrong schema version 'v' (expected 0)")
    session = payload.get("session", {})
    if not session.get("id"):
        errors.append("session.id missing")
    if session.get("identified_by") not in {"tool_use_id", "explicit", "cli_latest"}:
        errors.append("session.identified_by missing or invalid (ADR A4 provenance)")
    if "truncated" not in payload or not isinstance(payload["truncated"], bool):
        errors.append("missing/non-bool 'truncated' flag")
    if name in RANKING_PAYLOADS:
        if "weights" not in payload:
            errors.append("ranking payload must echo 'weights' (SPEC §7, never opaque)")
        if not any("breakdown" in f for f in payload.get("files", [])):
            errors.append("ranking payload must show per-file 'breakdown'")
    for f in iter_findings(payload):
        errors += check_finding(f)
    return errors


def check_error(payload) -> list[str]:
    errors = []
    if payload.get("v") != 0:
        errors.append("error payload missing 'v'")
    if payload.get("error") != "ambiguous_session":
        errors.append("error must be 'ambiguous_session'")
    for field in ("message", "hint", "candidates"):
        if field not in payload:
            errors.append(f"error payload missing '{field}'")
    for c in payload.get("candidates", []):
        if "id" not in c or "cwd_match" not in c:
            errors.append("candidate missing id/cwd_match")
    return errors


def check(path: pathlib.Path, name: str, cap_tokens: int) -> list[str]:
    raw = path.read_text()
    errors = []
    tokens = len(raw) / CHARS_PER_TOKEN
    if tokens > cap_tokens:
        errors.append(f"over cap: ~{tokens:.0f} tokens > {cap_tokens}")
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as e:
        return errors + [f"invalid JSON: {e}"]
    # take the error branch only when 'error' is the declared payload mode
    if payload.get("error"):
        return errors + check_error(payload)
    return errors + check_success(name, payload)


def main() -> int:
    failures = 0
    for tool, cap in CAPS_TOKENS.items():
        path = MOCK_DIR / f"{tool}.json"
        if not path.exists():
            print(f"FAIL {tool}: {path.relative_to(ROOT)} does not exist")
            failures += 1
            continue
        errors = check(path, tool, cap)
        if errors:
            failures += 1
            print(f"FAIL {tool}:")
            for e in errors:
                print(f"   - {e}")
        else:
            print(f"ok   {tool} (~{len(path.read_text()) / CHARS_PER_TOKEN:.0f}/{cap} tokens)")

    err_path = MOCK_DIR / "error_ambiguous_session.json"
    if not err_path.exists():
        print("FAIL error_ambiguous_session.json missing (ADR A4 fail-closed shape)")
        failures += 1
    else:
        errors = check_error(json.loads(err_path.read_text()))
        if errors:
            failures += 1
            print("FAIL error_ambiguous_session:")
            for e in errors:
                print(f"   - {e}")
        else:
            print("ok   error_ambiguous_session")

    print(f"\n{'CONTRACT SATISFIED' if failures == 0 else f'{failures} FAILURES'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
