## 2026-11-04 — Model disabled focus in panel tests

Extend the fake DOM in `extension/tests/panel/harness.mjs` to model focus
moving away when an active control is disabled. Its current `disabled` field
and `focus()` flag cannot expose this browser behavior. The independent review
of session sharing found that every session choice disabled its own selector,
interrupting repeated keyboard choices, while the offline suite stayed green.
Keep the real-browser keyboard assertions, and use the improved fake to catch
this class of regression when sandbox restrictions prevent Chromium from running.
