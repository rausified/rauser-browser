# Tasks Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Task-bar runs move out of the side panel onto a new extension page, `tasks.html`, with a list on the left and the chosen task on the right. The page holds the full paged history of retained tasks. The panel keeps only the field, **Run** and **Tasks ↗** ("Tasks · N running ↗").

**Architecture:**
- `list_runs` gains additive optional `kind` and `before` (cursor) fields, plus `more` in its answer, so the page can page through every task of one kind.
- The panel loses its task cards. A small `TaskTracker` keeps the running count and the `#task-announce` announcements.
- The page reuses `RunWatcher` polling through a new `TaskDetail` view (it replaces `TaskCard`), and pure list logic in `task-history.ts`.
- Reuse travels from the page to one panel through a `chrome.storage.session` key.

**Tech Stack:** Rust (`brauser` host, `host/src/service`, `brauser-protocol`), `protocol/schema.json` + `protocol/generate.py`, the MV3 extension in TypeScript, the Node test runner, the headless macOS smoke `scripts/smoke-macos.mjs`.

**Spec:** `docs/superpowers/specs/2026-10-28-tasks-page-design.md` (approved 2026-10-28). Executors read both.

## Global Constraints

- Work only in `/Users/jrauser/ENDURANCE/work/rb-tasks-page` on `feat/tasks-page`. Never use bare `git stash`. Implementer tasks never push, open PRs or spawn agents. Task 9 is the orchestrator's.
- Protocol stays **8**. Every change is additive and optional: the request fields `ListRunsRequest.kind` and `ListRunsRequest.before`, the new `RunCursor` def, and the response field `RunsListed.more`. `runs` stays `maxItems: 20`. With `kind` absent, the answer is byte-identical to today's (no `more` key).
- No new dependencies and no new extension permission. The `tabs` permission is not added. Find the Tasks tab with `chrome.runtime.getContexts` (Chrome 116, the manifest's `minimum_chrome_version`).
- Render agent text (instruction, title, report, stream) with `textContent` only. Never render HTML or Markdown, and never linkify. **Open page** opens the run's `url` field, which the host already bounds to http(s), and never a URL found in agent text.
- The page must not call `list_runs` unless a harness is ready (`agent_status` ready) or `runsMayExist` (`run-card.ts`) is true. It does not poll while `document.hidden`.
- No `diag` call ever receives an instruction, report, title or stream (DESIGN.md §15).
- Read DESIGN.md §5.7 and `extension/panel.css` before any markup or style change. Reuse `.secondary`, `.compact`, `.warning`, `.visually-hidden`, `.setup-warning` and `.run-card-*`. The colours are `#1557aa` primary, `#5b6470` secondary and `#8885` borders. No glass.
- Exact copy:
  - the button: "Tasks ↗"; "Tasks · N running ↗" (N counts queued and running)
  - the start-lost line: "The task may have started — check Tasks before running it again."
  - empty page: "No tasks yet. Start one from the side panel."
  - Reuse done: "Instruction sent to the task bar."
  - the failed/canceled note: "Changes the agent already made stay; check before running it again."
  - Windows: "Tasks are not supported on Windows yet."
  - older host: "Rebuild the Brauser host to see task history."
- Commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Per-task checks are focused: the named tests, plus `cargo fmt --check` and `cargo clippy` for Rust tasks, or `npm run build:extension && node --test <file>` and `npm run lint` for extension tasks. The full README.md gate runs once, in Task 9.

## Review Focus

1. **A task finishes while the page is paging through history.** It can arrive both in the first page (as active) and in a later page (as finished). The list must show it once, as finished. This is pinned by `mergeRuns keeps one view per run, the fresher state` (Task 3).
2. **The hash names a run the list does not have** (pruned, garbage, from another profile). The page selects the newest task and does not throw. This is pinned by `an unknown #run= selects the newest task` (Task 5).
3. **The Tasks tab is hidden, then shown again.** Polling stops while hidden and resumes with one immediate refresh when shown. This is pinned by `the page stops polling while hidden and refreshes on show` (Task 5).
4. **No harness and no record of runs.** Opening the page must not wake the service. It shows the agent setup text with **Open settings**. This is pinned by `the page does not list runs without a harness or a run record` (Task 5).
5. **A hostile report** (`<img src=x onerror=alert(1)>`, a `javascript:` URL, Markdown links). It shows as literal text, with no element or link created. This is pinned by `TaskDetail renders the instruction, title, report and stream as text only` (Task 4).

---

### Task 1: Protocol — `kind`, `before`, `more` on `list_runs`

**Files:**
- Modify: `protocol/schema.json` (`ListRunsRequest`, `RunsListed`, new `RunCursor`)
- Regenerate: `protocol/src/generated.rs`, `protocol/ts/generated.ts`
- Modify: `protocol/README.md` (new subsection "Task history (added within version 8)" after "Agent tasks")
- Test: `protocol/tests/` (beside the existing round-trip tests)

**Interfaces:**
- Produces:
  - `RunCursor { created_at: string (1..64), run_id: string (36) }`
  - `ListRunsRequest.kind?: RunKind`
  - `ListRunsRequest.before?: RunCursor`
  - `RunsListed.more?: boolean`

- [ ] **Step 1: Write failing protocol tests.** `list_runs_without_kind_round_trips_unchanged` serializes `ListRunsRequest` with no new fields and asserts the JSON has no `kind`, `before` or `more` key. `list_runs_with_kind_and_cursor_round_trips` covers `{"kind":"task","before":{"created_at":"2026-10-28T11:34:00-07:00","run_id":"<uuid>"}}`. `runs_listed_more_is_optional` checks that both with and without `more` parse.
- [ ] **Step 2:** Run `cargo test -p brauser-protocol`. Expected: FAIL (unknown fields).
- [ ] **Step 3:** Add the fields to `schema.json` as optional properties (not in `required`). Run `python3 protocol/generate.py`, then `python3 protocol/generate.py --check`.
- [ ] **Step 4:** Run `cargo test -p brauser-protocol`. Expected: PASS.
- [ ] **Step 5:** Write the README subsection. It covers:
  - the semantics in Task 2;
  - that `before` without `kind` is `invalid_request`;
  - that an older host answers `invalid_request` "invalid or unsupported request JSON", so the page asks for a rebuild, and host and extension deploy together.

  Then commit with `feat(protocol): kind filter and cursor paging for list_runs`.

### Task 2: Service and host — filtered, paged `list_runs`

**Files:**
- Modify: `host/src/service/supervisor.rs` (`list_runs`), `host/src/service/server.rs` (`Request::ListRuns` arm)
- Test: `supervisor.rs` tests beside `list_runs_keeps_active_and_newest_finished_runs_newest_first`; `server.rs` tests; `host/src/native.rs` tests beside the existing `ListRuns` forward test

**Interfaces:**
- Consumes: Task 1 types.
- Produces:
  - `pub struct ListQuery { pub kind: Option<RunKind>, pub before: Option<RunCursor> }` (`Default`)
  - `Supervisor::list_runs_where(&self, query: &ListQuery) -> (Vec<RunView>, Option<bool>)`
  - `Supervisor::list_runs()` stays, as `list_runs_where(&ListQuery::default()).0`, so existing callers and tests are untouched.

Semantics (a record without `kind` is a summary). Order is `(created_at, run_id)` descending, the order `RunStore::list` already sorts by (both are strings).
- `kind` absent: today's behaviour exactly, and `more` is `None`.
- `kind` set, no `before`: every queued or running run of that kind, then the newest finished of that kind up to `LIST_LIMIT` in total. `more = Some(finished_of_kind > room)`.
- `kind` set with `before`: only finished runs of that kind strictly older than the cursor, at most `LIST_LIMIT`. `more = Some(remaining > 0)`.
- `before` without `kind`: refused as `invalid_request` with "before needs kind" in `server.rs`.
- Views are cut as today: `site_draft` and `task_report` are `None`, and the instruction goes through `list_instruction`.

- [ ] **Step 1: Write failing supervisor tests.**
  - `list_runs_where_kind_task_skips_other_kinds`: 3 summaries and 2 tasks give 2 task views.
  - `list_runs_where_first_page_has_every_active_task_then_newest_finished`: 4 active and 30 finished tasks give 20 views, active first by order, and `more == Some(true)`.
  - `list_runs_where_before_pages_finished_runs_without_gaps_or_repeats`: 45 finished tasks, walking pages with each last view's cursor, give every id once and the last page has `more == Some(false)`.
  - `list_runs_where_equal_created_at_orders_by_run_id`: 21 finished tasks with one `created_at` give two pages that hold all 21 once, ordered by `run_id` desc.
  - `list_runs_where_without_kind_matches_list_runs`: the same views as `list_runs()` and `more == None`.
- [ ] **Step 2:** Run `cargo test -p brauser list_runs_where`. Expected: FAIL.
- [ ] **Step 3:** Implement `ListQuery` and `list_runs_where`. Wire `server.rs` to `RunsListed { runs, more }`, and add the `before`-without-`kind` refusal.
- [ ] **Step 4: Server and host tests.**
  - `list_runs_before_without_kind_is_invalid_request` (`server.rs`).
  - `list_runs_forwards_kind_and_before_unchanged` (`native.rs`, with `FakeService`): the forwarded request JSON carries the same `kind` and `before`.
  - `twenty_task_list_views_with_longest_fields_fit_one_frame`: 20 task views with the longest URL, title and cut instruction serialize to at most `MAX_OUTBOUND_BYTES`.

  Run `cargo test -p brauser`. Expected: PASS.
- [ ] **Step 5:** Run `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`, plus Windows clippy as CI does (`--target x86_64-pc-windows-msvc` if installed; otherwise note it for the gate). Commit with `feat(service): list_runs kind filter and cursor paging`.

### Task 3: Extension — shape check and task-history logic

**Files:**
- Modify: `extension/protocol-shape.ts` (`runs_listed`: `more` is absent or boolean)
- Create: `extension/task-history.ts`
- Modify: `extension/run-card.ts` (`MAX_DISMISSED_RUNS = 1000`; export `dismissedIds` and `rememberDismissed`; export `isTerminal`)
- Modify: `extension/build.mjs` (add `task-history.js`), `package.json` `test:extension` (add the new test file)
- Test: `extension/tests/task-history.test.mjs`, `extension/tests/run-card.test.mjs`

**Interfaces:**
- Consumes: Task 1 TS types.
- Produces (all in `task-history.ts`):
  - `type StatusFilter = "all" | "running" | "done" | "failed" | "canceled"` (running includes queued)
  - `listTaskPage(host: Pick<HostClient,"call">, before: RunCursor | null): Promise<{ runs: RunView[]; more: boolean }>` sends `{type:"list_runs", kind:"task", before?}`. A missing `more` counts as `false`.
  - `listAllTasks(host): Promise<RunView[]>` walks pages using the last view's `{created_at, run_id}` as the cursor. It stops when `more` is false, after `MAX_HISTORY_PAGES = 100` pages, or when a page does not advance the cursor.
  - `mergeRuns(known: RunView[], fresh: RunView[]): RunView[]` dedupes by `run_id`. When both copies exist it keeps the terminal one, otherwise the one from `fresh`, then sorts by `created_at` desc then `run_id` desc.
  - `filterTasks(runs, opts: { query: string; status: StatusFilter; dismissed: ReadonlySet<string>; showDismissed: boolean }): RunView[]`. The query is a case-insensitive substring match on `instruction ?? ""` or `title`. Dismissed runs are hidden unless `showDismissed`. Active runs are never hidden by dismissal.
  - `groupTasks(runs, now: Date): { label: string; runs: RunView[] }[]` makes "Running" first (every non-terminal run), then one group per local calendar day of `created_at`. The day groups are labelled "Today", "Yesterday", otherwise `date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })`. Empty groups are omitted.
  - `runningCount(runs): number`
  - `parseRunHash(hash: string): string | null` accepts only `#run=` followed by a lowercase hyphenated UUID.
  - `tasksPageUrl(runId: string | null): string` gives `chrome.runtime.getURL("tasks.html")` plus `#run=<id>` when set. It takes `getURL` as a parameter for tests.

- [ ] **Step 1: Write failing tests in `task-history.test.mjs`.**
  - `listAllTasks walks pages with the last view as cursor`: a fake host answers 20+20+5 and the second and third requests carry the right `before`.
  - `listAllTasks stops when the cursor does not advance`
  - `listAllTasks treats a missing more as the last page`
  - `mergeRuns keeps one view per run, the fresher state`: the same id running in `known` and succeeded in `fresh` gives one succeeded view; the reverse also keeps the succeeded one.
  - `filterTasks matches instruction and title case-insensitively`
  - `filterTasks running includes queued`
  - `filterTasks hides dismissed finished runs but never active ones`
  - `groupTasks puts active runs under Running, then Today and Yesterday`: build the dates from a local `now`.
  - `parseRunHash accepts only #run=<uuid>`: this covers `#run=ABC`, `#run=`, `#x=<uuid>` and a valid one.

  Add `rememberDismissed keeps the newest 1000` to `run-card.test.mjs`.
- [ ] **Step 2:** Run `npm run build:extension && node --test extension/tests/task-history.test.mjs extension/tests/run-card.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement. Add the build entry and the shape check change.
- [ ] **Step 4:** Run the same command. Expected: PASS. Then run `npm run lint`.
- [ ] **Step 5:** Commit with `feat(extension): task history list logic`.

### Task 4: Extension — `TaskDetail` view

**Files:**
- Modify: `extension/run-card.ts`. Add `export class TaskDetail extends RunWatcher` beside `TaskCard`. Task 6 deletes `TaskCard`, the `"task"` `CardKind` branch of `RunCards`, and `reuse`/`onTerminal` in `RunCardDeps`, once the panel stops using them. `restorable` keeps its `kind` parameter.
- Test: `extension/tests/run-card.test.mjs`.

**Interfaces:**
- Consumes: `RunWatcher` (polling, `cancelButtons()`, `resumeButtons()`), `elapsedText`, `stateText`, `rememberDismissed` (Task 3).
- Produces:
  - `interface TaskDetailDeps extends RunProgressDeps { storage: ChromeStorageArea; reuse(instruction: string): void; openPage(url: string): void; dismissed(runId: string): void; changed(run: RunView): void }`
  - `new TaskDetail(view: RunView, deps: TaskDetailDeps)`, with `.element: HTMLElement`, `.start()`, `.dispose()`, `.run: RunView`. `changed` is called on every new view, so the page updates its list row.
- Elements (classes, in order):
  - `.task-detail-instruction` (h2, full instruction)
  - `.task-detail-meta` ("on <title> · Codex|Claude Code · started <local HH:MM> · <stateText> · <elapsedText>")
  - `.run-card-actions`
  - `.run-card-problem`
  - `.task-detail-note` (the failed/canceled note, only after `started_at` is set)
  - `.task-detail-report` (labelled "Report", `white-space: pre-wrap`, hidden until there is a report)
  - `<details class="task-detail-output"><summary>Agent output</summary><pre class="run-card-stream"></pre></details>`. It starts open while the run is unfinished, and closes once when the run turns terminal. The user's later toggles are kept.
- Actions, per the spec table:
  - Cancel (active);
  - Copy report (done) or Copy (failed/canceled: the error or the stream);
  - Reuse (terminal, when `instruction` is present, with the full `get_run` instruction);
  - Open in Codex or Copy resume command;
  - **Open page**, which calls `openPage(run.url)`;
  - Dismiss (terminal), which calls `rememberDismissed` and then `dismissed(runId)`.

- [ ] **Step 1: Write failing tests (fake DOM as `run-card.test.mjs` already uses).**
  - `TaskDetail shows Running with live output open, then Done with the report and output closed`
  - `TaskDetail renders the instruction, title, report and stream as text only`: the report is `<img src=x onerror=alert(1)> [x](javascript:alert(1))`. Assert no child element is created inside `.task-detail-report` and the text is exact.
  - `TaskDetail Reuse passes the full get_run instruction, not the list cut`
  - `TaskDetail Open page opens the run url`
  - `TaskDetail Dismiss remembers the run and reports it`
  - `TaskDetail keeps the output toggle the user chose after finishing`
- [ ] **Step 2:** Run `npm run build:extension && node --test extension/tests/run-card.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement `TaskDetail`, sharing action-button code with `TaskCard` through `RunWatcher` where it is the same. The panel is untouched in this task.
- [ ] **Step 4:** Run `node --test extension/tests/run-card.test.mjs extension/tests/panel/run-cards.test.mjs` and `npm run lint`. Expected: PASS.
- [ ] **Step 5:** Commit with `feat(extension): TaskDetail view for the tasks page`.

### Task 5: Extension — the Tasks page

**Files:**
- Create: `extension/tasks.html`, `extension/tasks.ts`
- Modify: `extension/panel.css` (a `body.tasks-page` block), `extension/build.mjs` (copy `tasks.html`, add `tasks.js`), `package.json` `test:extension`
- Test: `extension/tests/panel/tasks-page.test.mjs`. If the harness needs another document, generalize its DOM builder to take the HTML, and export `TASKS_HTML` beside `PANEL_HTML`.

**Interfaces:**
- Consumes: Task 3 (`listAllTasks`, `listTaskPage`, `mergeRuns`, `filterTasks`, `groupTasks`, `parseRunHash`, `dismissedIds`, `runsMayExist`), Task 4 (`TaskDetail`), Task 7's `sendReuse` (wire it in Task 7; in this task Reuse calls a stub `reuse()` exported from `tasks.ts` deps), `ConfigSession.readHostConfig()` from `settings.ts` for `agent_status`, `serviceSetupText` from `agent.ts`.
- Markup ids:
  - `#tasks-status` (a line, plus `#tasks-status-settings` "Open settings")
  - `#tasks-search` (input, placeholder "Search tasks…")
  - `#tasks-filter` (select: All, Running, Done, Failed, Canceled)
  - `#tasks-show-dismissed` (checkbox "Show dismissed", off)
  - `#tasks-list` (`role="listbox"`, `li.task-row[data-run-id][role=option][aria-selected]` under `h3.task-group` labels)
  - `#tasks-detail`
  - `#tasks-empty`
- Layout: a two-column grid. `#tasks-list` is 38% wide with its own scroll, and `#tasks-detail` takes the rest. At `max-width: 720px` it stacks. A row shows the instruction (two-line clamp), a state pill (`.task-pill.running|done|failed`) and the local start time.

Behaviour:
- **Open.** On Windows, show the Windows text and nothing else. Read config. If no harness is ready and `runsMayExist` is false, show the agent setup text (`TASK_NEEDS_HARNESS_MESSAGE`) with Open settings and stop. Otherwise call `listAllTasks`, then render. Select the hash's run if it is in the list, otherwise the newest. Write `#run=<id>` with `history.replaceState`.
- **Errors.** `service_unavailable` gives `serviceSetupText` with Open settings. `invalid_request` with "invalid or unsupported request JSON" gives the older-host text. Anything else gives "Could not list tasks: …".
- **Refresh.** While visible and any listed run is active, call `listTaskPage(null)` every 2000 ms and `mergeRuns` it. On `visibilitychange` to visible, refresh once immediately. On `hashchange`, select the hash's run. Selecting a row disposes the old `TaskDetail` and starts a new one. `TaskDetail.changed` updates that row.
- **Keyboard.** ArrowUp and ArrowDown on the list move the selection. Click or Enter selects.

- [ ] **Step 1: Write failing tests.**
  - `the page lists every page of tasks and selects the newest`
  - `#run=<id> selects that task`
  - `an unknown #run= selects the newest task`
  - `the page does not list runs without a harness or a run record`: assert no `list_runs` call and the setup text shows.
  - `the page shows the service setup text with Open settings when the service is unavailable`
  - `an older host gets the rebuild text`
  - `search and status filter narrow the list; Running includes queued`
  - `the page stops polling while hidden and refreshes on show`
  - `a task finishing updates its row pill`
  - `Dismiss removes the row unless Show dismissed is on`
  - `arrow keys move the selection`
- [ ] **Step 2:** Run `npm run build:extension && node --test extension/tests/panel/tasks-page.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement the page, its styles and its build entries.
- [ ] **Step 4:** Run the same test plus `npm run lint`. Expected: PASS.
- [ ] **Step 5:** Commit with `feat(extension): tasks page with history and live detail`.

### Task 6: Panel — compact task bar, Tasks button, tracker

**Files:**
- Create: `extension/task-tracker.ts`
- Modify:
  - `extension/panel.html`: add `<button id="task-open" type="button" class="secondary compact">Tasks ↗</button>` beside `#task-run`. Keep `#task-run-area` for lines and notices only.
  - `extension/panel.ts`: remove `taskCards`, `announceTask`'s card wiring and `reuseInstruction`'s card source. `restoreCards()` now restores summaries only.
  - `extension/task.ts`: `TASK_MAY_HAVE_STARTED_MESSAGE` gets the new copy.
  - `extension/run-card.ts`: `restoreRunCards` sends `kind: "summary"`; delete `TaskCard`, the `"task"` `CardKind` branch of `RunCards`, and `reuse`/`onTerminal` in `RunCardDeps`; move the task-card tests in `tests/panel/run-cards.test.mjs` to `TaskDetail` tests or delete them where `tasks-page.test.mjs` covers them.
  - `extension/build.mjs`
- Test: `extension/tests/panel/task-bar.test.mjs` (rework the card assertions), `extension/tests/panel/run-cards.test.mjs`, `extension/tests/task.test.mjs`

**Interfaces:**
- Produces:
  - `class TaskTracker { constructor(deps: { host: Pick<HostClient,"call">; now(): number; visible(): boolean; changed(state: { running: number; newestId: string | null }): void; finished(run: RunView): void }); refresh(): Promise<void>; dispose(): void }`
  - `refresh` sends one `listTaskPage(null)`. A run the tracker saw active that is now terminal in the page gets `finished(run)`. If it is missing from the page, the tracker calls `get_run` with `since_offset: run.stream_bytes` and reports that view. While any task is active and the panel is visible, it reschedules itself at 2000 ms.
  - `newestId` is the newest task in the page.
- Panel wiring:
  - The tracker starts where `restoreCards` runs today, under the same harness / `runsMayExist` gate.
  - It refreshes after a successful start, after an uncertain start, and on `visibilitychange` to visible.
  - `changed` sets the button text to "Tasks ↗" or "Tasks · N running ↗".
  - `finished` feeds the existing `#task-announce` logic ("Task done|failed|canceled: <first 60>").
  - **Tasks ↗** calls `openTasksPage(lastStartedRunId ?? newestId)` (Task 7; until Task 7 lands, call `chrome.tabs.create({ url: tasksPageUrl(id) })`).

- [ ] **Step 1: Write failing tests.**
  - `the task bar shows only the field, Run and Tasks`: after a start, no `.run-card` exists in `#task-run-area`.
  - `Tasks reads "Tasks · 1 running ↗" while a task runs and "Tasks ↗" after`
  - `a task finishing is announced in #task-announce`, kept from today's test.
  - `a finished task missing from the list is fetched once with get_run`
  - `a lost start answer says check Tasks and refreshes the tracker`
  - `summary restore asks list_runs for kind summary`
  - `the panel does not list task runs without a harness or a run record`
- [ ] **Step 2:** Run `npm run build:extension && node --test extension/tests/panel/task-bar.test.mjs extension/tests/panel/run-cards.test.mjs extension/tests/task.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Run the same command, the whole `npm run test:extension`, and `npm run lint`. Expected: PASS.
- [ ] **Step 5:** Commit with `feat(extension): compact task bar with Tasks button`.

### Task 7: Opening the page and Reuse hand-off

**Files:**
- Create: `extension/task-handoff.ts`
- Modify:
  - `extension/chrome.d.ts`: `runtime.getContexts`, `tabs.create`, `tabs.update`, `windows.update`, `windows.getCurrent`, `storage.session`, `storage.onChanged` and `sidePanel.open`, whichever are missing.
  - `extension/panel.ts`, `extension/tasks.ts`, `extension/build.mjs`
- Test: `extension/tests/task-handoff.test.mjs`, plus additions to `tasks-page.test.mjs` and `task-bar.test.mjs`

**Interfaces:**
- Produces:
  - `REUSE_KEY = storageKey("task_reuse_v1")`, kept in `chrome.storage.session` as `{ instruction: string; window_id: number | null; at: number }`.
  - `sendReuse(storage, instruction: string, windowId: number | null): Promise<void>`
  - `takeReuse(storage, windowId: number): Promise<string | null>`. It takes the value when `window_id` is null or equal, removes the key, and returns the instruction. Otherwise it returns null.
  - `openTasksPage(chromeApi, runId: string | null): Promise<void>`. It looks for a `TAB` context whose `documentUrl` starts with `getURL("tasks.html")`. If one exists, it calls `tabs.update(tabId, { active: true, url: tasksPageUrl(runId) })` and `windows.update(windowId, { focused: true })`. Otherwise it calls `tabs.create({ url })`.
- **Reuse on the page.** In the click handler, before any `await`, call `chrome.sidePanel.open({ windowId })` for the page's window. If it resolves, `sendReuse(..., windowId)`. If it rejects, `sendReuse(..., null)`. Then show "Instruction sent to the task bar."
- **The panel.** On load and on `storage.onChanged` for `REUSE_KEY` (area `session`), call `takeReuse(chrome.storage.session, myWindowId)`. If it returns text and no start is in flight, set `#task-input` to it and focus the field.

- [ ] **Step 1: Write failing tests.**
  - `takeReuse takes a value for its window or for any window, once`
  - `takeReuse leaves another window's value`
  - `openTasksPage focuses an open Tasks tab with the run hash`
  - `openTasksPage opens a new tab when none is open`
  - `Reuse on the page opens the side panel and fills its task bar`, end to end with the two fakes sharing a fake `storage.session`
  - `the bar is never filled from a reuse value while a start is in flight`
- [ ] **Step 2:** Run `npm run build:extension && node --test extension/tests/task-handoff.test.mjs extension/tests/panel/tasks-page.test.mjs extension/tests/panel/task-bar.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement and wire it up, replacing the Task 5 and Task 6 stubs.
- [ ] **Step 4:** Run the same command plus `npm run lint`. Expected: PASS.
- [ ] **Step 5:** Commit with `feat(extension): open the tasks page and hand Reuse to the task bar`.

### Task 8: Headless smoke and docs

**Files:**
- Modify: `scripts/smoke-macos.mjs`. In `taskFromPanel`, read the task's state, report, headline and buttons from the Tasks page (open `tasks.html#run=<id>` the way `onPanel` opens `panel.html`, and add an `onTasksPage` helper beside it) instead of `#task-run-area .run-card`. Assert the panel's `#task-open` text goes from "Tasks · 1 running ↗" to "Tasks ↗", and that `#task-announce` still announces. Keep `checkDiagnostics` canaries.
- Modify:
  - `DESIGN.md`: §5.7 panel ASCII and the task-bar paragraph; replace the task-card paragraph and table with a "Tasks page (as built)" paragraph; the §12.2 step 11 "Next" line drops "task history"; the §4.5 retention sentence stays.
  - `TESTING.md`: real-UI acceptance for the page.
  - `AGENTS.md`: the `extension/` file list gains `task-history.ts`, `task-tracker.ts`, `task-handoff.ts` and `tasks.ts`; the panel test list gains `tasks-page`.
  - The AMOS inventory entry for Brauser under `/Users/jrauser/ENDURANCE/AMOS/inventory/`, if one exists: read its instructions and schema first, then validate.

- [ ] **Step 1:** Update the smoke script. Run `npm run smoke:macos:headless`. Expected: PASS.
- [ ] **Step 2:** Update the docs. Run `npm run lint` and the docs checks the README gate lists.
- [ ] **Step 3:** Commit with `test(smoke): task flow through the tasks page; docs: tasks page`.

### Task 9: Gate, review, merge, deploy (orchestrator)

- [ ] Run the full README.md development gate. Expected: green.
- [ ] Run `codex exec review --base origin/main` once. Fix P1s test-first. File P2s and P3s as `brwr` beads.
- [ ] Push `feat/tasks-page`, open the PR, and wait for CI to pass (`Build and check`). Merge it yourself.
- [ ] Deploy: rebuild the extension and host where Chrome loads them (`work/rauserBrowser/extension/dist`, and the native host manifest's build). The host and extension go together. First check that the main checkout's stale `.git/index.lock` belongs to no live process and that no peer session is using that checkout. Then ask John to reload the extension at chrome://extensions.
