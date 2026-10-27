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
3. **UI-free end-to-end run**, the default for routine checks and CI. A real browser runs headless in a temporary profile and is driven over the DevTools protocol. Native dialogs are replaced by test-only seams, and the extension holds the access it needs from install, so no browser permission prompt appears.
4. **Real-UI acceptance**, used only occasionally. It runs the same flow against the real dialogs and prompts on a desktop, only when the user agrees. Run it before accepting changes to dialogs, permission prompts, or visual behavior.

## Capture Site coverage

`npm run test:extension` builds on the extension build from the development
checks. `extension/tests/capture-site.test.mjs` exercises the existing starter
catalog through the new plan builder: real Jira, SharePoint/OneDrive and GitHub
URL forms, independent PR/issue scopes, current-page misses, all 27 fixed
probes, invalid inputs, full-document limits and preservation of other sites.
`extension/tests/panel.test.mjs` loads the shipped panel markup and JavaScript
with a scripted native client, worker and Chrome APIs. It covers review/save,
policy publication, cancel, tab/SPA changes, revision conflicts, pauses and
removals, missing folders, note edits and copy-out across identity changes,
title refreshes during a held save, page switches during a held post-save
note reload (including saving the next edit to the new page), and a failure
after config commit. It asserts no Chrome permission request or native
confirmation is sent. The worker regression installs policy with pages
already open and verifies that
attention begins immediately, private/non-artifact tabs stay excluded, and no
cold open is invented; existing expired-lease and upgrade-navigation tests
keep real opens intact during background renewal. These checks use no real
accounts, harnesses, network or clipboard.

This proves the composed DOM/client flow with mocks; it does not certify the
starter URLs against live tools or prove Chrome layout and keyboard focus.
Those checks remain the existing human starter-certification and interactive
Chrome gates. The macOS smoke runner's localhost adapter flow is unchanged;
Capture Site adds no scripted dialogs; the runner compares answered dialogs
with its transcript instead of expecting a fixed total. Unknown-site structural
drafting still needs the separate DESIGN §12.2 step 7.6 safety, grouping,
scope and identity tests when implemented.

## Page-pattern review coverage

`extension/tests/site-patterns.test.mjs` covers the shared URL shaper against
the recorded starter vectors and named Confluence, SharePoint, Notion,
Google Docs, encoded-name and mixed-case forms. Privacy assertions check
placeholder classes, query-value removal, literal/length limits, corrupt
stored values and metadata stripping. The link-reader tests serialize the
bundled function into a fresh VM context with a fake top-frame DOM, catching
module dependencies that would break Chrome's `scripting` serialization.
They verify exact URL/origin checks before and after href reads, singleton
literal generalization even for duplicated planted links, same-origin-only
results, the 500-anchor scan cap and the 50-shape result cap.

Worker tests exercise recognized and skipped navigations, incognito/missing
tabs, subframes, SPA events, count/shape caps, expiry, cold worker wake and
maintenance alarms, pending removals, policy suspension, pause and explicit
clear. They assert samples reach neither activity/delivery nor a native host,
and that sample-storage failures leave recognized capture working. Composed
panel tests assert isolated top-frame injection only after a click, read-only
results on unknown sites, cancellation/navigation races, collapse cleanup and
scripting failures. Composed settings tests inspect and clear patterns without
native messages, including during host repair.

These checks use mocked Chrome APIs and DOM objects; the serialized function
has been exercised, but real Chrome injection, disclosure layout and keyboard
focus still need the macOS interactive/headless gates. No scripted dialog or
smoke step was added. The Linux VM cannot run `smoke:macos:headless`; do not
claim that it passed here. Structural drafting and unknown-site enabling are
still unbuilt, so these tests do not certify any machine-drafted rule.

## Dialog reply fixture and restricted runners

`dialog::tests::a_dialog_that_answers_in_time_keeps_its_reply` uses a shell
child that reads the length line and exactly that many body bytes before
printing its reply. It exercises 60 exchanges across empty, short and 64 KiB
UTF-8 bodies. Do not replace the reader with an immediate `printf`: that
closes stdin before the parent finishes writing and races into `EPIPE`. Do
not read until EOF either: the parent keeps stdin open until the child exits.
The fixture uses the same `head -c` tool as the neighboring timeout fixture.

A restricted runner can deny Unix socket creation and connection with EPERM,
even under a temporary directory. On 2026-10-27 this caused 32 host service
test failures, confirmed with an independent Unix-socket bind. The same
runner denied Node `spawnSync`, preventing `test:scripts` from running its
win32 path simulation. Keep these tests enabled; run the full gate outside
that restriction. An explicitly filtered local run can diagnose the remaining
tests, but is not a passing full gate. The dialog repair remains in progress
until that external gate passes.

On the clean `728b200` baseline, a second restricted run reproduced the same
32 failures (447 host tests passed). A Python `AF_UNIX` bind under the
workspace also failed with `PermissionError: [Errno 1] Operation not permitted`,
independently of the service code. Node's restriction can be misleading:
`spawnSync` returned `status: 0` alongside `error.code: "EPERM"` and empty
stdout. The script suite therefore reported `Unexpected end of JSON input`
in its win32 simulation. Inspect the spawn result's `error` as well as its
exit status when diagnosing this runner. An independent child that only
prints JSON reproduced it; no registration or launchd call is needed.

The diagnostic workspace run excluding `service::client::tests::`,
`service::server::tests::` and `native::tests::run_proxy::` passed 493 tests
with 39 host tests filtered out. The scripted-dialog unit group passed 10
tests. Neither result substitutes for the unfiltered gate, and neither
justifies disabling the socket tests or changing production IPC to fit the
runner. Verify the full gate in a runner that permits these local operations.

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

### Native dialogs (the folder picker)

Brauser's only native dialog is the folder picker, used for the home folder and the knowledge base folder. There is no native Yes/No confirmation any more. If a future change adds a native dialog, give it the same seam. Add a **test-only seam at the process boundary**:

- Replace only the code that draws the dialog. Keep the code that decides what the answer authorizes (tokens, grants, validation) real, so a scripted answer has exactly the authority of a person's click.
- Gate the seam at compile time (a cargo feature or build flag) and make it **impossible in release builds** (for example, `compile_error!` when the feature is on without `debug_assertions`). Build it into a separate output directory so the everyday dev binary never contains it.
- Have the binary report the variant (for example in `--version`). The test runner should refuse to run headless without the seam, and refuse to use a seam build outside the headless run.
- Point the seam at a private absolute directory, never a symlink or group- or world-writable path, placed outside anything the code under test can write to.
- Fail closed: a missing, mismatched, or malformed answer is an error, as a crashed dialog would be.
- Record each dialog's kind and text to a transcript so the test can check what the user would have been asked, not just the answer.
- Count dialogs, never hard-code a total: the runner's `dialogsShown` must equal the number of scripted answers it gives, and the final check compares it with the transcript's line count.

Reference implementation: `host/src/dialog.rs` (`scripted` module) and `scripts/smoke-macos.mjs --headless`.

### Browser permission prompts

Brauser asks for nothing at run time. Its manifest requests `http://*/*` and `https://*/*` as install-time `host_permissions`, with `webNavigation` and `scripting` required, so loading the extension grants all of it and the UI-free run needs no grant-ahead step. The panel's site-access banner is covered by unit tests with a fake `chrome.permissions`. *Withdrawing and restoring access in headless Chrome (for example with `chrome.developerPrivate` from a `chrome://extensions` tab) has not been exercised here; confirm it and update this line.*

**Stock Chrome has no way to answer an extension permission prompt without UI**, so keep it that way. Findings as of Chrome 154, 2026-09-28, in case a later change brings a runtime request back:

- There is no command-line switch. The auto-confirm hooks (`ScopedTestDialogAutoConfirm`, `PermissionsRequestFunction` dialog actions) are C++ test-only.
- The DevTools protocol can't reach the prompt: it is native browser UI, not a DevTools target. In headless Chrome it waits forever.
- Editing the profile prefs does not work. Extension settings are signed, and Chrome discards edited entries (it records `tracked_preferences_reset`).
- What worked was granting access ahead of time through Chrome's own mechanisms: `chrome.developerPrivate.addHostPermission(extensionId, pattern)` in a `chrome://extensions` tab for site access, and loading the extension once with an API permission listed as required before reloading it with the real manifest. Chrome skips its prompt for permissions it recorded as granted, even after `permissions.remove`.

For **ordinary websites** (not extensions), DevTools `Browser.grantPermissions` covers web permissions such as notifications, geolocation, and clipboard. *This has not yet been exercised in this repo; confirm it and update this line.*

### External programs the code under test runs (AI harnesses, CLIs)

Never let a UI-free run find or start the real program, and never make a real model call. Pin what the code under test can see:

- The browser passes its own environment to the native host, so a host started by the test browser inherits the runner's `PATH`, credentials, and cloud settings. Start the host through a wrapper that clears the environment (`exec /usr/bin/env -i HOME=<temp home> PATH=<fake-bin>:/usr/bin:/bin ... <host> "$@"`) and sets only what the run needs, including a fake credential and a name the code must not pass on.
- Put fake programs in a private directory (0700, owned by the user) as symlinks into a separate install directory, so the test can swap a passing fake for a failing one with one atomic rename. The fakes use only shell builtins and absolute paths, and log their argv and environment variable names (never values) to a file the test checks.
- Remember any search locations the code adds beyond `PATH` (Brauser adds `$HOME/.local/bin`); a temporary `HOME` keeps those empty.
- Before clicking anything that runs a program, check that every offered program's resolved path is inside the fake install directory, and stop the run if not.

- **The Brauser service.** Never let a test reach launchd: no `launchctl bootstrap`, `kickstart`, or `bootout`, and nothing under the real `~/Library`. The headless runner starts the service child itself, as `<host wrapper> service` with the same cleared environment and temporary `HOME`, logging to `service.log` in the run directory, and waits for `service.sock`. Before that, the host runs without the test-only spawn seam, so a `start_run` must answer `service_unavailable` and no socket may appear. The Rust integration test `host/tests/service.rs` instead lets the host spawn the service directly through the `BRAUSER_SERVICE_SPAWN` seam; unit tests inject the launch function. `scripts/register-dev-host.mjs` and `scripts/dev-service.mjs` take an injectable `run` so their tests (`npm run test:scripts`) record launchctl argv without running it.
- **The fake agent.** Service runs use one test-only harness named by `BRAUSER_FAKE_HARNESS` (honored only in `scripted-dialogs` builds): a `/bin/sh` script outside the fake `PATH` directory, so harness discovery never sees it. It reads a mode file in the temporary `HOME` (`ok`, `slow`, or `fail`), writes a diagnostic line to stderr so `run.log` exists, and prints a JSON answer.
- **A short `HOME` under `/tmp`.** The service socket lives in the support directory under `HOME`, and a Unix socket path must fit in `sun_path` (104 bytes on macOS, 108 on Linux; Brauser enforces 104 on every Unix). A temporary directory under `$TMPDIR` (`/var/folders/...`) is long enough to break that, so the runner and the Rust service tests make their `HOME` with a short prefix directly under `/tmp` (`mkdtemp("/tmp/bsh-")`, `tempdir_in("/tmp")`) and remove it at the end.
- **The service on Linux.** The service, its unit tests, and `host/tests/service.rs` build and run on any Unix; only the launchd glue is macOS-only (`cfg(target_os = "macos")`). On Linux the tests' temporary `HOME` resolves the support directory to `$HOME/.config/brauser` (the tests ask the host with `--config-path` rather than hard-coding it), and the `scripted-dialogs` spawn seam starts the service exactly as on macOS. A production Linux host never starts the service: with nothing listening it answers `service_unavailable`. To type-check the Linux build from a Mac, run `cargo check`/`cargo clippy --target x86_64-unknown-linux-gnu`; `rfd`'s `wayland-sys` build script needs a `pkg-config` that allows cross builds, so point `PKG_CONFIG` at a stub that exits 0 and set `PKG_CONFIG_ALLOW_CROSS=1` (check never links). A native Linux build needs the real `libwayland-dev` and `pkg-config`.

Reference: `scripts/smoke-macos.mjs` (`installFakeHarnesses`, `onlyFakeHarnesses`, the agent service steps) and `host/tests/service.rs`.

### Prove the run stayed invisible

Don't just assume headless means invisible; check it. List on-screen windows (`CGWindowListCopyWindowInfo`, which includes menu-bar items) and Dock apps (`NSRunningApplication.activationPolicy == .regular`; an accessory app with no window shows nothing, and the browser's notification helper is one) for the browser's process group and any helper processes. Assert zero at launch, while each scripted dialog is open, and at the end. Both calls are read-only and need no Accessibility access. On failure, report which process was counted, not just the totals. Reference: `scripts/smoke-macos-windows.swift`.

## What the UI-free run cannot prove

- The real native dialogs appear, look right, and return the user's choice.
- Chrome's install-time permission warning, and what a real user sees after withdrawing and restoring site access in Chrome's own UI.
- Browser-startup ordering for an installed extension: whether Chrome loads it, with its listeners, before restored tabs commit, and `runtime.onStartup` (see the `--load-extension` and Chrome for Testing notes above). A loaded extension with a stopped worker does get restored tabs' commits; one loaded during startup misses the active tab's.
- Real idle and screen-lock transitions: the headless run has no way to drive them, and `chrome.idle.queryState` reported `active` throughout.
- Visual or OS-level effects such as focus, cursors, and window lifetime (for example, the macOS busy cursor, or whether the real folder picker closes when the host goes away).
- launchd: that `launchctl bootstrap` loads the dev LaunchAgent, that `launchctl kickstart` starts it on demand and `kickstart -k` replaces a stale build, that it exits after its idle timeout, and the environment launchd gives it. A release install's `SMAppService` registration (M5) is not built or tested at all. Real harness logins (Codex's stored login, Claude Code on Bedrock through `agent.env`) are not exercised either.

Cover these in real-UI acceptance, and don't claim them from a headless pass.

## Real-UI acceptance

Real-UI checks run on a desktop, so:

- Run one only when the user asks or agrees, and warn them just before anything appears or takes focus.
- Keep it short, and tell the user not to click unless the check needs a person.
- Design the check to run unattended where possible: have the code under test close the UI itself (a timeout or cancel path) and assert on what is on screen, so no one needs to click and nothing needs Accessibility access.
- Make the script refuse to run without an explicit flag (`--on-desktop` or `--auto`), so it can't be run by accident.

Checks of this kind in this repo:

- `npm run smoke:macos -- --auto`: the full flow with the real folder picker, answered through UI scripting. It takes keyboard focus.
- **[human] The Brauser service under launchd** (PR 2 of the open-access spec; also tracked as `brwr-sv2` in BOSUN_FEATURE_BACKLOG.md). It changes the user's own launchd domain, so a person runs it: (1) register the dev build with `node scripts/register-dev-host.mjs <extension ID> "$(pwd)/target/debug/brauser"` and close Chrome; (2) reopen Chrome and send a `start_run` from the panel's DevTools console; (3) confirm that `launchctl print gui/$UID/com.rauser.brauser.service` shows the service, and that it exits after 10 idle minutes; (4) confirm that `agent.env` values reach a run (this needs a real runner, so it waits for PR 3); (5) confirm that a rebuild plus a request restarts the service. *Not yet run.*

## Changing this document

This standard is meant to improve with use. When you change it:

- Update the relevant section directly; don't just append contradictions. If an approach stops working (for example, a Chrome update breaks a workaround), replace it and note what broke.
- Mark untested claims in *italics* as untested until someone runs them.
- Add a dated line to the learning log below with what you learned.
- If a change alters the hard rule, confirm it with the user first.

## Learning log

- 2026-10-27: Salvage review of Cookie cycle 2 reproduced three page-pattern
  gaps with failing-first tests: unreadable pending-removal state on a
  same-origin skipped page left the previous artifact mapped for attention
  time; a failed sample purge during pause was hidden, so resuming could keep
  the old rows; and a failed visited-sample read discarded the page's own
  shapes in the panel. The link reader now walks the lazy `document.links`
  collection and never reads its length, so the 500-link cap also bounds work
  on a hostile page; the site-pattern test's mock fails if length is read.
  A same-URL reload kept a completed review's link shapes, since the panel
  compared only tab ID and URL; any `loading` or URL update in the reviewed
  tab now discards the review, held reads included.
  A failed sample purge on site removal or policy suspension was likewise
  only logged, so re-enabling the origin revived its rows. Every purge now
  runs after the narrower policy commits (never blocking it), falls back to
  deleting all samples, and reports only if deletion fails too; maintenance
  also expires samples when pause or removal state is unreadable. A
  same-origin skipped page whose tab read failed also left the previous
  artifact mapped, since sampling moved the tab read ahead of the
  artifact match; that path now ends the entry too. A lease that expired
  while the tab and samples were read could still gain a sample; the worker
  now rechecks the lease before retaining one.
- 2026-10-27: Independent review reproduced an oversized same-origin
  navigation leaving the previous artifact mapped for attention measurement.
  The worker regression starts on a recognized artifact, navigates to a
  search URL over the byte limit, and verifies that later focused time is
  excluded and no pattern is retained. Oversized navigations now end the
  previous mapping through the normal unrecognized-page path. No browser or
  native dialog is involved.
- 2026-10-27: Branch `feat/brauser-home-folder`. The one-time move of an
  older folder layout to the fixed home layout is tested natively against a
  checked-in copy of a real pre-home install (`host/tests/fixtures/pre-home-layout`:
  the real config shape with `pages_dir = "pages"`, `log_dir = "log"`, and only
  `pages/` on disk), copied to a temporary folder per test. The tests cover
  the move through `hello`, refusal when `notes/` already exists (config.toml
  and both folders byte-exact, notes and saves refused, retried after the user
  moves the folder), a crash after a rename and before the config write,
  three hosts migrating at once while another holds the config lock or the
  capture lock, intent replay and torn-batch recovery after `log/` is renamed,
  and an index rescan after the rename. Removing the capture lock from the
  migration, or skipping the migration, makes those tests fail. The headless
  smoke run now checks the host saves the fixed layout, settings shows it with
  no folder-name field, and the home holds only the fixed folders at the end.
  The real folder picker was not exercised.
- 2026-10-27: Review reproduced the same stripped-URL collision through the
  fallback for an origin config.toml no longer covers: after the adapter and
  `[[adapter_id_history]]` were gone, the bare Jira board and another issue's
  modal resolved to ABC-123's key and autosaved over its note, and the
  board's URL note could migrate into it. That fallback also scanned the
  whole notes folder on every uncovered page and refused notes everywhere
  once it passed 4,096 files. Failing-first native tests
  (`without_recorded_contracts_*`,
  `a_large_notes_folder_does_not_block_notes_on_uncovered_pages`) cover it;
  uncovered origins now use the URL key, and tests that once asserted
  history-only recovery assert the artifact note stays untouched instead.
- 2026-10-27: Review reproduced, against the native handlers, a bare Jira
  board reading and autosaving into an issue's note because `strip_params`
  made the modal's stored canonical URL equal the board URL; the board's URL
  note listed as an "older note" of every modal issue and migratable into
  one; and site removal or an inert adapter document splitting a note across
  routes with no recorded activity. Failing-first native tests
  (`stripped_identity_collisions`, `note_policy_removal`) and a config unit
  test for `note_key_rule` now cover them. Tests that exercise history-only
  recovery remove `[[adapter_id_history]]` first, as a config repair would,
  because recorded contracts otherwise decide the key. Note-removal fixtures
  are dated from the clock, since capture rejects events older than 30 days.
- 2026-10-27: Independent review reproduced a tab switch during Capture Site's
  post-save note reload leaving the editor bound to the previous page. The
  composed-panel regression holds that load open, switches pages, then verifies
  the new page's body and the next save's URL. Note loads are suppressed only
  while the policy transaction can change identity; during the reload, tab
  events reach NoteEditor's existing cancellation tokens. No browser or native
  dialog is involved.
- 2026-10-26: Branch `test/smoke-pause-resume`. The headless smoke run now pauses from the panel and resumes from settings: it checks that pausing shows the "Capture off" strip and a status pointing to Resume in settings with no setup warning (DESIGN.md §5.7), keeps the site's adapter, and measures nothing, that **Resume capture** turns capture back on without opening a native dialog (rebased onto #69, which removed its confirmation), and that a later visit reaches the daily log. The panel's pause button sits inside the collapsed capture strip (`<details>`), so a real click on it does nothing until the runner opens the strip; check the host state after a click rather than trusting it.
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
  viewing, migration refusal and capture suppression. Since protocol v6
  (2026-10-26) rule narrowing, like every removal, is a plain framed
  `update_config` with real revision checks and no native confirmation; site
  removal is adapter removal, because sites derive from adapters. It covers edit/view
  routes, mode-only evidence, context without measured activity, notes without
  delivered history, occupied URL notes, ambiguous mappings, stale keys and
  versions, deleted caches and reindex. Separate file-count, per-file byte,
  aggregate-byte and non-UTF-8 fixtures prove incomplete primary scans refuse
  loads and autosaves without changing notes; optional older-note scans stay
  best effort. Three regressions failed before their fixes (initial fixture
  errors used invalid adapter syntax and a future timestamp and were corrected
  first). The existing unavailable-history test now exercises both activity
  and `why` evidence hiding a conflicting identity. No real browser or native
  dialog is involved; for the macOS smoke, the smoke's answered-dialog counter must equal the `shown.jsonl` line count, with no hard-coded total.
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
  claimed; for the macOS smoke, the smoke's answered-dialog counter must equal the `shown.jsonl` line count, with no hard-coded total.

- 2026-10-27: The single/batch activity parity test covers 13 scenarios through
  both native handlers with separate stores, explicit expected outcomes and
  reasons, correlated event IDs, no log/index writes on refusal, and decoded
  durable payloads for accepted activity (including mode, trail and title-ref
  evidence). It passes before and after consolidating authorization: duplication
  was confirmed, but no existing behavioral divergence was reproduced. Initial
  test-only failures assumed markers contained plain event IDs or struct field
  order; decoding their hex JSON checks the actual contract. No macOS run is
  claimed; for the macOS smoke, the smoke's answered-dialog counter must equal the `shown.jsonl` line count, with no hard-coded total.
- 2026-10-27: Independent review reproduced a sanitizer-upgrade recovery failure with a pre-upgrade v1 intent and a log torn inside a Unicode format character. Exact-hash compatibility now resumes that original entry through both single and batch delivery, preserves unrelated bytes, refuses changed payloads and edited tails, and deduplicates a complete marker. A second regression covers activity and context payloads. Future renderer-independent single-entry intent storage remains tracked as `brwr-3cc`; no macOS execution is claimed.
- 2026-10-27: Unicode-spoofing regressions first failed in daily rows and
  generated artifact files, then passed with the shared host presentation
  sanitizer. Tests exercise single and batch activity/legacy delivery, explicit
  context, exact encoded marker payloads, reindexing into owned files, unchanged
  source logs, plain worklogs, character caps, and readable non-ASCII text.
  The format-character table was exhaustively compared with the local Node
  Unicode 17 `General_Category=Cf` property (170 scalars); no dependency or
  network fetch is needed. These are host filesystem tests, not evidence of
  an Obsidian/VS Code preview or a macOS Chrome run. For the macOS smoke,
  the smoke's answered-dialog counter must equal the `shown.jsonl` line count, with no hard-coded total.
- 2026-10-27: Trail-transition validation uses a strict allowlist for new activity and the original bounded shape checks for historical markers. The regressions test terminal refusal through both native delivery paths, all browser values plus `forward_back`, and a pre-validation ingestion cache followed by reindexing an owned marker. They verify actual SQLite edge kinds, retained activity totals and exact payloads, unchanged source logs, and continued rejection of oversized or unpaired legacy trails. No browser or native dialog is involved.
- 2026-10-27: Branch `fix/c2-index-full-rebuild-cap`. Two index regressions failed first on main: once the logs together passed the per-pass read bound, a deleted cache, `brauser reindex`, `reconcile_from_logs`, and a removed or truncated log each returned unavailable history on every call, and the reset after a removed log let a backdated duplicate in an earlier-dated log replace the conflict winner the cache had chosen. Review then found two more, also failing first: a bounded pass never checkpointed headerless Markdown, so non-log files that filled a pass were reread forever, and a restored winner kept the rejected body's log position, so a same-instant sibling event took over the latest title after any rebuild. A second review round found that an earlier-dated log arriving between the passes of a cacheless rebuild let the later-dated body win, unlike a one-pass rebuild; the regression compares both rebuilds of the same logs. The tests lower the bound with the test-only `SCAN_LIMIT` (sized from one fixture log, so two logs fit per pass) instead of writing 64 MiB of fixtures; `bounded_passes` checks every pass's `LOG_BYTES_READ` stays within the bound and reports unavailable history until the rebuild finishes, and that no partial cache is published meanwhile. No browser or native dialog is involved.
- 2026-10-27: Branch `fix/lanec-index-robustness-2`. Index regressions failed first on main: a crafted log title over 2,048 bytes stopped every later `index.db` write, an ingestion cache already holding an oversize row did the same, a record payload between SQLite's local table-leaf limit (65,501 bytes at 64 KiB pages) and the old cell check was written inline and read back as "database disk image is malformed", and an M1 visit rendered `opens: 0` with an empty Timeline. The SQLite checks run Python's `sqlite3` `pragma integrity_check` on the written file rather than trusting the writer; size the fixture from the record header (a one-column text record of 65,497 bytes has a 4-byte header) so one row sits exactly at the limit and one just over. No browser or native dialog is involved.
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
- 2026-10-26: Branch `feat/open-access-pr1`. The native Yes/No confirmation is gone; the folder picker is the only native dialog. Removed the `macos-alert` crate and `scripts/check-macos-alert.mjs`.
- 2026-10-26: Branch `feat/open-access-pr1`. Install-time HTTP(S) access means the UI-free run no longer grants anything ahead of time, and the folder picker (notes and knowledge base) is the only dialog left. The dialog-counter rule is now explicit: `dialogsShown` must equal the scripted answers the runner gives, and the last check compares it with the dialog log's line count, never a hard-coded total, because overnight cycle 3 once added a dialog without updating a fixed count.
- 2026-10-26: Branch `feat/agent-service-pr2`. A Unix socket path must fit in `sun_path`, 104 bytes on macOS, and a socket in a support directory under a `$TMPDIR`-based temporary `HOME` (`/var/folders/...`) can exceed it, so binding fails. The service refuses a too-long path outright instead of moving the socket, and the smoke runner and service tests now use a short `HOME` directly under `/tmp`. Tests also never call `launchctl`: the runner starts the service child itself, and the host's spawn seam replaces `kickstart` in the Rust integration tests.
- 2026-10-27: Branch `fix/retired-hand-removed-contracts`. Rewriting config.toml in a test no longer simulates lost adapter contracts by itself: any refresh under the config lock (every note request, and `ConfigStore::update`) records adopted contracts in `adopted-contracts.json` beside the test's config, so a later hand edit keeps their note keys. A fixture that means "no contract recorded anywhere" must also remove `config::adopted_contracts_path(dir)`, and tolerate its absence, since a contract removed through settings before any locked refresh is only in `[[adapter_id_history]]`. A plain `loaded()` store refreshes without the lock and records nothing.
