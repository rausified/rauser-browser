## 2026-11-02 — Separate in-sandbox health checks from the out-of-sandbox gate

Cookie idled for several cycles because the scaffold copies every `--test-cmd`
verbatim into the cycle prompt's health check, including
`BRAUSER_REQUIRE_A11Y_TESTS=1 npm run test:a11y`. That flag exists for the
unsandboxed commit gate and CI, where a skip means a broken setup, but inside
the Codex sandbox it turned a known, unfixable Chromium launch block into a
red suite, and the prompt forbids feature work on a red suite. The launcher now
skips as "blocked by sandbox" there even under the flag. Change: keep that
rule in code for every `BRAUSER_REQUIRE_*` flag, and add a `test:health`
script (or a documented gate-vs-health split in TESTING.md) that names which
commands are gate-only, so a new required-flag suite cannot stall the worker
the same way. Where: `package.json`, TESTING.md "Accessibility checks in a real
browser" and "Local IPC in sandboxes".
- Review 2026-11-12: **Revise.** Separate checks that need unsandboxed Chromium from in-sandbox worker checks, so a known launch block does not consume cycles.
