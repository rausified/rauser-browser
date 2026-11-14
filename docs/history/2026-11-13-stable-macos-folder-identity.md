## 2026-11-13 — Stable macOS selected-folder identity

DESIGN.md §4.4 now specifies a persistent filesystem volume UUID plus directory ID for new macOS folder selections. A live diagnosis found an accessible internal-disk folder with an unchanged inode but a different device number, which the earlier device-and-inode check treated as a changed folder (PR #461; `brwr-m78n`).

The native reader binds the UUID observation to Brauser's already-open directory. It does not cache UUIDs by device number. Windows and strict fallback identities retain their existing behavior. Home, knowledge-base and configured task folders share the verifier.

Legacy selections are accepted only while their entire device-and-inode value still matches. A native reselection upgrades a folder once. Already-stale legacy selections cannot be safely inferred from inode alone, so the picker boundary remains. Capture intents and relocation records are not rewritten; mixed identity formats are compatible only while both verify against the same opened directory. Disposable index state can rebuild after a representation change.
