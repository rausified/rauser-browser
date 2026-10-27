# Testing Strategy

How to test this browser extension and native host without disturbing the person running the tests. Read it before running or writing tests here.

This document is self-contained and travels with the repository. It assumes nothing beyond what a contributor gets from cloning this repo and the tools named in [README.md](README.md#development) (DESIGN.md §2 principle 9: Brauser is a standalone product).

## The one hard rule

**Tests must not use the user's screen, keyboard focus, clipboard, or everyday browser profile.** Headed browser windows, native dialogs, UI scripting (osascript, System Events, Accessibility), and keystroke injection all take over the machine while someone works.

When you can't test something without UI, don't fall back to the user's screen on your own. Ask first (see "Real-UI acceptance" below), or say clearly that it was not tested.

## Test layers

Put each check in the lowest layer that can prove it.

1. **Unit tests** for logic: policy, parsing, validation, and state machines.
2. **Integration tests** across a real process or protocol boundary, such as a native host spawned by a test, or an HTTP server with real requests. No UI.
3. **UI-free end-to-end run**, the default for routine checks and CI. A real browser runs headless in a temporary profile and is driven over the DevTools protocol. Native dialogs are replaced by test-only seams, and browser permission prompts are handled by granting access ahead of time.
4. **Real-UI acceptance**, used only occasionally. It runs the same flow against the real dialogs and prompts on a desktop, only when the user agrees. Run it before accepting changes to dialogs, permission prompts, or visual behavior.

## UI-free end-to-end: the recipe

### Browser

- Launch Chrome with `--headless=new --user-data-dir=<temp dir> --no-first-run --no-default-browser-check --remote-debugging-pipe`.
- The pipe opens no network port. Use a temporary profile and never the user's.
- Headless Chrome accepts **only one start URL**. Open any others with `Target.createTarget`.
- Extensions: add `--enable-unsafe-extension-debugging` and call `Extensions.loadUnpacked`. In headless Chrome 154 the service worker runs, native messaging works, the real side panel opens, and `webNavigation` events fire.
- Chrome 154.0.8037.57 ignores `--load-extension`, even with `--disable-features=DisableLoadExtensionCommandLineSwitch`, and `Extensions.loadUnpacked` lasts one browser session. A relaunch therefore reloads the extension after startup (reason `install`), which also clears its `storage.session`. Behavior that depends on an extension installed before browser startup cannot be tested this way.
- Chrome for Testing (`npx @puppeteer/browsers install chrome@<version> --path ~/.cache/puppeteer`, a build matching branded Chrome's version) still honors `--load-extension`, so the extension is loaded at startup. It is still not an installed extension: Chrome installs it afresh at every launch (`runtime.onInstalled` reason `install`, no `runtime.onStartup`), and a CDP drag of the folder onto `chrome://extensions` does not install it either.
- Launch any Chrome build with `--use-mock-keychain`. An ad hoc signed build such as Chrome for Testing otherwise waits on the macOS login keychain for its cookie store, so every page load hangs at `about:blank` (and a keychain prompt may appear on screen). It also keeps branded Chrome out of the user's keychain.
- To test what an extension does when events arrive while it is loaded and its worker is stopped, park the fixture's requests (a hold gate in the test server), stop the worker with `ServiceWorker.stopAllWorkers`, then release. Brauser's `restore-order` probe does this for restored tabs.
- Put the native-messaging host manifest in `<profile>/NativeMessagingHosts/` so the normal registration is untouched.
- Drive pages with real input events (`Input.dispatchMouseEvent`) so the browser treats them as user gestures. Use `Runtime.evaluate` with `userGesture: true` for APIs that need a gesture, such as `chrome.sidePanel.open`.
- Assert on real state (files written, host config, `chrome.permissions.contains`, page text) rather than on the fact that a click happened.

### Native dialogs (file pickers, confirmations)

Add a **test-only seam at the process boundary**:

- Replace only the code that draws the dialog. Keep the code that decides what the answer authorizes (tokens, grants, validation) real, so a scripted answer has exactly the authority of a person's click.
- Gate the seam at compile time (a cargo feature or build flag) and make it **impossible in release builds** (for example, `compile_error!` when the feature is on without `debug_assertions`). Build it into a separate output directory so the everyday dev binary never contains it.
- Have the binary report the variant (for example in `--version`). The test runner should refuse to run headless without the seam, and refuse to use a seam build outside the headless run.
- Point the seam at a private absolute directory, never a symlink or group- or world-writable path, placed outside anything the code under test can write to.
- Fail closed: a missing, mismatched, or malformed answer is an error, as a crashed dialog would be.
- Record each dialog's kind and text to a transcript so the test can check what the user would have been asked, not just the answer.

Reference implementation: `host/src/dialog.rs` (`scripted` module) and `scripts/smoke-macos.mjs --headless`.

### Browser permission prompts

**Stock Chrome has no way to answer an extension permission prompt without UI.** Findings as of Chrome 154, 2026-09-28:

- There is no command-line switch. The auto-confirm hooks (`ScopedTestDialogAutoConfirm`, `PermissionsRequestFunction` dialog actions) are C++ test-only.
- The DevTools protocol can't reach the prompt: it is native browser UI, not a DevTools target. In headless Chrome it waits forever.
- Editing the profile prefs does not work. Extension settings are signed, and Chrome discards edited entries (it records `tracked_preferences_reset`).

What works is granting access ahead of time through Chrome's own mechanisms, after which `chrome.permissions.request` resolves without a prompt:

- **Site/host access:** run `chrome.developerPrivate.addHostPermission(extensionId, pattern)` in a `chrome://extensions` tab. This is what the Site access control does.
- **API permissions (for example `webNavigation`):** load the extension once with the permission listed as required, then reload it with the real manifest. Chrome keeps the permission active, and `permissions.remove` still revokes it.
- Chrome skips its prompt for permissions it recorded as granted, even after `permissions.remove`. Expect silent re-grants and write assertions accordingly.

For **ordinary websites** (not extensions), DevTools `Browser.grantPermissions` covers web permissions such as notifications, geolocation, and clipboard. *This has not yet been exercised in this repo; confirm it and update this line.*

### External programs the code under test runs (AI harnesses, CLIs)

Never let a UI-free run find or start the real program, and never make a real model call. Pin what the code under test can see:

- The browser passes its own environment to the native host, so a host started by the test browser inherits the runner's `PATH`, credentials, and cloud settings. Start the host through a wrapper that clears the environment (`exec /usr/bin/env -i HOME=<temp home> PATH=<fake-bin>:/usr/bin:/bin ... <host> "$@"`) and sets only what the run needs, including a fake credential and a name the code must not pass on.
- Put fake programs in a private directory (0700, owned by the user) as symlinks into a separate install directory, so the test can swap a passing fake for a failing one with one atomic rename. The fakes use only shell builtins and absolute paths, and log their argv and environment variable names (never values) to a file the test checks.
- Remember any search locations the code adds beyond `PATH` (Brauser adds `$HOME/.local/bin`); a temporary `HOME` keeps those empty.
- Before clicking anything that runs a program, check that every offered program's resolved path is inside the fake install directory, and stop the run if not.

Reference: `scripts/smoke-macos.mjs` (`installFakeHarnesses`, `onlyFakeHarnesses`).

### Prove the run stayed invisible

Don't just assume headless means invisible; check it. List on-screen windows (`CGWindowListCopyWindowInfo`, which includes menu-bar items) and Dock apps (`NSRunningApplication.activationPolicy == .regular`; an accessory app with no window shows nothing, and the browser's notification helper is one) for the browser's process group and any helper processes. Assert zero at launch, while each scripted dialog is open, and at the end. Both calls are read-only and need no Accessibility access. On failure, report which process was counted, not just the totals. Reference: `scripts/smoke-macos-windows.swift`.

## What the UI-free run cannot prove

- The real native dialogs appear, look right, and return the user's choice.
- The browser's real permission prompt appears, and a first-time grant works.
- Behavior that happens only after a genuine first grant (for example, the extension restart).
- Browser-startup ordering for an installed extension: whether Chrome loads it, with its listeners, before restored tabs commit, and `runtime.onStartup` (see the `--load-extension` and Chrome for Testing notes above). A loaded extension with a stopped worker does get restored tabs' commits; one loaded during startup misses the active tab's.
- Real idle and screen-lock transitions: the headless run has no way to drive them, and `chrome.idle.queryState` reported `active` throughout.
- Visual or OS-level effects such as focus, cursors, and window lifetime (for example, the macOS busy cursor, or whether the real confirmation closes when the host goes away or it times out).

Cover these in real-UI acceptance, and don't claim them from a headless pass.

## Real-UI acceptance

Real-UI checks run on a desktop, so:

- Run one only when the user asks or agrees, and warn them just before anything appears or takes focus.
- Keep it short, and tell the user not to click unless the check needs a person.
- Design the check to run unattended where possible: have the code under test close the UI itself (a timeout or cancel path) and assert on what is on screen, so no one needs to click and nothing needs Accessibility access.
- Make the script refuse to run without an explicit flag (`--on-desktop` or `--auto`), so it can't be run by accident.

Checks of this kind in this repo:

- `npm run smoke:macos -- --auto`: the full flow with the real picker, alert, and Chrome prompt, answered through UI scripting. It takes keyboard focus.
- `npm run check:macos-alert -- --on-desktop` (`scripts/check-macos-alert.mjs`): shows the real macOS confirmation twice and checks, by counting `UserNotificationCenter`'s on-screen windows, that the alert closes with no click when the dialog child's stdin ends and when it times out.

## Changing this document

This standard is meant to improve with use. When you change it:

- Update the relevant section directly; don't just append contradictions. If an approach stops working (for example, a Chrome update breaks a workaround), replace it and note what broke.
- Mark untested claims in *italics* as untested until someone runs them.
- Add a dated line to the learning log below with what you learned.
- If a change alters the hard rule, confirm it with the user first.

## Learning log

- 2026-10-27: Independent review reproduced an older URL note disappearing
  from a recovered context-only route when the shared note's creation route
  had no delivered activity. Older-note discovery now uses the shared note's
  ownership-checked creation URL as well as log evidence. The failing-first
  native regression covers adapter removal, rule narrowing, site removal,
  cache deletion/reindex, read-only viewing and unchanged note revisions.
  No browser or native dialog is involved. Full workspace and scripted-dialogs
  suites pass, as do extension checks. The Node differential runner stalled in
  this sandbox; the same 5,000 generated requests, with its U+2028/U+2029
  escaping and generator-health checks, passed through separate bounded Python
  subprocess calls to the Rust and TypeScript engines (2,355 matches, 426
  invalid documents). macOS smoke cannot run on Linux, and offline fuzz
  compilation lacks the cached `libfuzzer-sys` dependency.
- 2026-10-27: The note-removal matrix uses framed native requests with a fresh
  host store per request for save/load, adapter and site removal, older-note
  viewing, migration refusal and capture suppression. Rule narrowing replaces
  only the confirmation dialog through `confirm_config_with`; real consent
  tokens, revision checks and the update handler still run. It covers edit/view
  routes, mode-only evidence, context without measured activity, notes without
  delivered history, occupied URL notes, ambiguous mappings, stale keys and
  versions, deleted caches and reindex. Separate file-count, per-file byte,
  aggregate-byte and non-UTF-8 fixtures prove incomplete primary scans refuse
  loads and autosaves without changing notes; optional older-note scans stay
  best effort. Three regressions failed before their fixes (initial fixture
  errors used invalid adapter syntax and a future timestamp and were corrected
  first). The existing unavailable-history test now exercises both activity
  and `why` evidence hiding a conflicting identity. No real browser or native
  dialog is involved; the macOS smoke expectation stays 14 dialogs.
- 2026-10-27: Independent review reproduced a wrong-note load and overwrite
  when log catch-up failed and note recovery silently ignored the missing
  historical candidates. The regression creates two artifacts sharing a
  stripped canonical URL, with the second note on an alternate route, removes
  the adapter, and injects both an oversized log and a SQLite publication
  failure. Loads and saves now report unavailable identity, both note files
  remain byte-exact, and clearing either fault restores ambiguity handling
  and the stale-key refusal. No browser or real dialog is involved.
- 2026-10-27: Index catch-up is tested through the native handlers by restoring
  valid older cache files after a durable append, reopening configuration, then
  querying or delivering another single/batched event without replaying the
  missing one. Assertions cover Recent, worklog, and the actual SQLite rows.
  Separate faults replace index.db with a directory or add a sparse oversized
  log: capture stays acknowledged, all three history APIs return unavailable,
  and clearing the fault restores history without duplicate log bytes. Index
  tests cover checkpoint migration, changed-file-only reads, a completed
  trailing marker, dirty tracking, and the aggregate read cap without partial
  publication. These are local filesystem fixtures; no macOS browser run is
  claimed and the scripted-dialog count remains 14.

- 2026-10-27: The single/batch activity parity test covers 13 scenarios through
  both native handlers with separate stores, explicit expected outcomes and
  reasons, correlated event IDs, no log/index writes on refusal, and decoded
  durable payloads for accepted activity (including mode, trail and title-ref
  evidence). It passes before and after consolidating authorization: duplication
  was confirmed, but no existing behavioral divergence was reproduced. Initial
  test-only failures assumed markers contained plain event IDs or struct field
  order; decoding their hex JSON checks the actual contract. No macOS run is
  claimed; the scripted-dialog count stays 14.
- 2026-10-27: Independent review reproduced a sanitizer-upgrade recovery failure with a pre-upgrade v1 intent and a log torn inside a Unicode format character. Exact-hash compatibility now resumes that original entry through both single and batch delivery, preserves unrelated bytes, refuses changed payloads and edited tails, and deduplicates a complete marker. A second regression covers activity and context payloads. Future renderer-independent single-entry intent storage remains tracked as `brwr-3cc`; no macOS execution is claimed.
- 2026-10-27: Unicode-spoofing regressions first failed in daily rows and
  generated artifact files, then passed with the shared host presentation
  sanitizer. Tests exercise single and batch activity/legacy delivery, explicit
  context, exact encoded marker payloads, reindexing into owned files, unchanged
  source logs, plain worklogs, character caps, and readable non-ASCII text.
  The format-character table was exhaustively compared with the local Node
  Unicode 17 `General_Category=Cf` property (170 scalars); no dependency or
  network fetch is needed. These are host filesystem tests, not evidence of
  an Obsidian/VS Code preview or a macOS Chrome run. The smoke runner still
  expects 14 scripted dialogs.
- 2026-10-27: Trail-transition validation uses a strict allowlist for new activity and the original bounded shape checks for historical markers. The regressions test terminal refusal through both native delivery paths, all browser values plus `forward_back`, and a pre-validation ingestion cache followed by reindexing an owned marker. They verify actual SQLite edge kinds, retained activity totals and exact payloads, unchanged source logs, and continued rejection of oversized or unpaired legacy trails. No browser or native dialog is involved.
- 2026-09-28: First version, branch `headless-smoke-seam`. The headless Chrome 154 end-to-end run passed twice with no windows or Dock presence. Found that the extension permission prompt can't be answered without UI; added the pre-grant techniques and the scripted-dialog seam pattern.
- 2026-09-28: Branch `fix/whole-repo-review`. Chrome 154.0.8037.57, even headless, starts `Google Chrome Helper (Alerts)` (`com.google.Chrome.framework.AlertNotificationService`), an accessory-policy app with no windows, and the invisibility check counted it as visible, which stopped every run after `hello`. The check now counts only regular (Dock) apps plus on-screen windows, and names each process it counts.
- 2026-09-28: Branch `m2-harness-setup`. Chrome passes the runner's environment to the native host, so AI harness setup would have seen the real `PATH` and credentials. The headless wrapper now clears the environment with `env -i` and pins `PATH` to fake harnesses; added the external-programs section. The headless run with fake harnesses passed with nine scripted dialogs.
- 2026-09-28: Branch `fix/whole-repo-review`. On macOS, rfd's Yes/No alert is drawn by `UserNotificationCenter`, not the dialog process, so killing the process leaves the alert on screen. The fix uses `CFUserNotificationCreate` with a timeout, plus cancel. Added `scripts/check-macos-alert.mjs` as the first check queued for the VM, and a rule to design real-UI checks so they close their own UI.
- 2026-09-28: The user decided against a macOS VM or a second login for real-UI testing: too much setup. Real-UI acceptance now runs on a desktop only when the user agrees, behind an explicit flag, and designed to close its own UI. Removed the VM plan and the VM guard in `check-macos-alert.mjs`.
- 2026-09-28: Branch `m15a-probes`. Added `npm run probe:m15a`, headless Chrome API probes for M1.5a. Found that Chrome 154 ignores `--load-extension`, so startup ordering for an installed extension is untestable headless, and that the headless run cannot drive `chrome.idle` out of `active`. Added both to the cannot-prove list.
- 2026-10-16: Adapter-only capture made an enabled site without an adapter intentionally inert. Until the adapter editor ships, the smoke runner now adds its fixture adapter through the real `confirm_config` / `update_config` native-message exchange from the extension page, answers and inspects that separate confirmation, reloads settings to publish the resulting lease, and then proves that the worker and host retain only the recognized `/allowed/...` navigation.
- 2026-09-28: Branch `m15a-restore-order`. Chrome for Testing 154 honors `--load-extension` but reinstalls the extension at every launch, so it still cannot stand in for an installed extension at startup. It hung on every page load until launched with `--use-mock-keychain`, which the probe runner now always passes. The `restore-order` probe found that a loaded extension with a stopped worker gets restored tabs' commits, and one loaded during startup misses the active tab's. Updated the browser recipe and the cannot-prove list.
- 2026-10-16: Branch `feat/overnight-cookie-loop-20261016`. A Linux run with umask `0002` found that fake harness directories created with inherited modes were correctly rejected as unsafe, obscuring the behavior each process test meant to exercise; fixtures now set their secure modes explicitly and still test unsafe modes deliberately. The same run found that `cap-std` directory capabilities may be Linux `O_PATH` handles, so durability tests must sync a readable self-opened directory handle rather than the lookup capability itself. GNU `stat -f` also succeeds with filesystem output instead of rejecting BSD syntax, so cross-platform shell fixtures must select syntax explicitly instead of relying on `||` fallback.
- 2026-10-16: Branch `ship-overnight-20261016`. Overnight cycle 3 added the fixture-adapter confirmation (dialog 14) without updating the runner's dialog count, and nothing caught it: the overnight gate runs on Linux, where the smoke runner refuses to start, and CI did not run it. CI's macOS build job now builds the scripted-dialogs host and runs `npm run smoke:macos:headless`, so a change that breaks the end-to-end run fails its PR. *Not yet confirmed on a GitHub-hosted runner.*
