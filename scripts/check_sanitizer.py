#!/usr/bin/env python3
"""T1.2 acceptance test for scripts/sanitize.py (default-deny sanitizer).

Two properties, both required:
  A. STRUCTURE PRESERVED — the identity/skeleton signals key off survives:
     line count, per-type histogram, the uuid/message.id/requestId/tool_use-id
     graph, usage token sums, structuredPatch line-number shapes, and known
     harness error strings (blind-write detection keys off the literal
     "File has not been read yet").
  B. SECRETS GONE — canary secrets planted in content fields (code, commands,
     prompts, and an unexpected custom field) do NOT survive.
"""
import collections
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SANITIZE = ROOT / "scripts" / "sanitize.py"

CANARIES = [
    "sk-supersecretapikey1234567890",     # in a Bash command
    "MyProprietaryClassName",             # in edited code
    "acme-corp-confidential-project",     # in a user prompt
    "hunter2password",                    # in an unexpected custom field
]

# A raw line set exercising the fields the sanitizer must handle.
RAW_LINES = [
    {"type": "user", "uuid": "u1", "parentUuid": None, "sessionId": "s1",
     "timestamp": "2026-07-01T10:00:00Z", "cwd": "/Users/real/secret-proj",
     "message": {"role": "user", "content": "please fix acme-corp-confidential-project"}},
    {"type": "assistant", "uuid": "u2", "parentUuid": "u1", "sessionId": "s1",
     "requestId": "req1", "timestamp": "2026-07-01T10:00:05Z",
     "message": {"id": "m1", "model": "claude-x", "role": "assistant",
                 "usage": {"input_tokens": 100, "output_tokens": 50,
                           "cache_read_input_tokens": 10, "cache_creation_input_tokens": 5},
                 "content": [
                     {"type": "tool_use", "id": "t1", "name": "Bash",
                      "input": {"command": "curl -H 'auth: sk-supersecretapikey1234567890' x",
                                "description": "call api"}},
                     {"type": "tool_use", "id": "t2", "name": "Edit",
                      "input": {"file_path": "/Users/real/secret-proj/src/Secret.ts",
                                "old_string": "class MyProprietaryClassName {}",
                                "new_string": "class MyProprietaryClassName { x() {} }"}}]}},
    {"type": "user", "uuid": "u3", "parentUuid": "u2", "sessionId": "s1",
     "timestamp": "2026-07-01T10:00:06Z",
     "message": {"role": "user", "content": [
         {"type": "tool_result", "tool_use_id": "t1", "is_error": True,
          "content": "Exit code 1"}]},
     "toolUseResult": {"stdout": "", "stderr": "boom", "interrupted": False}},
    {"type": "user", "uuid": "u4", "parentUuid": "u3", "sessionId": "s1",
     "timestamp": "2026-07-01T10:00:07Z",
     "message": {"role": "user", "content": [
         {"type": "tool_result", "tool_use_id": "t2", "is_error": True,
          "content": "File has not been read yet. Read it first."}],},
     "toolUseResult": {"filePath": "/Users/real/secret-proj/src/Secret.ts",
                       "oldString": "class MyProprietaryClassName {}",
                       "newString": "class MyProprietaryClassName { x() {} }",
                       "userModified": False,
                       "structuredPatch": [{"oldStart": 1, "oldLines": 1,
                                            "newStart": 1, "newLines": 1,
                                            "lines": ["-old", "+new"]}]}},
    # an unknown line type with an unexpected secret-bearing field
    {"type": "custom-future-type", "uuid": "u5", "timestamp": "2026-07-01T10:00:08Z",
     "surpriseField": "hunter2password"},
    # secret stored as a JSON KEY (real case: AskUserQuestion answers keyed by
    # question text) — the sanitizer must scrub keys, not just values.
    {"type": "user", "uuid": "u6", "timestamp": "2026-07-01T10:00:09Z",
     "toolUseResult": {"answers": {"secret-question-about-acme-corp-confidential-project?": "yes"}}},
    # an untimestamped event (amendment 5 case)
    {"type": "permission-mode", "mode": "acceptEdits", "sessionId": "s1"},
]


def graph(lines):
    """Identity relations that must survive sanitization."""
    uuids, msgids, reqids, tool_ids = set(), set(), set(), set()
    for o in lines:
        if o.get("uuid"):
            uuids.add(o["uuid"])
        m = o.get("message", {})
        if isinstance(m, dict) and m.get("id"):
            msgids.add(m["id"])
        if o.get("requestId"):
            reqids.add(o["requestId"])
        content = m.get("content") if isinstance(m, dict) else None
        if isinstance(content, list):
            for c in content:
                if isinstance(c, dict) and c.get("id"):
                    tool_ids.add(c["id"])
    return uuids, msgids, reqids, tool_ids


def usage_sum(lines):
    total = 0
    for o in lines:
        u = o.get("message", {}).get("usage") if isinstance(o.get("message"), dict) else None
        if isinstance(u, dict):
            total += sum(v for v in u.values() if isinstance(v, int))
    return total


def main() -> int:
    if not SANITIZE.exists():
        print(f"FAIL: {SANITIZE.relative_to(ROOT)} does not exist")
        return 1

    tmp_in = ROOT / "fixtures" / "raw" / "_canary.jsonl"
    tmp_out = ROOT / "fixtures" / "_canary_sanitized.jsonl"
    tmp_in.parent.mkdir(parents=True, exist_ok=True)
    tmp_in.write_text("\n".join(json.dumps(o) for o in RAW_LINES) + "\n")

    proc = subprocess.run([sys.executable, str(SANITIZE), str(tmp_in), str(tmp_out)],
                          capture_output=True, text=True)
    errors = []
    if proc.returncode != 0:
        errors.append(f"sanitizer exited {proc.returncode}: {proc.stderr.strip()}")
        _cleanup(tmp_in, tmp_out)
        _report(errors)
        return 1

    out_text = tmp_out.read_text()
    out_lines = [json.loads(l) for l in out_text.splitlines() if l.strip()]

    # B. secrets gone (check the full serialized text)
    for canary in CANARIES:
        if canary in out_text:
            errors.append(f"SECRET SURVIVED: {canary!r}")

    # A. structure preserved
    if len(out_lines) != len(RAW_LINES):
        errors.append(f"line count changed: {len(RAW_LINES)} -> {len(out_lines)}")
    if _hist(RAW_LINES) != _hist(out_lines):
        errors.append(f"type histogram changed: {_hist(RAW_LINES)} -> {_hist(out_lines)}")
    if graph(RAW_LINES) != graph(out_lines):
        errors.append("identity graph (uuid/msgid/reqid/tool_use id) changed")
    if usage_sum(RAW_LINES) != usage_sum(out_lines):
        errors.append(f"usage token sum changed: {usage_sum(RAW_LINES)} -> {usage_sum(out_lines)}")
    if "File has not been read yet" not in out_text:
        errors.append("harness error string 'File has not been read yet' lost (breaks fumble detector)")
    if "Exit code 1" not in out_text:
        errors.append("harness error string 'Exit code 1' lost")
    # structuredPatch line-number shape preserved
    sp = [o for o in out_lines if isinstance(o.get("toolUseResult"), dict)
          and o["toolUseResult"].get("structuredPatch")]
    if not sp or sp[0]["toolUseResult"]["structuredPatch"][0].get("oldStart") != 1:
        errors.append("structuredPatch line-number shape not preserved")

    _cleanup(tmp_in, tmp_out)
    _report(errors)
    return 1 if errors else 0


def _hist(lines):
    return collections.Counter(o.get("type") for o in lines)


def _cleanup(*paths):
    for p in paths:
        p.unlink(missing_ok=True)


def _report(errors):
    if errors:
        print("FAIL sanitizer:")
        for e in errors:
            print(f"   - {e}")
    else:
        print("ok   sanitizer: structure preserved, all canary secrets removed")
        print("\nSANITIZER CONTRACT SATISFIED")


if __name__ == "__main__":
    sys.exit(main())
