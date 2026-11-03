## 2026-11-02 — Generate padded tokens in differential tests

Change: extend `scripts/adapter-templates-diff.mjs`'s `genValue` to emit
unpadded tokens and one/two-character padding within the selected bounds,
then exercise these values through titles and aliases as well as URLs.
Keep malformed padding among the near misses. Why: `brwr-tp1` reproduced
padded-title failures in both engines, but the random generator uses only the
body alphabet and cannot explore this supported grammar family. Fixed shared
vectors now cover it; generated combinations would catch future drift with
case modifiers, bounds and title structure. Where: the generator and
`scripts/tests/adapter-templates-diff.test.mjs`; tracked as `brwr-ys8y`.
