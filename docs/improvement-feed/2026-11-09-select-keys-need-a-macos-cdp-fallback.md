## 2026-11-09 — Select-key a11y tests need a macOS CDP fallback, not a longer timeout

Change: added `selectByKeyboard(page, target, which)` to
`extension/tests/a11y/fixture.mjs`. On Linux it presses the real
Home/End/ArrowDown/ArrowUp (+Enter) keys the test used before. On macOS
(`process.platform === "darwin"`) it calls Playwright's `selectOption()`
instead, matching the chosen index. `extension/tests/a11y/keyboard.test.mjs`
now calls this helper everywhere it used to drive a `<select>` with raw key
presses (brwr-fb0u).

Why: three `test:a11y` keyboard tests failed only on macOS, each on a
`<select>` (`#worklog-share-scope`, `#worklog-focus-session`,
`#recent-reading`, `#task-command`). Root cause, confirmed empirically with a
standalone Playwright script against both headless and a visible (headful)
real Chrome window on this machine: a synthetic Home/End/ArrowDown/Enter key
event through CDP never changes a closed `<select>`'s value on macOS Chrome
— not even in a visible window — while the identical sequence does change it
on Linux Chrome (which is why CI's Ubuntu Accessibility job passed). macOS
Chrome's `<select>` popup is apparently drawn outside the content layer CDP's
synthetic key events reach, so no `change` ever fires there; raising the
timeout only waits longer for an event that will never come. `selectOption()`
fires the same `input`/`change` events a real selection produces and leaves
the control focused, so it exercises the same app-side change-handling code
either way, without depending on which platform's popup implementation would
have driven it.

Generalization: any future a11y keyboard test that drives a native `<select>`
with raw arrow/Home/End key presses will reproduce this exact macOS-only
failure. Use `selectByKeyboard` for that interaction instead of re-deriving
the platform split per-test.

Where: `extension/tests/a11y/fixture.mjs` (new `selectByKeyboard` export),
`extension/tests/a11y/keyboard.test.mjs` (three tests updated to call it).
- Review 2026-11-12: **Done.** Use Playwright selection on macOS for native select controls; raw CDP keys did not change their value. Entry describes implementation; confirm merge before marking Done.
- Done 2026-11-12: verified on main at 4cd617d; `selectByKeyboard` uses the macOS fallback.
