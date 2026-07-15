#!/usr/bin/env python3
"""Structure-preserving, default-deny sanitizer for Claude Code transcripts.

Turns a raw `~/.claude/projects/**/session.jsonl` into a committable fixture:
the *skeleton* the parser and signals depend on is preserved exactly, while
all private *content* is synthesized. Default-deny (ADR A7 + /ship M2): a
string field is passed through only if its key is on the structural allowlist;
every other string is scrubbed, and any unrecognized key is reported so review
is directed, not blind.

Preserved verbatim: type, all ids (uuid/parentUuid/sessionId/requestId/
message.id/tool_use id/agentId), timestamps (including absence and equality
collisions), version, usage numbers, tool names, is_error / booleans,
structuredPatch line numbers, and a whitelist of harness error strings that
detectors key off (e.g. "File has not been read yet").

Synthesized: file paths (deterministically, so the same real path always maps
to the same fake path — churn grouping survives), and all free text (commands,
code, prompts, stdout/stderr) become length-approximate placeholders with any
embedded paths remapped consistently.

Usage:  python3 scripts/sanitize.py <raw.jsonl> <out.jsonl>
"""
import hashlib
import json
import pathlib
import re
import sys

# ---- field policy -----------------------------------------------------------

# Keys whose STRING value is structural and passes through untouched.
PRESERVE_STR = {
    "type", "uuid", "parentUuid", "leafUuid", "sessionId", "requestId",
    "timestamp", "version", "userType", "entrypoint", "role", "model", "id",
    "tool_use_id", "sourceToolUseID", "stop_reason", "stop_sequence",
    "permissionMode", "mode", "level", "subtype", "gitBranch", "agentId",
    "promptSource", "origin", "service_tier", "inference_geo", "messageId",
    # content-block discriminators
    "name",
}
# Keys whose value is a filesystem path → deterministic synthetic path.
PATH_KEYS = {"cwd", "file_path", "filePath", "path", "notebook_path"}
# Keys whose value is free text → scrub (paths inside are remapped).
SCRUB_KEYS = {
    "command", "description", "old_string", "new_string", "oldString",
    "newString", "content", "text", "thinking", "lastPrompt", "aiTitle",
    "prompt", "stdout", "stderr", "summary", "title", "message",
    "surpriseField",  # example of an unknown field; default-deny catches it anyway
}
# Harness error fragments detectors depend on — preserved verbatim if present.
HARNESS_ERRORS = [
    "File has not been read yet",
    "String to replace not found",
    "tool_use_error",
    "has not been read",
    "doesn't want to proceed",
    "No such file or directory",
]
EXIT_CODE = re.compile(r"Exit code \d+")
PATH_RE = re.compile(r"(/[\w.\-/]+|[\w.\-/]+\.\w{1,5})")

_unknown_keys: set[str] = set()
_path_map: dict[str, str] = {}

# A real JSON field name is a short identifier. Anything else used as a key
# (e.g. AskUserQuestion answers keyed by the question text) is content and must
# be scrubbed, not passed through.
SAFE_KEY_RE = re.compile(r"^[\w.\-]{1,40}$")


def clean_key(k: str) -> str:
    """Preserve identifier-like keys; replace content-bearing keys."""
    if k in PRESERVE_STR or k in PATH_KEYS or k in SCRUB_KEYS or SAFE_KEY_RE.match(k):
        return k
    _unknown_keys.add("<content-key>")
    return "q_" + hashlib.sha1(k.encode()).hexdigest()[:8]


def fake_path(real: str) -> str:
    """Deterministic synthetic path preserving extension (stable churn keys)."""
    if real in _path_map:
        return _path_map[real]
    ext = pathlib.PurePosixPath(real).suffix or ""
    h = hashlib.sha1(real.encode()).hexdigest()[:8]
    fake = f"/work/proj/f_{h}{ext}"
    _path_map[real] = fake
    return fake


def scrub_text(s: str) -> str:
    """Redact free text but keep harness errors and remap embedded paths."""
    keep = [frag for frag in HARNESS_ERRORS if frag in s]
    m = EXIT_CODE.search(s)
    if m:
        keep.append(m.group(0))
    # remap any path-like tokens so stderr/command paths stay consistent
    paths = [p for p in PATH_RE.findall(s) if "/" in p or "." in p]
    remapped = " ".join(fake_path(p) for p in dict.fromkeys(paths))
    placeholder = "x" * min(len(s), 40)
    return " ".join(part for part in [placeholder, remapped, *keep] if part).strip()


def sanitize(node, key=None):
    """Recursively sanitize one JSON value under `key` (default-deny)."""
    if isinstance(node, dict):
        return {clean_key(k): sanitize(v, k) for k, v in node.items()}
    if isinstance(node, list):
        return [sanitize(v, key) for v in node]
    if isinstance(node, str):
        if key in PRESERVE_STR:
            return node
        if key in PATH_KEYS:
            return fake_path(node)
        if key in SCRUB_KEYS:
            return scrub_text(node)
        # DEFAULT DENY: unknown string field — record it and scrub.
        if key is not None:
            _unknown_keys.add(key)
        return scrub_text(node)
    # numbers, bools, null pass through (usage/line-numbers/is_error verbatim)
    return node


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: sanitize.py <raw.jsonl> <out.jsonl>", file=sys.stderr)
        return 2
    src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
    out, bad = [], 0
    for line in src.read_text().splitlines():
        if not line.strip():
            continue
        try:
            out.append(json.dumps(sanitize(json.loads(line))))
        except json.JSONDecodeError:
            bad += 1  # a malformed raw line stays malformed (parser must cope)
            out.append(line)
    dst.write_text("\n".join(out) + "\n")
    print(f"sanitized {len(out)} lines ({bad} unparseable passed through) -> {dst}")
    if _unknown_keys:
        print("REVIEW these unrecognized keys (scrubbed by default-deny): "
              f"{sorted(_unknown_keys)}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
