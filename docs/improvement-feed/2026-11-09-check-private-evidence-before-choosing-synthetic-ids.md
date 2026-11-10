## 2026-11-09 — Check private evidence before choosing a synthetic ID, and scrub derived substrings too

A real Miro board ID from John's browsing history leaked into 19 tracked
files (docs, DESIGN.md, adapter-templates.md, Rust and TS test
vectors/fixtures) via commits on 2026-11-01. Two things worth automating:

1. Before picking a synthetic replacement value, grep the private evidence
   file (`/Users/jrauser/ENDURANCE/state/brauser-history/families.jsonl`,
   never read into committed content) for it. Several other real board IDs
   in that file shared a common prefix with the leaked one, so a naively
   "random-looking" replacement could still have collided with a *different*
   real ID also present there.
2. A plain string replace of the full ID missed a one-character-short
   *derived* occurrence (a wrong-length boundary test case in
   `host/tests/fixtures/miro-board-urls.json` and
   `protocol/adapter-templates/vectors.json`) that still fingerprinted the
   real value. `git grep` on the full literal isn't enough; also search for
   prefix/truncated substrings before declaring a scrub complete.

Suggested change: a pre-commit or CI check that reads the private evidence
file locally (never shipped, never read in CI) and fails a commit/PR
touching `protocol/`, `docs/adapters/`, or test fixtures if any token from
that file's real URLs appears verbatim. Optional and local-only; must never
ship the evidence file or its contents anywhere.
