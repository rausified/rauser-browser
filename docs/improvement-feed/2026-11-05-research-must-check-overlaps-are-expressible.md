## 2026-11-05 — Starter research must check that intended overlaps are expressible

Change: add a step to the research checklist (research method, spec
`2026-11-03-starter-families-design.md` "Research method") that runs the
`overlap_report` check and the generator against the drafted family file
before research is called done, not only once a plan is written from it.

Why: Splunk Cloud's research (`docs/research/blind/splunk-cloud/research.md`)
called the `splunk.app` / `splunk.dashboard` overlap intended — `splunk.app`
must win so the Search page is recorded under the right type instead of as a
`dashboard` named `search` (plan `docs/superpowers/plans/
2026-11-05-starters-d2-box-webex-splunk.md`, "Design challenge"). But today's
generator's `overlaps` validation only accepts `"disjoint"`; it has no way to
declare a container-first pair. That gap surfaced only during planning (Open
decision 2), after the research was already written, and forced deferring
the whole `splunk-cloud` family out of this PR (D2 ships Box and Webex only;
the generator change and the Splunk family are a follow-up, listed in the D2
pull request; bead to follow). Had the research step itself run the overlap
report and the generator against a draft family file, the gap would have
surfaced then, when research could
either flag the limitation up front or choose a different type split, instead
of blocking a finished plan's task list.

Where: the research checklist (`docs/research/blind/` README/checklist) and
`AGENTS.md` "Adding a starter".
- Review 2026-11-12: **Keep.** During research, run the draft family through the generator and overlap check. This catches unexpressible rule relationships before a starter plan is considered ready.
