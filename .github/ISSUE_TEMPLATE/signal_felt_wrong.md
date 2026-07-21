---
name: A signal felt wrong
about: The ranking accused the wrong file, or missed a real struggle
labels: signal-accuracy
---

suMCP is an honesty tool, so a wrong accusation matters more than a miss. Thank
you for reporting it.

**What the ranking said**
(the top files and their score breakdown)

**What actually happened in that session**

**Which was it**
- [ ] false accusation (a file was ranked that you did not struggle with)
- [ ] miss (a file you struggled with was not ranked)

**A sanitized fixture**
This is the most useful part. Run
`python3 scripts/sanitize.py <session>.jsonl fixtures/felt-wrong.jsonl`
and attach the output so the case can become a regression test.
