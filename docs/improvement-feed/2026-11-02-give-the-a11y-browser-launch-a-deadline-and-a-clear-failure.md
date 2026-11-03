## 2026-11-02 — Give the a11y browser launch a deadline and a clear failure

What: in `scripts/a11y-browser.mjs`, wrap the Playwright `launch` (and the first page open) in a bounded timeout, for example 60 s, that fails the whole `npm run test:a11y` run at once with "Chrome did not start within 60 s (CHROME_PATH=…); is another Chrome using the profile, or is the machine overloaded?", instead of letting `node --test` wait forever.

Why: during the auto-summary build (load average 20–27), `npm run test:a11y` first failed every test with "Promise resolution is still pending but the event loop has already resolved" after a 17.7 s first test. A rerun then hung for over 10 minutes with no Chrome child process and no output after the fixture test, and had to be killed by hand. The same machine passed the headless smoke and every other gate, so the gate's result said nothing about accessibility.

Where: `scripts/a11y-browser.mjs` (the shared launcher), `extension/tests/a11y/*.test.mjs`; note the timeout and its message in TESTING.md "Accessibility checks in a real browser".
