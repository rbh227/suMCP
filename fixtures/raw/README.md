# fixtures/raw/ — donor session drop zone (gitignored)

Put raw Claude Code transcripts here for analysis and fixture-making:

```bash
cp ~/.claude/projects/<project-dir>/<session-id>.jsonl fixtures/raw/
# include subagents if present:
cp -r ~/.claude/projects/<project-dir>/<session-id>/ fixtures/raw/
```

Everything in this directory is ignored by git (see .gitignore) — raw
transcripts contain your prompts, code, and file paths and must never be
committed or pushed.

To turn a raw session into a committable fixture:

```bash
python3 scripts/sanitize.py fixtures/raw/<session>.jsonl fixtures/<name>.jsonl
```

then review the output by hand before `git add` (SPEC §6 / ADR A7).
