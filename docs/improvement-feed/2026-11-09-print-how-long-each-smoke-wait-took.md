## 2026-11-09 — Print how long each smoke wait took

Change: have `waitUntil` and `waitFor` in `scripts/smoke-macos.mjs` print one
line with the wait's name and elapsed time when a step fails, and keep the
last few successful waits' times in the failure message. Why: brwr-b508's
cause was found only by subtracting CI log timestamps. The step lasted 30.4 s
and its timed-out wait 30.3 s, so the wait before it passed in about 0.1 s,
which is impossible for a click that needs a 1 s poll. A printed "opened
after 80 ms with 0 clicks" would have pointed straight at the wrong panel
document. Where: `waitUntil`/`waitFor` and `step()` in
`scripts/smoke-macos.mjs`.
- Review 2026-11-12: **Keep.** Print wait durations and click counts on smoke failure; CI timestamps were needed to find a wrong panel document.
