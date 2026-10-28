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

## Retired adapter history budget

`cargo test --locked -p brauser adapter_history` exercises real config.toml
fixtures at the 32 KiB history budget and near the 64 KiB whole-file cap.
It covers pausing capture and adding a disjoint site without changing history,
refused growth with byte-for-byte preservation, Capture Site preflight, and
safe duplicate/exact-restoration pruning. A restart checks that restored and
still-retired routes keep their artifact identities. Native dispatch tests
both budget and total-file refusals as actionable `invalid_config` replies.
Restoration must precede retirement of a different same-type ID; the existing
ID freeze still applies. All fixtures use disposable folders and no browser,
network, native dialog, or user config.

## Capture fixture clocks

Capture and native unit-test event helpers share `capture::fixtures::event_time`:
one frozen UTC noon on the preceding day per test thread. It stays inside the
host's real 30-day delivery window, leaves room for nearby fixture offsets,
and prevents separate helper calls from choosing different daily logs.
Production clock validation is unchanged. `daily_log_fixtures_work_at_midnight`
reruns the compact-batch/recovery and full-log rejection tests with fixture
clocks at 23:59:59 and the following midnight in UTC, UTC-12 and UTC+14.
The midnight batch regression failed before the safe-noon fixture was applied.

## Legacy capture recovery and normalized URL bounds

`capture::tests::over_limit_normalized_urls_complete_v1_intents_before_rejection`
seeds real v1 intents and torn owned logs with independently reconstructed
pre-limit bytes. It covers legacy visits and activity page, mode and trail
URLs expanded by percent encoding, through both single and batch delivery.
Changed payloads leave the partial bytes intact and remain retryable; the
original payload completes exactly once. Existing encoded-URL tests keep new
oversized writes rejected without creating logs or intents, and accept the
exact 8192-byte boundary. The mixed-batch regression checks that a rejected
oversized event cannot select the day or block a valid neighbor, in either
order. `completed_mixed_batch_recovers_after_selecting_a_different_home_folder`
interrupts cleanup after a real batch append has synced, then selects another
home folder. It checks that only appended rows are journaled, recovery clears
the completed intent without touching the old log, and capture continues in
the new folder. The interruption seam is compiled only into unit tests. All
use disposable local directories; no browser or network is used.

## Single-event recovery across renderer upgrades

`cargo test --locked -p brauser capture::tests` includes independently prepared
historical prose with unchanged machine markers in real v2 journals. A restart
recovers a tail torn inside a multibyte character through both single and batch
delivery, preserving unrelated rows and deduplicating completed markers. Changes
past the title display limit, a different selected root, and edited tails are
refused without writes. Why and summary fixtures cover the same prepared-byte
path, including summary path changes before and after marker completion.
Missing fields, bad hashes, unknown versions, corrupt bytes and oversized
journals/entries leave logs intact. An activity at all field bounds, with
JSON-escaped control characters, verifies the 512 KiB entry and 1 MiB journal
budgets. Existing pre-sanitizer and normalized-URL fixtures explicitly retain
v1 hash-only journals, so new fixtures cannot silently replace legacy coverage.
All use disposable directories and no browser, network or user configuration.

## Standalone overflow delivery

`extension/tests/worker.test.mjs` fills the byte-capped queue with one final
batch, holds its native reply, then freezes two completed activity hours through
the real pressure path. Both are dropped; acknowledging the final batch must
immediately send their count and range with empty `events`, without another
navigation. Other cases lose a standalone reply, read its frozen ID/snapshot
back from storage, add later drops, observe backoff and retry, and pause a
pending delivery. A refused-retry regression queues new activity beside an expired
standalone snapshot and checks that the activity drains first, even when the
marker receives `overflow_recorded: false` or an older host's `invalid_request`.
Existing attached-snapshot and Dismiss races remain covered.

`capture::tests::standalone_overflow_*` checks the offset-derived log day,
exact marker payload, torn compact-intent recovery, duplicate retries after
restart, malformed/range-invalid times, and preservation of unowned logs.
`native::tests::standalone_overflow_uses_the_validated_home_and_obeys_capture_gates`
checks real writes and duplicate delivery plus Pause, invalid adapter policy,
missing home selection and a replaced home directory. The wire test rejects
an invented URL in an overflow payload. All fixtures are local; no browser,
external service or real user directory is involved. The macOS smoke dialog
count is unchanged.

## Summary title extraction

`extension/tests/extract.test.mjs` serializes the real extractor into a fresh
VM context. A list-to-item SPA fixture changes its URL and body while keeping
tab, document and Readability titles stale, then runs the real `summarizeTab`
entry point through mocked Chrome injection and host transport. It checks the
item heading reaches `start_run` and the returned run, and that a caller still
holding the list URL is refused without starting another run. Separate cases
cover Readability/body fallback, missing/hidden/ambiguous headings, equal
duplicate headings, and title limits with surrogate replacement. The stale
title cases failed before the extractor preferred a unique main-page H1.
These are local mocks; actual GitHub navigation and Chrome rendering remain
macOS browser acceptance work.

## Panel status and artifact refresh coverage

The built-panel tests assert adapter-based capture wording with zero and some
adapters and clear the artifact verdict when a readable tab becomes a Chrome
page or loses site access. A reconnect test brings the mocked host back through
note autosave, checks updated artifact totals, and keeps an unsaved context
draft. Delivery tests advance the worker's `delivered_batches` counter before
Dismiss, Pause, Discard, or the normal poll; each updates the current card once
and an unchanged poll sends no lookup. They invoke the captured 30-second poll
callback directly, without waiting for wall-clock polling. Eight regressions
failed before the fixes; the ordinary-poll control already passed. These tests
use mocked Chrome and host transport, with no browser or external service.

## External policy changes and note identity

The built-panel tests deliver a valid policy lease for a different host
revision after loading a URL-keyed note. The next `load_note` returns an
artifact key, and the following autosave must carry that key and revision
without a conflict. Two held-save cases cover a pending debounce and newer
typing during an in-flight save: a `key_changed` response preserves the draft
in copy-out and synchronous recovery, loads the current host note, and never
replays the refused text. A later successful artifact save keeps the conflict
copy. An unchanged host revision leaves the editor and debounce alone; the
existing editor tests still check retrying failed loads.

Run the focused cases after `npm run build:extension` with
`node --test --test-isolation=none --test-name-pattern='external.*revision|policy refresh with the same' extension/tests/panel.test.mjs`.
The clean-note and in-flight rebinding cases failed before the fix. These use
mocked host/Chrome transport and in-memory browser storage; they do not prove
real Chrome timing or native host integration. Run the normal full extension
suite as the gate and keep macOS browser acceptance separate.

## Older-note migration coverage

The panel tests load an artifact with and without its shared note and check
that older notes offer **View** in both cases, and **Use as artifact note**
only when the shared note is absent. They open the older note read-only and
verify that viewing neither changes the editor nor sends a save or migration.
The note-editor tests cover migration availability after a first save, a
conflict with another writer, and deletion elsewhere, retaining conflict text
for copy-out. The host's
`url_keyed_note_is_listed_and_can_become_the_artifact_note` regression migrates
from a board modal to an item route, checks the original URL in the file and
through `artifact_keyed_note_url`, then saves from the item route and checks
that URL again. Existing ownership, stale-revision and occupied-destination
tests still guard the move. All of these use local fixtures and mocked host
transport; interactive Chrome acceptance remains separate.

## Preserved artifact-note viewing

`cargo test --locked -p brauser --lib preserved_note_browsing` covers multiple
ambiguous historical notes beside an occupied URL note, lost contracts,
stripped addresses, unavailable index/adopted history, rechecked ownership,
duplicate ownership fields, unrelated file kinds and bounded response metadata.
The primary key, stale-key save refusal and capture config stay unchanged.
Protocol wire tests round-trip both additive requests and the list response
and check the 24-item schema bound. `extension/tests/panel.test.mjs` checks
explicit discovery, text-only titles and bodies, Copy, the original autosave
key/revision, errors and empty lists, and stale list/view replies after
navigation, collapse, close or another selection. Run after building the
extension; `node --test --test-isolation=none --test-name-pattern=preserved
extension/tests/panel.test.mjs` exposes all eight focused cases individually.
The fake DOM does not initialize `disabled` from HTML: waits must observe a
sent `load_note` as well as an enabled editor before interacting with it.
Actual Chrome disclosure styling and keyboard interaction still need the
separate macOS interactive check; these tests do not claim that acceptance.

## Page-note recovery coverage

`extension/tests/note-editor.test.mjs` drives the real recovery store with
in-memory synchronous storage. It covers pending debounce, newer input during
an in-flight save, failed saves, conflicts, empty deletions, exact acknowledged
snapshot cleanup, independent panels, stale discard, quota errors, blocked
storage, partial write/cleanup failures, malformed/unrelated entries and
artifact-key changes. Reopened editors load the current host note and never
replay recovery text automatically.

`extension/tests/panel.test.mjs` runs the built panel module and shipped HTML
with the real native client and mocked DOM/Chrome/storage. Its close/reopen
tests exercise debounce and in-flight-save teardown, a late host completion,
separate current and recovered text, Copy/Discard, persistent conflicts
(including empty deletions), subsequent successful edits and visible quota
errors while native autosave continues. Clipboard calls are captured in memory;
no test touches the user's clipboard or a real browser profile.

These prove the composed client flow; Chrome storage durability, layout and
real panel teardown still need the macOS browser gates. The headless macOS
runner refuses Linux, and no real-browser pass is claimed on this VM. No
smoke step, real dialog or protocol changed for recovery.

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
with its transcript instead of expecting a fixed total.

### Unknown-site drafting (`brwr-t40`)

Two shared fixtures pin both sides of the boundary to one source:

- `protocol/capture-site/catalog.json`, the host-owned drafting catalog. The
  host parses it (`host/src/capture_site.rs` tests check its shared origins
  equal `SHARED_ORIGINS` and that its probes cover the root, search, auth and
  capability routes); `extension/tests/adapter-review.test.mjs` pins
  `probeUrls` to its `probe_paths`, and `starter-adapters.test.mjs` checks
  every starter type is reserved in `starter_types`.
- `protocol/capture-site/shapes.json`, real URL forms (every starter's
  recorded forms plus Confluence, SharePoint `sourcedoc`, Notion, Google Docs
  `/u/0/`, Linear, percent-encoded and long mixed-case names) with their
  expected shapes, and shape strings that must be refused. The in-page shaper
  (`site-patterns.test.mjs`) and the Rust parser
  (`adapter-templates/tests/shapes.rs`) both read it.

The per-rule checks have 32 `draft_rules` vectors (protocol/adapter-templates/README.md)
plus unit tests in `adapter-templates/src/draft.rs`. The drafter's step 7.6
cases are in `adapter-templates/tests/structural.rs`: recorded starter URLs
drafting to matching adapters, the GitHub pull request and `/files` tab, the
`hex(32)@tail` and Notion (`brwr-gf1`) cases, SharePoint `sourcedoc`, Google
Docs' account index, seven planted-link cases (plus the `adapter-templates/src/structural.rs` unit test `link_only_alternative_never_widens_visited_rule`) and overlap. The
`structural_draft` fuzz target checks that every returned rule passes the
per-rule checks and none matches the root. Host tests in
`host/src/capture_site.rs` cover the handlers: revision binding, raw-URL and
invalid-shape refusal, repair refusal, recorded contract types counted as
taken, the full preview, zero re-mapping, no writes to the home or config
directories, the outbound cap at the request limits, and `check_site_draft`'s
overlap, user-written warnings, stale-revision conflict, scope choices and
the forbidden-key refusal of a table that carries an overlay. Extension tests
cover the plan (`capture-site.test.mjs`), the composed review, save,
recurring-pages hint and older-host fallback (`panel.test.mjs`), the
advanced-editor handoff and per-rule provenance (`options.test.mjs`) and the
worker's handoff expiry and that link shapes are never sampled, buffered or
saved (`worker.test.mjs`). They assert that no request carries a raw URL,
link `href` or tab title, and that no Chrome permission request or native
dialog occurs.

The headless smoke run adds `step("Capture Site drafts and saves an unknown site")`
before `step("Site removal")`. It serves `/wiki/pages/<n>/<Title>` pages
that link to sibling pages, `/search?q=` and `/login` on the second origin
`http://localhost:<port>` (distinct from the configured `127.0.0.1` origin),
clicks **Capture Site**, and checks one checked "pages" rule that captures
this page, the counts ("1 of 1 visited patterns and 2 of 4 link patterns"),
the search and sign-in links in the "won't capture" list, and the safety
list. It saves, checks the host holds a drafted adapter with only Tier 0 keys
while the configured site's adapter is unchanged, checks the reloaded panel
shows the artifact card, and checks no native dialog was shown. `Site
removal` then removes both origins. This proves the real Chrome injection
and save on one simple site; it does not certify drafts on live tools.

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
claim that it passed here. Unknown-site drafting is covered separately
(above); these tests do not certify any machine-drafted rule.

## Capture Site attention across an hour boundary

The worker's already-open-tab policy-install test uses fixed local times,
covering both a mid-hour interval and the last ten seconds of an hour. After
15 seconds, `get_status` freezes completed hours into delivery: the boundary
case must retain 10 seconds in the queue and 5 seconds in live totals, with
zero opens across both. Queued activity uses seconds; live totals use
milliseconds. Checking only live totals against 15 seconds made the old test
fail whenever the real wall clock happened to cross an hour boundary.

## Capture and index round trips

`cargo test --locked -p brauser ids_survive_reindex` captures lowercase,
uppercase and unhyphenated uppercase UUIDs through single activity, batch
activity and Why delivery, reads the actual hex marker payloads, then rebuilds
the index. It checks preserved measurements/context, canonical payload IDs,
hash mismatch refusal and idempotent immediate index updates from the original
request. Why fixtures first capture measured activity: context alone does not
create an artifact. The regressions failed before the writer fix.

`cargo test --locked -p brauser encoded_` rejects Unicode page, mode and trail
URLs whose encoded spelling exceeds 8192 bytes, including a one-byte overflow.
It exercises single/batch activity, legacy single/batch visits, Why and the
shared summary writer, checking that no log or intent was created. The page
and nested-URL regressions persisted before the bound fix. The companion
`normalized_urls_at_the_capture_limit_survive_reindex` test accepts actual
8192-byte serialized URLs in every activity URL field and checks rebuilt
measurements. These use local temporary folders and no service, browser,
dialog, credentials or external API. Historical noncanonical markers and
partial intents are a separate recovery follow-up; these tests cover new
writes and existing canonical IDs.

## Dialog reply fixture

`dialog::tests::a_dialog_that_answers_in_time_keeps_its_reply` uses a shell
child that reads the length line and exactly that many body bytes before
printing its reply. It exercises 60 exchanges across empty, short and 64 KiB
UTF-8 bodies. Do not replace the reader with an immediate `printf`: that
closes stdin before the parent finishes writing and races into `EPIPE`. Do
not read until EOF either: the parent keeps stdin open until the child exits.
The fixture uses the same `head -c` tool as the neighboring timeout fixture.

## Local IPC in sandboxes

Some development sandboxes deny local IPC. Codex's `workspace-write` sandbox
on Linux, which Cookie's in-cycle health gate runs in, refuses to bind or
connect a Unix socket even in a temporary directory (EPERM), and Node's child
pipes, which are socketpairs on Linux, fail the same way: a `spawnSync` child
can come back with `status: 0`, `error.code: "EPERM"` and empty stdout. This
is a limitation of the sandbox, not a product defect. Do not change service
paths, permissions or production IPC to fit it.

Every test that binds or connects a Unix socket, or starts a service that
does, starts with a guard (Rust: `host/src/ipc_test_guard.rs`, test and
`scripted-dialogs` builds only; `test:scripts`: `scripts/tests/ipc-guard.mjs`,
with a `"socket"` and a `"child-pipe"` probe). One cached probe per test
process binds and connects a socket in a fresh `/tmp` directory (the Node
`"child-pipe"` probe reads a child's stdout instead) and then:

- IPC works: the test runs.
- EPERM or EACCES, with `BRAUSER_REQUIRE_IPC_TESTS` unset: the test returns
  early and prints `<test>: skipping: local IPC denied by sandbox (...)` to
  stderr, so the suite passes in the sandbox without hiding the skip.
- EPERM or EACCES with `BRAUSER_REQUIRE_IPC_TESTS=1`: the test fails. CI sets
  the flag on every build job (`.github/workflows/build-os.yml`), so CI never
  skips a socket test.
- Any other probe error: the test fails, flag or not.

`BRAUSER_TEST_SIMULATE_IPC_DENIED=1` makes the probe report a denial without
trying, to check both paths on a machine that allows IPC. On macOS a real
denial can be reproduced with a Seatbelt profile that denies
`network-bind (local unix-socket)` and `network-outbound (remote unix-socket)`
under `sandbox-exec -f <profile> cargo test ...`. A new socket test must start
with the guard (`if crate::ipc_test_guard::ipc_denied() { return; }`, or
`brauser_host::ipc_test_guard::ipc_denied()` from `host/tests`; in Node,
`if (await ipcDenied(t, "socket")) return;`); without it, it fails in the
sandbox like the 32 to 47 tests that kept Cookie's gate red before `brwr-rq5`.

This includes connections to a missing listener: a restricted runner can
return EPERM before the client sees the expected missing-socket error, so
missing-service, failed-launch and launch-timeout tests need the guard too.
A boundary test that proves rejection without connecting but also verifies
successful forwarding at the limit needs it for that forwarding step.

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

### Waiting for state, not time

The required CI check runs the headless smoke on a shared runner, sometimes the slower `macos-15-intel` fallback, so every wait must hold up when the machine is loaded. `scripts/smoke-macos.mjs` has one helper for this, and new steps should use it:

```js
const state = await waitUntil("what the step waits for", observe, accept, { timeout, interval, signal: abort.signal });
```

`observe()` reads the current state (anything `JSON.stringify` can show: host config, worker storage, run cards, files). `accept(state)` says whether that state is the one wanted. The accepted state is returned. On timeout it throws `Timed out after <s> waiting for <what>; last observed <state>` plus the last error, so a stop shows what the run saw, not just "found []". An error marked `fatal` (`Object.assign(new Error(...), { fatal: true })`) stops the wait at once. An optional `diagnose()` runs once on timeout, and what it returns is added to the message. Use it for a costly read such as `get_run`, so that read stays out of every poll. `cardIn(runId, states)` uses `waitUntil` this way: on timeout it reports the card last seen (or that there is none) and the run as the service sees it. Use that for states that can never recover, such as a blocked page that was measured. The older `waitFor(check)` (the check throws until it passes) is still fine for simple UI polls whose error message already names the state.

Rules learned from the stops fixed in `brwr-smk`:

- **Wait for the state the next step depends on, not for something that usually comes with it.** The worker judges a navigation by the lease it holds when the navigation commits, and never looks at it again. So before a step navigates and expects capture, call `workerHoldsHostPolicy()`, which waits until the worker's stored lease has the host's current revision. "Settings is connected" is not proof of that.
- **`Page.reload` can return while the old document still answers.** Use `reloadSettings()`. It marks the old document and waits for an unmarked, connected one.
- **Identify a new item by excluding everything that already existed, not by what the page showed a moment earlier.** A new panel enables **Summarize & log** before it restores earlier runs' cards from `list_runs`, so `summarizeFromPanel(harness)` treats every run the service already lists as old, and checks the new run's harness.
- **A cold Chrome can take far longer than one CDP request's timeout to start reading its pipe.** The runner waits for `Browser.getVersion` before `Extensions.loadUnpacked`. It never retries a non-idempotent request. Chrome writes `Preferences` on a delay of about ten seconds, so the wait for the profile to record the extension needs room well beyond that. The runner logs each of these times.
- **Retry only what is safe to repeat, and check the result before asking again.** `openPanel()` looks for the panel target before each new `chrome.sidePanel.open` request. Opening a panel that is already open changes nothing.
- **Stop at once when the page already says why the wait cannot succeed.** After **Summarize & log**, the panel shows a run line until the card appears. A warning line, such as "can't read this page's content", means no run was started, so `summarizeFromPanel` fails with that text instead of waiting 30 s and reporting only the old cards. Look for the page's own failure text before adding a long wait.
- **Never build in a worktree while its smoke runs.** Chrome reads an unpacked extension's files from disk as it needs them. `npm run build:extension` deletes `extension/dist` and writes it again, even for the same commit. So a build during a run (for example a review that runs the gate in the same worktree) loses the panel's page (`Panel shows: null`, `Panel shows the URL: (none)`) or the injected `vendor/readability.js` (no run card). The runner refuses to start while `extension/.build` exists. It records each `dist` file's inode, size and modification time at the start, fails a run that otherwise passed if they changed, and adds `Also: extension/dist was rebuilt during the run` to any other stop.
- **To check a wait, run the smoke under full CPU load** (one `yes > /dev/null` per core, from `sysctl -n hw.ncpu`, killed afterwards), not just on an idle machine. Every stop fixed in `brwr-smk` passed when idle.

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
- **The fake agent (Rust tests).** The service's own Rust tests (`host/tests/service.rs` and the server, supervisor, setup, and registry unit tests) use one test-only harness named by `BRAUSER_FAKE_HARNESS` (honored only in test and `scripted-dialogs` builds, and only for an absolute executable): a `/bin/sh` script outside any search path, so detection never sees it, registered for both harness ids through the plain stdin-to-stdout `CommandRunner`. It reads a mode file in the temporary `HOME`, writes a diagnostic line to stderr so `run.log` exists, and prints a JSON answer. The headless smoke run no longer sets it.
- **Fake `codex` and `claude` (headless smoke).** The smoke runner's fakes are Node scripts with a `#!<this node>` shebang in the fake `PATH` directory, so the service detects them and the real Codex and Claude Code runners drive them. `codex --version` prints `codex-cli 0.144.4` and `codex app-server --listen stdio://` answers `initialize`, `thread/start` (thread `fake-thread-1`), `thread/name/set`, and `turn/start`, then streams `item/agentMessage/delta`, `item/completed`, and `turn/completed`; `claude --version` prints `2.1.310 (Claude Code)` and `claude -p …` prints `system`/`init` (session `fake-session-1`), text deltas, the assistant message, and `result`. Both read `$HOME/fake-agent.mode`: `ok` (a valid §6.4 answer in two deltas), `slow` (no answer until killed), `fail` (stderr and a failed turn or exit 3), `badjson-once` (not JSON, then a valid answer to the repair turn), `badjson` (never JSON), and `summary` (a valid answer whose description holds a Markdown image, an `<img>` tag, a Mermaid fence with an image node and a Dataview inline query, and whose entity is an `<img>` tag, for the summary-file steps); the setup probe is answered `OK` except in `fail`. Each logs one JSON line per start to `harness-argv.log` with its argv, cwd, and environment variable names (never values), then what it read (each JSON-RPC request, or Claude's whole stdin prompt), so the run can check that no argv carries article text and that the prompt arrived on stdin.
- **Summary outputs (headless smoke).** After the agent steps the runner sets Codex up again and summarizes in `summary` mode: a page with a note (the note's bytes must not change), then a page the fixture adapter recognizes. It checks the summary file itself (owned `brauser:` block; front matter that parses back to the plain values, with no entities and no raw `<` or `![`; the body's `\![`, `&lt;img`, `&#96;&#96;&#96;mermaid` and `&#96;&#36;= dv.el(`, with no line starting with a backtick fence, no raw `` `$= `` and no `<img` that is not backslash-escaped), the decoded `kind=summary` marker in the log for its own date, `list_artifacts` (a `web.page` item, and the recognized page's summary on its `smoke.page` item with no `web.page` item), the worklog line, the artifact files after `refresh_artifacts` (the Summary link resolves to the file; the summary line sorts above its day's rollup), Retry save over an unowned file (fails as `save_failed` with the file untouched; after the file is deleted, one `retry_save` logs one event and a second logs none), `reindex`, and the panel's `#page-summary` line and **Open summary**, which must write `-- <home>/<summary_path>` to `open.log`.
- **The opener.** Tests never run `/usr/bin/open`. In test and `scripted-dialogs` builds the opener (`host/src/open.rs`) appends what it would have opened to the file named by `BRAUSER_OPEN_LOG` and fails when that is unset; the smoke wrapper sets it to `open.log` in the run directory and checks `codex://threads/fake-thread-1` there after **Open in Codex**. The native host's `open_summary` uses the same opener through `SetupContext.open`; its unit tests (`open_summary_*` in host/src/native.rs) pass a recording opener instead and check the `-- <path>` argv, and an `Unsupported` opener for the off-macOS answer.
- **The Codex app-server runner (`host/src/service/codex.rs`).** Unit tests put a `#!/bin/sh` fake named `codex` in a private `bin` directory and pass that directory as the runner's search path. The fake answers JSON-RPC ids 1-3 (`initialize`, `thread/start`, `thread/name/set`) in order, runs a per-test turn script for later ids, logs every stdin line to `rpc.log`, and logs its argv, cwd and environment variable names (never values). What the runner sends and reads follows the installed `codex` 0.144.4 (aarch64-apple-darwin, from `~/.codex/packages/standalone/current/codex-package.json`), checked on 2026-10-27 by reading the installed binary's strings, not by running it:
  - argv `codex app-server --listen stdio://`, which the binary's help text lists as the default transport. There is no `--session-source` flag, so none is passed.
  - `thread/start` takes `sandbox: "danger-full-access"` (kebab-case), while `turn/start` takes `sandboxPolicy: {type: "dangerFullAccess"}` (camelCase); both use `approvalPolicy: "never"`.
  - The `turn/start` params include `outputSchema`, documented as an "Optional JSON Schema used to constrain the final assistant message for this turn", so the production registry passes the answer schema (`answer_schema()`) as `outputSchema`.
  - Notifications read: `item/agentMessage/delta` (`params.itemId`, `params.delta`), `item/completed` with `params.item.type == "agentMessage"` (`item.id`, `item.text`), and `turn/completed` (`params.turn.status` of `completed`, `interrupted` or `failed`, with `params.turn.error.message`).
  - **[human]** Confirm on a machine where running `codex` is allowed: `codex --version` and `codex app-server generate-json-schema --out target/codex-schema`, then `grep -rl '"outputSchema"' target/codex-schema`.
- **The Claude Code runner (`host/src/service/claude.rs`).** Unit tests put a `#!/bin/sh` fake named `claude` in a private `bin` directory and pass that directory as the runner's search path. Each run of the fake appends its argv to `argv.log`, saves its stdin per run, logs its cwd and environment variable names (never values), and prints per-test stream-json lines. What the runner passes and reads follows the installed Claude Code 2.1.311 (arm64 Mach-O, from the `~/.local/bin/claude` symlink target `~/.local/share/claude/versions/2.1.311`), checked on 2026-10-27 by reading the installed binary's strings, not by running it:
  - argv for the first turn: `claude -p --output-format stream-json --verbose --include-partial-messages --dangerously-skip-permissions --session-id <uuid>`, with a UUID Brauser chooses; a later turn passes the same flags with `--resume <id>` in place of `--session-id`, using the id the first turn's `system`/`init` event reported. The help text says `stream-json` requires `--verbose` in print mode and `--include-partial-messages` works only with `--print` and `--output-format=stream-json`. `--session-id` must be a valid UUID. `--no-session-persistence` exists but is never passed, so the session can be resumed. The prompt goes on stdin, which is then closed.
  - Lines read: `{"type":"system","subtype":"init","session_id":...}`; `{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":...}}}`; `{"type":"assistant","message":{"content":[{"type":"text","text":...}, ...]}}`, streamed only when the turn had no deltas; and `{"type":"result","subtype":"success"|"error_during_execution"|...,"is_error":...,"result":...}`, where an error result may carry `errors` instead of `result`. A turn succeeds only with exit 0 and a `result` with `is_error: false`, whose `result` is the answer.
  - **[human]** Confirm on a machine where running `claude` is allowed: `claude --version` and `claude --help` show these flags, and one `claude -p --output-format stream-json --verbose --include-partial-messages` turn prints the event shapes above.
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
- Real harnesses: that the installed `codex` and `claude` accept the recorded argv and JSON-RPC and stream-json shapes end to end (the fakes only follow what was read off the binaries), the quality of a real model's answer and whether it follows the §6.4 shape without a repair turn, and how a real agent with full access behaves on a hostile page.
- The Codex Desktop deep link: that `/usr/bin/open codex://threads/<id>` opens the thread in Codex Desktop. The smoke run only sees the URL in `open.log`.
- Readability on arbitrary sites: the fixture article and the unit tests' fake documents prove the extractor's shape and byte caps, not how Readability 0.6.0 does on real pages (single-page apps, paywalls, heavy layouts, very large pages).

Cover these in real-UI acceptance, and don't claim them from a headless pass.

## Real-UI acceptance

Real-UI checks run on a desktop, so:

- Run one only when the user asks or agrees, and warn them just before anything appears or takes focus.
- Keep it short, and tell the user not to click unless the check needs a person.
- Design the check to run unattended where possible: have the code under test close the UI itself (a timeout or cancel path) and assert on what is on screen, so no one needs to click and nothing needs Accessibility access.
- Make the script refuse to run without an explicit flag (`--on-desktop` or `--auto`), so it can't be run by accident.

Checks of this kind in this repo:

- `npm run smoke:macos -- --auto`: the full flow with the real folder picker, answered through UI scripting. It takes keyboard focus.
- **[human] The Brauser service under launchd** (PR 2 of the open-access spec; also tracked as `brwr-sv2` in BOSUN_FEATURE_BACKLOG.md). It changes the user's own launchd domain, so a person runs it: (1) register the dev build with `node scripts/register-dev-host.mjs <extension ID> "$(pwd)/target/debug/brauser"` and close Chrome; (2) reopen Chrome and send a `start_run` from the panel's DevTools console; (3) confirm that `launchctl print gui/$UID/com.rauser.brauser.service` shows the service, and that it exits after 10 idle minutes; (4) confirm that `agent.env` values reach a run (possible since PR 3 registered the real runners); (5) confirm that a rebuild plus a request restarts the service. *Not yet run.*

- **[human] Real summaries from the dev build** (PR 3 of the open-access spec; tracked with `brwr-sr3` in BOSUN_FEATURE_BACKLOG.md). It runs real agents with full access on the user's login, so a person runs it: (1) register the dev build and set up Codex in settings (**Detect harnesses**, then **Test and use**); (2) on a page you trust, click **Summarize & log** and confirm the card reaches "Summarized" with a sensible type, category, and importance line; (3) click **Open in Codex** and confirm Codex Desktop opens that thread; (4) set up Claude Code (for Bedrock, through `agent.env`), summarize a page, and confirm the card reaches "Summarized" and **Copy resume command** resumes the session in a terminal. *Not yet run.*

- **[human] Neutralized summaries in Obsidian and VS Code preview** (PR 4 of the open-access spec; recorded in DESIGN.md §5.2 step 6). `npm run test:markdown-neutralizer` proves the corpus inert in markdown-it, micromark (GFM) and marked, but not in Obsidian or VS Code, so a person checks: (1) write each neutralized output of `host/tests/fixtures/markdown-neutralize-corpus.json` into a scratch vault (set `BRAUSER_NEUTRALIZE_CORPUS_OUT` to a JSON file path while running `cargo test --locked -p brauser the_corpus_comes_out_inert` to dump each input with its `output`, `frontmatter`, `inline` and `wikilink` forms); (2) open them in Obsidian reading and live-preview modes, with the Dataview plugin enabled (Mermaid and math are built in), and in VS Code's Markdown preview, with the network monitored; (3) confirm no image, embed (`\![[embed]]` included), iframe or other remote resource loads, no code block, Mermaid diagram, Dataview query or math renders, and the text reads as the plain characters. Record the date and result in DESIGN.md §5.2 step 6. *Not yet run.*

## Changing this document

This standard is meant to improve with use. When you change it:

- Update the relevant section directly; don't just append contradictions. If an approach stops working (for example, a Chrome update breaks a workaround), replace it and note what broke.
- Mark untested claims in *italics* as untested until someone runs them.
- Add a dated line to the learning log below with what you learned.
- If a change alters the hard rule, confirm it with the user first.

## Learning log

- 2026-10-28 (`brwr-2iz`): Size history fixtures with pretty TOML, rather than
  JSON, because repeated table headers contribute to the persisted cap.
  Check both the store error and native reply: an otherwise helpful size
  error becomes a generic internal failure without a typed dispatch mapping.

- 2026-10-27: Branch `feat/capture-site-unknown` (`brwr-t40`). Unknown-site Capture Site pins the host catalog and real URL shapes in `protocol/capture-site/` and reads them from both the Rust and extension suites, so neither side keeps a private copy of the probe list, fill values or shaper expectations ("Unknown-site drafting" above). Scope, `unscoped` and title overlays are tested on both sides of `check_site_draft`: the checked request never carries them, and a table that does is still refused. `scripts/check_current_state.py` now rejects the retired "Unknown-site automatic drafting is still in development" claim. After merging #126 (protocol v7), the drafter's scoped-tenant anchor is pinned on both sides: `validate.drafted.*` vectors (rust, ts) show a drafted shared-origin adapter whose only anchor is its tenant validating scoped and failing `no_literal_segment` unscoped in both engines, plus the drafted Notion adapter and the ID-less rule (`id_unknown_capture`); `draft_rules` vectors and host `check_site_draft` tests show the same tables kept, `scope_anchored`, and refused unscoped as `draft_scope_choice`. The step 7.6 shared-origin panel test now expects the tab's tenant pre-filled and chosen as "Capture for <tenant>" (one-click Save), and a tab with no tenant in its URL still waiting for a choice.
- 2026-10-27: Branch `fix/strip-params-read-by-adapters` (`brwr-f4s`).
  Failing-first host tests (`config::tests::strip_params_readers`,
  `native::tests::update_config_refuses_stripping_a_parameter_an_adapter_reads`)
  showed `update` accepting a `strip_params` entry a Jira modal rule reads,
  including prefix, case, `absent`, and retired-contract variants. A refusal
  raised inside `update_with_grants` reaches the extension as `internal`
  ("could not save configuration") unless native dispatch downcasts its
  error type, so a new under-the-lock refusal needs its own type
  (`StripParamsRefusal`) as well as the pre-lock check against the store's
  last read. Settings tests cover the warning and the one-click fix with the
  fake DOM; the headless smoke run does not cover it, because it never edits
  config.toml by hand. Codex review then reproduced an existing conflict
  masking a new rule on the same adapter that read the same parameter,
  because conflicts were compared by entry, parameter, origin, and type
  only; they now include the reading rule and `id` template, and the
  existing-conflict test adds such a rule first. No browser or native
  dialog is involved.
- 2026-10-27: Branch `feat/agent-runners-pr3`. The page extractor caps text in encoded bytes, not characters: it truncates until the JSON-encoded, UTF-8 length of the content plus the selection fits 3 MiB (the selection alone 64 KiB). CJK text is three bytes a character, and quotes and control characters grow under JSON escaping (`\u0001` is six bytes), so a character cap can push a `start_run` frame past the host's 4 MiB `MAX_INBOUND_BYTES`. `maximal extraction stays under the host frame cap` builds the full `start_run` JSON from 1,000,000 CJK characters with interleaved `"` and `\u0001` and checks it.
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
- 2026-10-27: Branch `fix/adopted-contract-repair`. `ConfigStore::update` now reads `adopted-contracts.json` for the frozen-ID check, so a test that adds adapters after damaging or hand-editing that file gets a refusal it did not before: either restore the file or assert the refusal. Checking a refusal's wording against a constant was not enough once the message carried the runtime state path; the native test matches the stable prefix plus `adopted_contracts_path(dir)`, then renames the file aside and checks that notes come back, which is the repair the message promises.
- 2026-10-27: Branch `fix/sandbox-aware-ipc-tests` (`brwr-rq5`). Cookie's health gate had failed every Unix-socket test inside Codex's `workspace-write` sandbox since the service landed, and Cookie read that as a blocked gate for 20+ cycles. The socket tests now probe once and skip on EPERM/EACCES unless `BRAUSER_REQUIRE_IPC_TESTS=1` ("Local IPC in sandboxes"). Checked on macOS: a normal run skips nothing. Under `sandbox-exec` with Unix-socket bind and connect denied, and separately with `BRAUSER_TEST_SIMULATE_IPC_DENIED=1`, the 47 guarded Rust tests (38 unit, 9 `scripted-dialogs` integration) skip with the reason and both Cargo runs pass; with `BRAUSER_REQUIRE_IPC_TESTS=1` the same runs fail exactly those 47. In `test:scripts` the simulated denial skips the 2 guarded child-pipe tests and the probe self-test, and fails them with the flag; Seatbelt leaves child pipes alone, so there only the socket probe self-test skips.
- 2026-10-27: Branch `feat/adopted-state-repair`. `get_config` now refreshes under the config lock, so it records adopted contracts like a note request; a test that wants "never recorded" must avoid `get_config` as well as locked refreshes. Recording warnings come from the reporting host's own last locked attempt, so a host test injects a write failure by making the config directory read-only (0500) after `.config.lock` exists, and an unlocked refresh must leave the warning in place. Settings' confirmation text lives in `options.html`, which the fake DOM does not parse, so the options test checks the markup between `forget-adopted-confirm` and `forget-adopted-yes`. The headless smoke forgets a damaged record through the real page and checks the `.bak` bytes and that no native dialog appeared.
- 2026-10-27: Branch `feat/summary-outputs-pr4`. A summary takes the user's offset from the extension, as captured events do: the panel's `requested_at` (the click's local time with its `±HH:MM` offset, `localTimestamp`) decides which day's log it lands in, and `summarized_at` is the answer's `answered_at` instant rewritten in that offset (`summarized_at_uses_the_click_offset` in host/src/service/finisher.rs), and an unparseable value falls back to UTC. Using the clock at save time instead made every Retry save a different event; stamping `answered_at` once, and re-stamping only when the daily log's 30-day window would refuse it, keeps retries idempotent.
- 2026-10-27: Branch `fix/forget-for-full-record`. Settings decides whether to offer Forget from `adopted_state.full`, not from warning text, so the tests assert both the flag and the wording: a full record's warning mentions Forget recorded adapters, and write-failure and lock-failure warnings must not mention Forget at all.
- 2026-10-27: Branch `fix/smoke-macos-flakes` (`brwr-smk`). Since #98, main's required macOS smoke had stopped three ways, each passing on rerun. The fixes are in "Waiting for state, not time". (1) `Extensions.loadUnpacked timed out` on the `macos-15-intel` fallback: the first CDP request had 15 s to cover Chrome's whole cold start. Reproduced locally with the old runner under 3x CPU load. On the arm64 CI runners, loading plus the profile check already took 12.9 s, and Chrome writes `Preferences` about ten seconds late, so the old 12 s profile wait was nearly as tight. (2) `Expected exactly two claude runs; got []`: a reopened panel enables **Summarize & log** before `restore()` adds earlier cards, so the earlier, already-summarized Codex card was taken for the new run, and the check ran before Claude Code started. Reproduced 2 of 5 under full load, where the fake's log showed both Claude turns starting after the check. (3) `Expected one open of the allowed page in the hourly totals; found []`: never reproduced locally (0 of 8 before the fix, including 4 old runs at 3x load). The step went on once settings looked connected, not once the worker held the new lease. The lease the worker held before matches only `/items/{id:int}`, which gives exactly an empty total. Probes found settings enables its controls only after its lease install, and showed no stale document after `Page.reload`, so this cause is inferred, not shown. The new waits record the worker's lease in the timeout message, so a recurrence will tell a late lease from a product fault. A fourth stop appeared under load: `openPanel()` gave up when one `chrome.sidePanel.open` evaluate timed out. Results: before the fix, 3 of 3 idle and 3 of 5 under full load passed; after, 16 consecutive passes (6 idle, 6 under full load, 4 under 3x load). No product race was found.
- 2026-10-27: Cookie's `brwr-d3m`/`brwr-p9r` cycle. Duration presentation tests should inspect both the worklog and a regenerated artifact file, including numeric frontmatter, rather than only the shared formatter. Panel startup tests now hold hello or `install_policy` to cover opening Recent before connection, during policy installation, and after startup. The connected policy-install window already permits requests; retry only views that actually waited. The fake DOM supplies `Option` and select `add` for real Recent rendering. For focused test diagnostics in a runner whose child test output is opaque, `node --test --test-isolation=none --test-name-pattern='Recent loads once' extension/tests/panel.test.mjs` prints the individual cases; still run the normal complete extension suite for the gate.
- 2026-10-27: Independent review of the brief-activity/startup-Recent cycle reproduced two additional startup failures before fixing them: no home folder left both areas saying "Waiting for the host…" after connection, and a worker policy-install error prevented history loads despite a usable host configuration. Composed-panel regressions now cover setup guidance with Copy disabled and exactly one history/worklog request after that worker failure.
- 2026-10-27: Branch `salvage/cookie-cycles-2-3` (`brwr-p9r` follow-up). A startup Recent test that releases the policy install together with `hello` cannot tell "loads on connect" from "loads after startup finishes"; hold `install_policy` pending and assert the history results arrive first.
- 2026-10-27: Branch `feat/summary-outputs-pr4` (`brwr-t53`). The headless smoke now covers summary outputs end to end ("Summary outputs (headless smoke)" above). The new steps run after `agent.env`, not straight after the first Codex summary, because a panel restores only the newest five finished runs (`MAX_RESTORED_FINISHED`), and Restore expects exactly the five earlier runs; three more runs before it would push the Codex card out. A summary on an unrecognized page has no measured activity, so its artifact's Timeline has no rollup; the rollup order is checked on the recognized page's artifact, which does. As built, `markdown::inline` also escapes `=`, so the `<img>` entity is written `\<img src\=…\>`. Every new wait uses `waitUntil`, and every navigation calls `workerHoldsHostPolicy()` first. Three consecutive runs passed.
- 2026-10-27: Branch `feat/summary-outputs-pr4`, merging #126. Summaries and repository containers are checked in `host/src/index.rs` (`a_summary_within_a_repo_attaches_to_the_contained_artifact`, `a_summary_only_contained_artifact_does_not_inflate_its_container`, `a_summary_of_a_repo_page_attaches_to_the_repository`, `a_version_5_cache_with_containers_is_migrated`) and `host/src/native.rs` (`a_summary_only_pull_request_names_its_repository`). Equal-focus Recent rows sort by title, so a fixture PR titled "Page" sorts before `acme-corp/widgets`. A v5 cache stand-in must change a file without touching its `brauser:` identity, or the regeneration reports a conflict instead of a rewrite.
- 2026-10-27: Branch `fix/smoke-run-card-wait` (`brwr-smr`). PR #135's smoke stopped twice with `Timed out after 30.2s waiting for a new run card after Summarize & log` and once with `Panel shows: null`, under load and while a codex review was rebuilding `extension/dist` in the same worktree. CPU load alone did not reproduce either: 6 of 6 runs passed under full load, and 5 of 5 under 3x load, where every new card appeared within about 1 s (20 of 20). One `npm run build:extension` started at the moment of the Claude Code click reproduced the card timeout exactly. The panel's run line said `Brauser can't read this page's content.`, because `executeScript` could not find `vendor/readability.js` while `dist` was being rewritten. One build at the start of the Claude Code step reproduced the missing-panel form (`Panel shows the URL: (none)`). This is not a product race: an installed extension's files do not disappear. The runner now refuses to start during a build, detects a rebuild of `dist` during the run, and stops the run-card wait at once with the panel's failure line. After the fix: 10 consecutive passes (5 idle, 5 under full load). With the same build at the click, the run now stops in about 1 s with the panel's text and the rebuild note. After merging PR 4 (#96), `cardIn` moved onto `waitUntil` too, with a `diagnose` hook that reads `get_run` once on timeout. That covers t53's `summarizeWithCodex()`. On the merged tree: 11 consecutive passes (5 idle, 6 under full load), including every t53 step.

- 2026-10-28: Assigned adapter-repair and capture-state regressions share
  `host/tests/fixtures/adapter-repair-responses.json`. Its TOML inputs and
  complete wire replies are checked by the Rust `serve_with_io` test and
  consumed by settings tests, so format 0 and 65-adapter repair replies cannot
  drift into synthetic UI-only coverage. The incoming shape guard accepts
  them while the outgoing adapter engine still refuses them. For detailed
  Node failure output, run one extension test file at a time with
  `node --test --test-isolation=none --test-reporter=spec <file>`; combining
  the fake-DOM panel, settings and worker suites without isolation mixes
  their global Chrome/DOM stubs and produces unrelated failures. The normal
  `npm run test:extension` keeps each file isolated. Linux still cannot run
  the macOS headless smoke runner.

Capture Site starter history is covered by
`native::tests::capture_site_preflight_checks_removed_contracts_before_save`
and the built-panel cases in `extension/tests/panel.test.mjs`. The native
fixture removes a custom Jira contract through settings and by hand, checks
the starter before save, permits restoration, and checks an independent fresh
support directory. Adopted contracts belong to the support directory, not a
config filename; another config file in the same directory is not a fresh
installation. Panel tests hold and reorder preflight replies to verify that
edits, invalid selections and closing/reopening cannot reuse an older verdict.
These fixtures use local storage and scripted native replies only.

Incoming navigation (`brwr-1g2`) is covered by the native request test
`artifact_query_exposes_bounded_validated_incoming_sources` and index tests
`incoming_trails_only_show_known_navigation_sources`,
`incoming_trails_are_bounded_sorted_and_deduplicated_after_rebuild`, and
`old_unknown_trail_transitions_preserve_activity_but_never_become_edges`.
They cover known browser transitions, unresolved/self/back-forward sources,
the five-pair cap, deterministic ordering, repeated observations and reindex
from owned markers. The composed panel tests validate links and hostile
text against scripted local source/target fixtures and simulate activating
the linked source; the fake DOM does not implement a browser's anchor
default action. The macOS smoke runner now uses protocol v8 with the same
dialog count; actual Chrome link opening still needs macOS verification.

- 2026-10-28: Independent review reproduced a standalone overflow retry
  starving every newer capture batch when its old log or timestamp is refused.
  Ready activity now drains before standalone retries, while the failed marker
  retains its original ID, snapshot and backoff. The failing-first worker
  regression covers both a false acknowledgement and an older-host error.
