## 2026-11-02 — Enable the merge queue on main

AGENTS.md has assumed a merge queue since late October ("merge main in only to
resolve a real conflict"), but GitHub reports none on main (`mergeQueue: null`,
checked 2026-11-02). Without it, nothing rebuilds a waiting PR on current main,
so agents merge main in by hand, and each push cancels the in-flight ~40-minute
macOS run. PR #243 (Confluence starter) spent 2h38m from open to merge, across
six CI runs, five merges from main and a `final-stretch` hold, for a change whose
code took about an hour. Change: enable the merge queue on main in the repo
settings (needs admin; merge method: merge commit; the `merge_group` trigger
and `merge-hold` handling already exist in `ci.yml` and `merge-hold.yml`), then
drop the "not enabled today" sentence from AGENTS.md "Review and merge cadence".
`scripts/dev/pr-watch.mjs` already prints the queue-aware arm command once
GitHub reports a queue. Where: GitHub repository settings, AGENTS.md.
