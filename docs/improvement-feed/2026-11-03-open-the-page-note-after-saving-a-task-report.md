## 2026-11-03 — Open the page note after saving a task report

Follow-up: `brwr-vuvc` (discovered from `brwr-bsx`).

Add an explicit **Open note** action to the successful Save to note confirmation
in Tasks. Resolve the note on the host through the task's retained source URL
and the ordinary physical-note ownership path, using the same final inode and
symlink recheck as `open_summary`; never accept a caller path.

`brwr-bsx` now keeps a report in its source note and preserves existing user
text, but the user must still find that note themselves to read the combined
result. A direct action would complete that verification step without requiring
another task or manual folder browsing. Implement beside `extension/run-card.ts`
and `host/src/native/task_note.rs`, with an injected opener and no automatic
navigation or writes to recovery copies.
