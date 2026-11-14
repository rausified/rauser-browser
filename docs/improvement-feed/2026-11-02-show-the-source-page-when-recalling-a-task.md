## 2026-11-02 — Show the source page when recalling a task

Change: add a compact original-page cue and an explicit route to inspect or
reopen the recalled task (`brwr-k6rg`). Why: keyboard recall reuses the complete
stored instruction on the current page, whereas Tasks-page Reuse also reopens
its original page; page-specific instructions deserve a visible distinction
before Run. Keep the draft, current-page context and Enter/Run boundary intact;
never navigate automatically. Where: `extension/task-recall.ts`, the task-bar
context in `extension/panel.ts`, and `extension/tests/panel/task-bar.test.mjs`.
- Review 2026-11-12: **Keep.** Shows the original page for a recalled instruction so a page-specific task is not accidentally run on the current page.
