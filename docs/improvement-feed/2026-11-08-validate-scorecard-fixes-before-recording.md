## 2026-11-08 — Validate a scorecard fix before recording it

Change: before a scorecard (or a plan's "resolved" decision) records a rule
fix, run the proposed rule through the engine once, for example one line of
`{"adapter": …, "url": …}` into `scripts/adapter-templates-eval.mjs`, and
paste the result beside the fix.

Why: `docs/research/blind/SCORECARD.md` miss 1 proposed matching "any
single-segment portal path" for ServiceNow knowledge articles, keyed on the
query. The D3 plan (`docs/superpowers/plans/2026-11-05-starters-d3-aha-servicenow.md`)
took that as resolved and specified the exact rule:
`{ path = "/{_}", query = { id = "kb_article", … } }`. The engine rejects it
with `no_literal_segment` (`adapter-templates.md` §4.1: `{_}` is not an
anchor, and a query binding never anchors). The plan's headline fix couldn't
ship, which only came out at implementation time. A five-second eval would
have caught it when the scorecard was written.

Where: the scorecard method in `docs/research/blind/` and the starter plan
template's "Open decisions" ("resolved" needs an engine result for any rule
it fixes).
