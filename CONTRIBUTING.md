# Contributing to suMCP

Thanks for looking. suMCP is a deterministic, read-only Rust MCP server; the
bar for changes is that they stay honest, cheap, and reproducible.

## Dev setup

```bash
git clone https://github.com/rbh227/suMCP && cd suMCP
cargo build
cargo test --workspace
```

## Donating a fixture

The most useful contribution is a real session that suMCP got wrong or right in
an interesting way. Never commit a raw transcript: it contains your paths, code,
and prompts. Sanitize it first:

```bash
python3 scripts/sanitize.py path/to/your/session.jsonl fixtures/your-case.jsonl
```

The sanitizer preserves the structure the parser and signals depend on (ids,
timestamps, tool names, the harness error strings detectors key off) and
synthesizes everything private (paths become deterministic fakes, free text
becomes length-approximate placeholders). Review the output, then open a PR.

## Adding a signal

Detectors live in `crates/sumcp-core/src/signals/`. A new signal must carry a
**tier**, an **exact-vs-heuristic** flag, a **confidence**, and the action
indices that prove it, and must add a fixture that makes it fire (and a
zero-fire case). Ranking stays a transparent weighted count with a visible
per-category breakdown.
