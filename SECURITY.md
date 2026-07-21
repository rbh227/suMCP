# Security

suMCP is read-only and offline by design:

- It never makes network calls.
- It never executes code from a transcript; it parses text.
- The only files it writes are under `$HOME` (everything self-contained in
  `~/.claude/sumcp/`), and `install` backs up anything it touches and is fully
  reversible with `uninstall`.

## Reporting

If you find a way suMCP reads outside a session file, writes outside its
documented paths, or leaks transcript content, please open a private security
advisory on the GitHub repository rather than a public issue.
