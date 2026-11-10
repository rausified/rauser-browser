## 2026-11-09 — Generate the starter-adapter mirror in vectors.json

Change: Have `scripts/generate-starters.mjs` write the copy of the generated starter adapters that `protocol/adapter-templates/vectors.json` keeps, which today is maintained by hand, or have the test read the generated output directly instead of a hand copy. `--check` should then flag any drift.

Why: In PR #395 (Jira renamed-product titles), the only real test failure came from this hand-kept mirror. `extension/tests/starter-adapters.test.mjs` compares it byte-for-byte with the generated adapters, so a one-line title template change also needed a hand edit to a second file. Every starter title or rule change pays that cost, and a contributor who doesn't know about the mirror gets a confusing failure. Everything else derived from the family files is already generated.

Where: `scripts/generate-starters.mjs` and its `--check`; `protocol/adapter-templates/vectors.json`; `extension/tests/starter-adapters.test.mjs`; `protocol/starters/README.md` "Generating".
