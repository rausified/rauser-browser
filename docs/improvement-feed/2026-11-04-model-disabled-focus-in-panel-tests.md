## 2026-11-04 — Model disabled focus in panel tests

Extend the fake DOM in `extension/tests/panel/harness.mjs` to model focus
moving away when an active control is disabled. Its current `disabled` field
and `focus()` flag cannot expose this browser behavior. The independent review
of session sharing found that every session choice disabled its own selector,
interrupting repeated keyboard choices, while the offline suite stayed green.
Keep the real-browser keyboard assertions, and use the improved fake to catch
this class of regression when sandbox restrictions prevent Chromium from running.

- +1 2026-11-05: Cycle 8's external pagination gate failed twice because a new checkbox was focused inside a still-disabled fieldset. A test-scoped inherited-disabled focus mock reproduces the failure offline and verifies the ordering repair; extend the shared fake to cover ancestor fieldsets as well as controls' own disabled state (`brwr-tcd5`).
- +1 2026-11-09: Independent review found that hiding More sessions during a delivery refresh or Reload sessions dropped keyboard focus without restoring it. The regressions must explicitly model focus moving to the body because the fake DOM's `hidden` field does not. Extend the shared focus model to hidden controls too; real-browser assertions now cover both refresh and reload.
