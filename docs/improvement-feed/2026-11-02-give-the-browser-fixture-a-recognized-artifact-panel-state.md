## 2026-11-02 — Give the browser fixture a recognized-artifact panel state

Change: add a `recognized` state to `extension/tests/a11y/fixture.mjs` whose
fake host answers `get_artifact` with a recognized artifact (mode, measures,
`within`, incoming navigation), a saved summary with an overview, and one
running task, and audit it in light and dark. Why: the side panel redesign
added the page card's chips, the Summary card's tags and run-card state
indicators, but the fixture only models an unrecognized page with no summary,
so the axe gate never measures those surfaces and visual checks need DOM
injection by hand. Where: `extension/tests/a11y/fixture.mjs` and
`pages.test.mjs`; the same state would let a screenshot check compare the
panel against the design.
- Review 2026-11-12: **Keep.** Audits the actual artifact, summary, and running task panel states in both themes, closing a coverage hole.
