## 2026-11-09 — Fix interstitial titles before certifying starters from history

Change: Add a title policy to the adapter-template format before running history certification again. Two options: a type lists the generic titles it should ignore, or a tab title that fails every template never replaces a title a template already matched. Then add a `scripts/dev` grader that prints the per-type table (real URLs, rules confirmed, titles correct, real URLs missed) from a local evidence file. That table shows which types are close to passing before a lane is spent on certifying them.

Why: The first history check graded 30 development types on real visits and certified none of them. Only two types had every rule confirmed, and both fell short on titles or real URLs that no rule matched. Sign-in pages, password gates, app-shell loading titles and error pages were recorded as the item's title on almost every type that had real visits. Those titles are a property of the format, not of any one starter's rules, so more research on a single starter would not have fixed it.

Where: `adapter-templates.md` §9 (title templates), both engines, `docs/superpowers/specs/2026-11-03-starter-families-design.md` "Research method", and a new `scripts/dev` history grader that never writes evidence into the repo.
