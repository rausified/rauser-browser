## 2026-11-13 — Prove a reflow by its normalized text

Branch `jrausercisco/df-docs-split` (delay-fix lane L2b, `brwr-jtd8`).
A Markdown reflow is safe when the whitespace-joined text is identical before and after, because that is what Markdown renders a paragraph's soft line breaks as: `node scripts/sentence-lines.mjs --compare <before> <after>` checks it, and `scripts/tests/sentence-lines.test.mjs` checks that reflowing any root Markdown file changes nothing else.
The merge driver's AGENTS.md handling is tested with real two-branch merges in scratch repositories (`scripts/tests/merge-generated.test.mjs`): two PRs that each add a row merge cleanly in either order to the same file, a conflicting hand-written edit stays an ordinary conflict, and an AGENTS.md from before the generated table still merges as plain text.
