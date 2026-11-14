## 2026-11-02 — Search the topic a summary actually describes

Change: extend Tasks history search to match a summary's overview as well as
its page title, keeping existing matches for older results. Why: brwr-sm3
now delivers the useful one-sentence description to each retained run, but
`filterTasks` still searches only the title and task instruction; remembering
a decision described by the summary does not find its run. Where:
`extension/task-history.ts`, `extension/tasks.ts`, their tests, and brwr-khqi
(discovered from brwr-sm3).
- Review 2026-11-12: **Keep.** Searches summary overview text in Tasks so users can find a remembered decision without knowing its page title.
