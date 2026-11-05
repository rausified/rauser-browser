## 2026-11-04 — Edit saved task commands in Settings

Add a Settings editor for the reusable sidebar commands introduced by
`brwr-f410`. A person can now invoke a repeatable workflow from the task bar,
but must edit `config.toml` to write or repair its instruction. That makes the
new capability harder to adopt than the rest of the panel.

Use `host/src/task_commands.rs` as the validation authority and a host-owned,
revision-checked mutation endpoint. Preserve invalid raw entries for review
instead of dropping them, keep unsaved edits on a stale save, and expose only
the supported literal/sidebar subset in `extension/options.ts`. Page text must
never populate instruction fields. Cover create/edit/remove and stale-save
behavior with offline native and settings tests.

Tracked as `brwr-kqt4`.

- Done 2026-11-05: brwr-kqt4 — Settings authoring/removal, revision-bound saves, preserved invalid-value repair and offline regressions; complete local gates pass with explicit sandbox skips.
