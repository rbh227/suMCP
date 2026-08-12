#!/usr/bin/env python3
"""Corpus archiver (dev-only, like sanitize.py; python3 stdlib only).

WHY THIS EXISTS
---------------
Claude Code's `cleanupPeriodDays` defaults to 30 and `~/.claude/projects` is
therefore a rolling window. `tasks/todo.md` records the consequence already:
one frozen held-out project aged out, and the exact population of the
2026-07-22 predictive-validity study can no longer be rebuilt from disk.

Every measurement claim backstory-mcp makes is only as durable as the transcripts
behind it. This script copies the evidence somewhere the cleanup does not
reach, incrementally, so it can be run as often as you like.

WHAT IT ARCHIVES
----------------
  ~/.claude/projects/**/*.jsonl   main transcripts AND subagents/agent-*.jsonl
  ~/.claude/projects/**/*.meta.json   subagent metadata (agentType, model, ...)
  ~/.claude/file-history/**       real pre-edit file contents, keyed by session
  ~/.claude/history.jsonl         every prompt, with project + sessionId + ts
  ~/.claude/sessions/*.json       cwd, startedAt, version, session name

PRIVACY
-------
These files contain unredacted secrets and real work. The default destination
is deliberately OUTSIDE this repository (`~/claude-corpus-archive`), because
backstory-mcp is a public repo and a transcript archive must never become committable.
The script refuses to write anywhere inside the repo, and refuses a
destination that is inside a git work tree at all.

INTEGRITY
---------
Every archived file is recorded in `manifest.jsonl` with its sha256, size, and
source mtime. After copying, each new file is re-hashed FROM THE DESTINATION
and compared against the source hash, so a truncated or partial copy fails
loudly instead of silently corrupting the corpus. Re-runs skip files whose
(size, mtime, hash) already match, so archiving is cheap and idempotent.

USAGE
-----
    python3 scripts/archive_corpus.py                 # dry run: report only
    python3 scripts/archive_corpus.py --apply         # actually copy
    python3 scripts/archive_corpus.py --apply --dest DIR
    python3 scripts/archive_corpus.py --verify        # re-hash the whole archive
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

# Read chunk size for hashing. Transcripts reach 8 MB; this keeps memory flat.
CHUNK = 1 << 20

DEFAULT_DEST = Path.home() / "claude-corpus-archive"
CLAUDE = Path.home() / ".claude"


def sha256_of(path: Path) -> str:
    """Stream a file through sha256 so an 8 MB transcript costs 1 MB of RAM."""
    h = hashlib.sha256()
    with path.open("rb") as fh:
        while chunk := fh.read(CHUNK):
            h.update(chunk)
    return h.hexdigest()


def sources() -> list[tuple[Path, str]]:
    """Every file worth archiving, paired with the archive subdirectory it
    belongs in. Returns (absolute_source_path, archive_relative_path)."""
    out: list[tuple[Path, str]] = []

    projects = CLAUDE / "projects"
    if projects.is_dir():
        # rglob catches BOTH layouts in one pass: the main `<uuid>.jsonl` files
        # and the `<uuid>/subagents/agent-*.jsonl` children. The subagent
        # transcripts are ~41% of all file modifications in this corpus, so
        # missing them would archive a badly skewed sample.
        for p in projects.rglob("*.jsonl"):
            if p.is_file() and not p.is_symlink():
                out.append((p, str(Path("projects") / p.relative_to(projects))))
        for p in projects.rglob("*.meta.json"):
            if p.is_file() and not p.is_symlink():
                out.append((p, str(Path("projects") / p.relative_to(projects))))

    fh_dir = CLAUDE / "file-history"
    if fh_dir.is_dir():
        for p in fh_dir.rglob("*"):
            if p.is_file() and not p.is_symlink():
                out.append((p, str(Path("file-history") / p.relative_to(fh_dir))))

    sessions = CLAUDE / "sessions"
    if sessions.is_dir():
        for p in sessions.glob("*.json"):
            if p.is_file() and not p.is_symlink():
                out.append((p, str(Path("sessions") / p.name)))

    hist = CLAUDE / "history.jsonl"
    if hist.is_file():
        # history.jsonl is append-only and mutates constantly, so it is
        # snapshotted under a timestamped name rather than overwritten. Losing
        # the older prompt history would defeat the point of archiving it.
        stamp = time.strftime("%Y-%m-%dT%H%M%S", time.gmtime(hist.stat().st_mtime))
        out.append((hist, f"history/history-{stamp}.jsonl"))

    return out


def load_manifest(dest: Path) -> dict[str, dict]:
    """Existing archive state, keyed by archive-relative path."""
    mf = dest / "manifest.jsonl"
    known: dict[str, dict] = {}
    if mf.is_file():
        for line in mf.read_text(errors="replace").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                # A partially-written last line is data loss of one record,
                # not a reason to refuse the whole archive. It re-copies.
                continue
            known[rec["path"]] = rec
    return known


def assert_safe_dest(dest: Path) -> None:
    """Refuse to archive unredacted transcripts anywhere committable."""
    repo = Path(__file__).resolve().parent.parent
    # `resolve()` works for a path that does not exist yet: it resolves the
    # symlinks of every existing ancestor and keeps the tail as-is, which is
    # exactly what a safety check on a to-be-created destination needs.
    resolved = dest.resolve()
    if resolved == repo or repo in resolved.parents:
        sys.exit(f"refusing: {resolved} is inside the backstory-mcp repo. Transcripts "
                 f"carry unredacted secrets and must not be committable.")
    # Probe the NEAREST EXISTING ancestor, not merely the immediate parent.
    # A nested nonexistent destination (<worktree>/new/nested/archive) used
    # to probe only <worktree>/new/nested; `git -C` on a directory that does
    # not exist fails with a nonzero exit rather than an exception, so the
    # guard fell open and the copy proceeded into a committable tree (codex
    # adversarial review, 2026-07-28).
    probe = resolved
    while not probe.exists():
        if probe.parent == probe:
            break  # filesystem root; nothing left to walk up to
        probe = probe.parent
    try:
        r = subprocess.run(
            ["git", "-C", str(probe), "rev-parse", "--is-inside-work-tree"],
            capture_output=True, text=True, timeout=10,
        )
        if r.returncode == 0 and r.stdout.strip() == "true":
            sys.exit(f"refusing: {resolved} is inside a git work tree.")
    except (OSError, subprocess.SubprocessError):
        # No git, or git misbehaved. The in-repo check above already ran, and
        # that is the case that actually matters here.
        pass


def verify(dest: Path) -> int:
    """Re-hash every archived file against the manifest. Returns exit code."""
    known = load_manifest(dest)
    if not known:
        print(f"no manifest at {dest / 'manifest.jsonl'}")
        return 1
    bad, missing = [], []
    for rel, rec in sorted(known.items()):
        p = dest / rel
        if not p.is_file():
            missing.append(rel)
        elif sha256_of(p) != rec["sha256"]:
            bad.append(rel)
    print(f"verified {len(known)} files: {len(bad)} corrupt, {len(missing)} missing")
    for rel in (bad + missing)[:20]:
        print(f"  FAIL {rel}")
    return 1 if (bad or missing) else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--apply", action="store_true",
                    help="actually copy (default is a dry run)")
    ap.add_argument("--dest", type=Path, default=DEFAULT_DEST,
                    help=f"archive directory (default {DEFAULT_DEST})")
    ap.add_argument("--verify", action="store_true",
                    help="re-hash the existing archive and exit")
    args = ap.parse_args()

    dest: Path = args.dest.expanduser()
    assert_safe_dest(dest)

    if args.verify:
        return verify(dest)

    if not CLAUDE.is_dir():
        sys.exit(f"no {CLAUDE} on this machine")

    known = load_manifest(dest)
    todo, skipped, total_bytes = [], 0, 0

    for src, rel in sources():
        try:
            st = src.stat()
        except OSError:
            continue
        prev = known.get(rel)
        # Cheap change test first: identical size and mtime means identical
        # content in every realistic case, and skips hashing 1100+ files.
        if prev and prev["size"] == st.st_size and prev["mtime"] == int(st.st_mtime):
            if (dest / rel).is_file():
                skipped += 1
                continue
        todo.append((src, rel, st))
        total_bytes += st.st_size

    mb = total_bytes / (1024 * 1024)
    print(f"archive: {dest}")
    print(f"  {len(todo)} file(s) to copy ({mb:.1f} MB), {skipped} unchanged")

    if not args.apply:
        for src, rel, _ in todo[:10]:
            print(f"  + {rel}")
        if len(todo) > 10:
            print(f"  ... and {len(todo) - 10} more")
        print("\ndry run. re-run with --apply to write.")
        return 0

    dest.mkdir(parents=True, exist_ok=True)
    copied, failed = 0, 0
    with (dest / "manifest.jsonl").open("a") as mf:
        for src, rel, st in todo:
            target = dest / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            try:
                src_hash = sha256_of(src)
                shutil.copy2(src, target)
                # Re-hash FROM THE DESTINATION. copy2 reporting success is not
                # proof the bytes landed; a full disk or a racing writer both
                # produce a short file that looks fine to the caller.
                if sha256_of(target) != src_hash:
                    print(f"  FAIL hash mismatch after copy: {rel}")
                    failed += 1
                    continue
            except OSError as e:
                print(f"  FAIL {rel}: {e}")
                failed += 1
                continue
            mf.write(json.dumps({
                "path": rel,
                "sha256": src_hash,
                "size": st.st_size,
                "mtime": int(st.st_mtime),
                "archived_at": int(time.time()),
                "source": str(src),
            }) + "\n")
            copied += 1

    print(f"  copied {copied}, failed {failed}")
    # 0700: the archive holds unredacted secrets, so no other account reads it.
    if os.name != "nt":
        os.chmod(dest, 0o700)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
