## 2026-11-04 — Show composed command budget

Show a saved command's effective instruction length and the room left for an
additional request in Settings. The new command editor validates literal prompts
up to 4096 characters, but `host/src/task_commands.rs` adds a command heading
before the existing task instruction bound is checked. A prompt at its allowed
limit therefore saves but cannot run, even without extra user text.

Use a host-derived budget alongside the fields in
`extension/task-command-settings.ts`, sharing the real composition rule rather
than duplicating headings in another validator. Cover Unicode, long names and
the extra-request heading. Keep explicit Save and Run as separate gestures.

Tracked as `brwr-5wup`, discovered from `brwr-kqt4`.
