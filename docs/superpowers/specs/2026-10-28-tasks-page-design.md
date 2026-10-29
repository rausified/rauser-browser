---
title: Tasks page — task history and live view outside the side panel
date: 2026-10-28
status: draft
tags: [brauser, task-bar, design]
---

# Tasks page

## Intent

The side panel's task bar (DESIGN.md §5.6–§5.7, PR #180) shows each task as a card in the panel: instruction, state, a streamed-output box and a report box. In a 400 px panel those two boxes are cramped scroll areas. A finished task disappears from the panel after 24 hours, or after five newer ones finish, so there is no way to look back at earlier tasks.

John wants to **see what is happening and what happened** in a proper page, and to **review the history of all previous tasks**. The panel keeps only the controls for starting a task. A button opens the page.

## Goals

1. **A Tasks page.** It is an extension page (`tasks.html`) opened in a normal browser tab, with layout A (list and detail):
   - **Left: the task list.** It holds every task the service still keeps. Queued and running tasks are pinned at the top under "Running". The rest are grouped by day ("Today", "Yesterday", then dates), newest first. Each row shows the instruction (clamped to two lines), a state pill ("Running · 1m 04s", "Done · 7m 35s", "Failed", "Canceled", "Queued") and the start time. The list has a search box that matches the instruction and the page title, and a status filter (All, Running, Done, Failed, Canceled).
   - **Right: the selected task.** It shows the full instruction as the headline, then the subline "on <page title> · Codex|Claude Code · started <time> · <state> · <elapsed>". The action buttons come next, then the **Report**, then a collapsible **Agent output** section holding the streamed text. While the task runs, the output streams live, the elapsed time ticks, and the section starts expanded. Once it finishes, the report shows and the output section collapses. The note "Changes the agent already made stay; check before running it again." shows for a task that failed or was canceled after it started, as the card shows it today.
   - **Actions** match today's task card (DESIGN.md §5.7 task-state table): Cancel, Copy report or Copy, Reuse, Open in Codex or Copy resume command, and Dismiss. The page adds **Open page**, which opens the task's URL in a new tab.
2. **A compact panel task bar.** The bar keeps the text field, the "Uses this page…" context line, its hint line, its warning line and the `#task-announce` live region. Below them sit just two buttons, **Run** and **Tasks ↗**. Task cards leave the panel entirely, so `#task-run-area` no longer renders cards. While any task is queued or running, the second button reads "Tasks · N running ↗".
3. **Task history from the service.** The page lists every retained task, not just the panel's restore set. Retention keeps every run younger than 30 days plus the newest 200 of any age, whichever is more (`host/src/service/maintenance.rs`). A heavy month can therefore hold more than 200 tasks, more than one 900 KiB frame carries. The list is **paged** (see Constraints), and history must never be silently cut short.

## Non-goals

- **Tracking the artifacts a task created** (files, Confluence pages and so on). John explicitly left this out. The report is the only record of what a task did.
- **Summarize & log and site-draft runs.** They keep their cards in the panel and do not appear on the Tasks page.
- **Starting a task from the page.** A task needs the page it runs on, so starting stays in the panel.
- **Deleting run records.** Retention stays as it is.

## Anti-goals

- Agent text (instruction, report, stream, title) must never be rendered as HTML. The page uses `textContent` only, as `run-card.ts` does (DESIGN.md §5.7, §14).
- The page must not start the on-demand service when no harness is set up. The panel today does not call `list_runs` then, and the page follows the same rule.
- There must be no diagnostics logging of instructions, reports, titles or streams (DESIGN.md §15).
- The page must not poll the host while the tab is hidden.

## Decided behavior

- **Opening the page.** **Tasks ↗** focuses an existing Tasks tab if one is open, matched on the extension's `tasks.html` URL. Otherwise it opens a new tab. The page selects the newest task when it opens, or when a hash names a run id (`tasks.html#run=<id>`) it selects that run. The panel passes the newest run's id once a start succeeds, so Run followed by Tasks ↗ lands on the new task. The panel does not open the page by itself when Run is clicked.
- **Reuse** on the page puts the full instruction (from `get_run`, never the 280-character list cut) back in the panel's task bar. The page and the panel are separate documents, so this needs a hand-off. The builder chooses the mechanism (for example `chrome.storage.session` with a key the panel watches, or `chrome.runtime` messaging). The instruction goes to one panel only: the panel in the window the Tasks tab sits in. If no panel is open in that window, the instruction waits for the next panel opened in any window, and the first panel to take it clears it, so it is never filled twice. The page then shows "Instruction sent to the task bar." and opens the side panel if Chrome allows it from that gesture.
- **Dismiss** hides a task from the page's list. It does not delete the record. It shares the `dismissed_runs_v1` set the panel already uses. That set is capped at `MAX_DISMISSED_RUNS = 200`, newest kept (`extension/run-card.ts`), which was enough for five restored cards but not for a full history. Raise the cap to 1000 (about 40 KB in `chrome.storage.local`), keeping the key and format so the panel's existing reads still work. A "Show dismissed" toggle is the builder's call (my call: include it, off by default).
- **Completion announcement.** The panel's `#task-announce` still says "Task done: …" (or failed, or canceled) when a task it saw unfinished finishes. The panel therefore still polls active tasks (the running count needs this anyway), even though it no longer draws cards.
- **Start errors.** Errors from starting a task still show as the one warning line under the bar for the starting tab, with the same texts and **Open settings** cases (DESIGN.md §5.7). "The task may have started — check below before running it again." becomes "The task may have started — check Tasks before running it again." A failed `list_runs` in the panel shows the existing notice under the bar.
- **The one-time setup warning** (§7.1) still shows in the panel before the first task.

## Constraints

- **Protocol.** `list_runs` today returns at most `LIST_LIMIT` (20) runs of all kinds, newest first, with active runs always included (`host/src/service/supervisor.rs` `list_runs`). The page needs every retained task. DESIGN.md §12.2 step 11 already names the intended change, an optional `ListRunsRequest.kind`, which also stops summaries crowding tasks out of the panel's restore. The protocol change must be additive within version 8, following the rules for `start_task` (protocol/README.md "Agent tasks"), so an older service or host is handled. The shape is a `kind` filter plus paging: a `before` cursor (the last run's `created_at` and `run_id`, matching the list's sort) and a `limit`. The answer says whether more runs remain. Active runs come in the first page. A list view keeps instructions cut to 280 characters and `task_report` empty, and the page size must keep a page well inside the 900 KiB outbound frame (`host/src/frame.rs`). The page fetches every page when it opens, so search and filter cover the whole history. Later refreshes re-fetch only the first page, and merge it by run id. The full instruction, report and stream come from `get_run`, as today.
- **Host and extension deploy together** (protocol/README.md). An older host that rejects the new list field must produce a clear "Rebuild the Brauser host…" message on the page, as the task bar does for `start_task`.
- **Polling.** The page polls `get_run` for the selected task while it is unfinished, at `POLL_MS` (1000 ms) with `since_offset` streaming, and refreshes the list periodically while any task is active. It stops while `document.hidden` is true. Reuse `run-card.ts` helpers (`elapsedText`, `stateText`, `resumeCommand`, the dismissed set) rather than copying them.
- **Platform.** The page is macOS only, like the task bar. On Windows the Tasks button and page show "Tasks are not supported on Windows yet."
- **Styling.** Follow DESIGN.md and `extension/panel.css` (the system font, the `#1557aa` primary and `#5b6470` secondary buttons, `#8885` borders, light and dark through `color-scheme`). Use a full-width layout with the list about 38–40% wide. Below about 720 px the layout stacks.
- **Build.** Add `tasks.html` and `tasks.js` to `extension/build.mjs`, and the page to `manifest.json` only if Chrome requires it (extension pages need no `web_accessible_resources`).
- **Docs.** Update DESIGN.md §5.6–§5.7 (the panel layout ASCII, task bar text, task card section moves to the Tasks page), the §12.2 step 11 "Next" line, TESTING.md real-UI acceptance for the page, the protocol README for the list change, and the AMOS inventory entry for Brauser if one exists.

## Left to the builder

- The exact protocol shape (filter and limit or paging), and how an older service is detected.
- The Reuse hand-off mechanism between the page and the panel.
- Whether the list refresh is a timer or piggybacks on `get_run` of active tasks.
- Empty, loading and error states on the page, following the panel's wording style ("Waiting for the native host…", the service setup text with **Open settings**, "No tasks yet. Start one from the side panel.").
- Keyboard: arrow keys move the list selection, and Enter or click selects.
- Test layout: unit tests in `extension/tests` for list grouping, filtering, selection and hash handling, plus host and service tests for the new `list_runs` filter.

## Calls made by the agent (not John's decisions)

- Clicking Tasks ↗ focuses an existing tab rather than opening a second one, and selects the newest task by default.
- Reuse sends the instruction back to the panel's task bar.
- Dismiss hides a task and does not delete it. It shares the panel's dismissed set.
- Summaries and site drafts are not on the page.
- History is paged, never capped, because John asked for the history of all previous tasks.
- Reuse goes to the panel in the Tasks tab's window, or else to the next panel that opens, and only once.
- The dismissed-run cap rises from 200 to 1000.
- "Running" (in the filter and in the "N running" count) includes queued tasks.
- The running count lives on the Tasks button. This matches the mockup John chose ("Tasks · 1 running ↗").
