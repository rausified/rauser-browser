## 2026-11-02 — Give the extension a fixed ID before M5

The self-registering app (DESIGN.md §11) writes `allowed_origins` for one
stable extension ID, but the development build has no `key` in
`extension/manifest.json`, so each unpacked load path gets its own ID and
`scripts/register-dev-host.mjs` has to be told it. Change: add a pinned public
`key` to the manifest (matching the future Web Store item's key once it
exists), so development, the release app and `brauser register` share one ID
and the registration code can be tested against it before M5. Where:
`extension/manifest.json`, `scripts/register-dev-host.mjs`, and the
self-registration tests when that code is built.
- Done 2026-11-02: `brwr-q0i4` (cycle commit prepared for driver).
