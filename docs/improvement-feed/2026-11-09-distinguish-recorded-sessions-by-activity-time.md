## 2026-11-09 — Distinguish recorded sessions by activity time

Change: Add a recorded activity date/time to each session choice, alongside its
name and stable identity. Keep ordering and selection tied to the recorded ID.

Why: Paging now exposes every recorded session, and regression fixtures include
140 sessions with duplicate names. A name plus UUID suffix distinguishes their
identities but does not help a person find the meeting or work period they recall.

Where: The catalog in `host/src/index/focus.rs`, `FocusSession` catalog metadata
in `protocol/schema.json`, and `renderWorklogSessions` in `extension/panel.ts`.
Use the newest recorded activity, without reading unrelated session contents.
