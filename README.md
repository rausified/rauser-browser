# Brauser: The Rauser Browser Assistant

The Rauser Browser Assistant (Brauser) is a local-first browser assistant designed to help people keep useful context from the pages they visit. It uses a browser sidebar, Markdown notes, a native host for local storage, and optional local AI harness integrations.

## Status

M0 is merged. M1 development code includes a Chrome side panel, opt-in capture, and native folder selection and confirmation. The side panel was then reworked (before M2) so page notes are editable and autosaving instead of create-only: the panel follows the active tab, shows its note, and saves the whole note about a second after typing stops, on tab change, and on close. M1.5a now makes configured adapter templates active: only recognized artifact URLs pass the worker's private buffer, and the host independently rechecks every delivery. Settings includes a full adapter review workspace and development starter drafts for enabled Jira Cloud, SharePoint/OneDrive, and scoped GitHub origins; the drafts use cross-engine recorded fixtures but still need the live-tool checks documented beside them before release certification. Automatic capture records hourly artifact activity (cold opens, focused and background time, modes, refs, trails, and explicit `why` context), coalesces under buffer pressure, and delivers day-bounded batches through one compact durable intent and one log append. Durable deliveries update a disposable SQLite artifact index, while owned artifact Markdown regenerates lazily with bounded Timeline rollups; `brauser reindex` rebuilds both from the log. The panel now turns that history into an artifact card, one-line context, a filtered cross-tool Recent view, and copyable daily or weekly worklogs. Host-derived record keys make alternate routes share a note, and settings can regenerate artifact files while reporting unowned-path conflicts. The extension is an unpacked development build; there is no Chrome Web Store listing or installable release yet. M1 is accepted for development on macOS; Windows is in scope but unvalidated (DESIGN.md §13), so it is not yet ready for general download.

Read the [design specification](DESIGN.md) for the architecture, security model, planned features, and release plan.

The [2026-10-18 review and immediate plan](DESIGN.md#123-review-checkpoint-and-immediate-plan-2026-10-18) distinguishes implementation from acceptance. The newest panel/note slice is on the development branch; live starter certification is pending and daily use has not started. Two reproduced gaps come first: artifact notes become hidden after adapter removal, and a valid stale index cache can omit durable history until reindex. Reconcile the divergent branch plans, preserve the queued SQLite/host hardening work, extend artifact browser coverage, and build Capture Site with safe re-learning before M2. John reconfirmed this direction and macOS/Windows scope on 2026-10-18. Harness setup exists, but page invocation and summaries do not. The current full-history JSON cache and SQLite snapshot are an interim index implementation to replace before long-lived history or M3 search.

## Planned support

- Google Chrome Stable on macOS 15 or later (Intel and Apple silicon)
- Windows is in scope; its version baseline, CI, real Chrome acceptance, harness process containment, and installer are still pending.
- Edge and other browsers, and Linux, are out of scope (DESIGN.md §13)

The release plan calls for the browser extension to be distributed through the Chrome Web Store and signed native-host installers for macOS and Windows. See [Distribution](DESIGN.md#11-distribution) for details.

## Development

The pinned Rust toolchain is in `rust-toolchain.toml`. Python 3 generates protocol types, and Node.js runs the TypeScript build. From the repository root:

```sh
python3 protocol/generate.py --check
python3 scripts/check_current_state.py
cargo build --locked --workspace --all-targets
cargo test --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
npm ci --ignore-scripts
npm run typecheck
npm run lint
npm run build:extension
npm run test:extension
```

### Try the M1 development build

1. Build the native host with `cargo build --locked -p brauser` and the extension with `npm ci --ignore-scripts && npm run build:extension`.
2. In Google Chrome, open `chrome://extensions`, enable Developer mode, and load `extension/dist/` as an unpacked extension. Copy the extension ID shown there.
3. Register the development host for that ID: run `node scripts/register-dev-host.mjs YOUR_EXTENSION_ID "$(pwd)/target/debug/brauser"` after replacing `YOUR_EXTENSION_ID`.
4. Open the Brauser side panel. Until setup is complete it warns that Brauser isn't set up; click the gear icon or **Open settings** to open the settings page in a tab. There, choose a notes folder, enter a site URL and path prefix, then enable the site. Chrome requests the site permission and the native host confirms the capture policy. For Jira Cloud, SharePoint/OneDrive, and GitHub, settings then offers matching starter drafts; GitHub requires the work organizations to capture. Every draft opens in the same rule and safety-probe preview as a custom adapter before native confirmation. The panel updates when settings change; it sends buffered activity and pauses capture. It follows whichever tab is active: on any HTTP(S) page it loads that page's note (if any) and autosaves what you type. Recognized artifact pages also show measured activity and one-line context; **Recent** combines artifacts across tools and generates a copyable worklog. The first note on a page the extension has no standing Chrome access to needs the toolbar button, the **Add a note in Brauser** command (`Ctrl+Shift+9`/`Cmd+Shift+9`), or the page's right-click menu, which grant temporary access to that one tab; the panel can then request the page's exact origin so it keeps working there without repeating the trigger.

If **Choose folder** is disabled and the settings page says "Native host unavailable," check that the extension ID in the native-host registration matches the ID shown on `chrome://extensions`, rebuild the host at the registered path, then reload the settings page and reopen the panel. The development host registration is per user; the guided macOS smoke run below sets up its own isolated profile automatically.

For a guided macOS Chrome smoke run, build both components as above, then run:

```sh
npm run smoke:macos
```

The runner opens an isolated Chrome profile and a localhost fixture, loads `extension/dist/` through a local Chrome DevTools pipe, and verifies Chrome can exchange a `hello` with the native host. It then drives the side panel, the settings page, and the fixture tab itself, and stops only for the steps that need a person: the macOS folder picker, Chrome's permission prompt, and Brauser's confirmation dialog. Each prompt is named in the terminal, and the runner continues on its own once it sees the answer. Choose any empty folder when asked. At the end it asks whether a busy cursor stayed on screen after a dialog. The debugging flag and pipe apply only to the disposable profile; no remote debugging port is opened. It checks the host configuration, Chrome grants, visit log, page note autosave (including a simulated conflict from a stale save), and site removal after each step. The native host uses a temporary `HOME`, and its registration lives only in the isolated Chrome profile; the runner does not change the normal Chrome host registration. It preserves its profile and notes under the printed temporary directory for inspection. Run `node scripts/smoke-macos.mjs --help` for an alternate host or Chrome binary path, `--default-port` to serve the fixture on port 80, or `--auto` to answer every prompt through macOS UI scripting with no person present. `--auto` needs Accessibility access for the terminal (System Settings > Privacy & Security > Accessibility) and the Xcode command-line tools' `swiftc`; it takes keyboard focus while dialogs are open.

For routine checks that put nothing on screen, build the test host with `cargo build --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs` and run `npm run smoke:macos:headless`. It runs the same steps in headless Chrome. The host's dialog child takes each answer from a file in the run directory instead of showing the picker or confirmation; the host still mints and checks every folder and consent grant, and the runner checks the confirmation text. Chrome's permission prompt cannot be answered without UI, so the runner grants the access first through Chrome's own mechanisms: the first load of the extension copy lists `webNavigation` as required, and the site is granted from `chrome://extensions`. The run checks that no test process has a window or Dock icon. It also sets up an AI harness using only fake `claude` and `codex` scripts in the run directory: the host wrapper starts the host with `env -i`, so the host sees only a temporary `HOME`, a `PATH` of that fake directory plus `/usr/bin:/bin`, a fake API key, and one name setup must not pass on. The run checks detection (the fake `codex` passes its version, `exec --help`, and features-list checks, the last only with the empty `CODEX_HOME` the host gives it), a canceled setup, a failing test run, a confirmed setup, adding and removing privacy exclusions, a confirmed Codex setup (its confirmation must say that the user's own Codex instructions and skills apply and that Claude Code is recommended, and its test run must use the reviewed arguments with only `HOME` and `PATH`), and an exclusion saved after the harness is removed, and refuses to click setup if any offer points outside the fakes. No real harness or model is ever run. Guided and `--auto` runs pin `PATH` the same way but skip these steps. It does not exercise the real picker, alert, or Chrome prompt, the first-grant extension restart, or the busy-cursor fix, so run `--auto` or the guided run before accepting dialog or permission changes. The feature does not compile in release builds, and the runner refuses to use a scripted host outside `--headless`.

To check the Chrome behavior M1.5a artifact logging relies on (new-tab events, transition types, worker stops, idle, and discarded or restored tabs; DESIGN.md §12.2 step 6.1), run `npm run probe:m15a`. It needs no build, runs headless Chrome on a temporary profile with nothing on screen, and exits 1 if Chrome disagrees with the design; `--only <probe>` runs one group and `--json <file>` saves the report.

To check that the real macOS confirmation closes by itself, with no click, when the host goes away and when it times out, build with `cargo build --locked -p brauser && cargo build --locked -p brauser-macos-alert --example show_alert` and run `npm run check:macos-alert -- --on-desktop`. It shows the real alert on screen twice, for about 3 seconds each; don't click it. It closes the dialog child's stdin the way the host does, uses a 1-second timeout in place of the host's 260 seconds, and checks that `UserNotificationCenter` has no alert window left. It refuses to run without `--on-desktop`.

The development registration in step 3 changes only the current user's Chrome native-host entry. It is for development; the signed installers planned for M5 will own installation and uninstallation. A folder path previously entered directly in an M0 config must be selected again through the native picker before M1 can use it. The host backs up malformed or incompatible config files during a revision-checked repair.

The host binds the chosen folder to its filesystem identity. If that folder is moved or replaced, choose it again before capture resumes. Current artifact delivery uses one compact in-flight batch intent, one append, and one log sync so retries can recover across log dates and interrupted appends. M1's per-visit intent path remains only to drain older queues; compact delivery already shipped in M1.5a.

Page notes are replaced whole under the version and host-derived record key the panel names on every save (`load_note`/`save_note`); an old version or changed key is refused, and the panel keeps unsaved text for copy-out. The host checks the file's recorded identity and Brauser ownership before replacing it; a mismatch is left untouched. The final panel-close flush is currently best effort, so close/reopen during a pending save remains an acceptance gap; a displayed **Saved** acknowledges a completed save. Visit logging checks the original URL against confirmed policy before normalization, while notes work on any HTTP(S) page regardless of that allowlist. See [protocol/README.md](protocol/README.md) for the message contract.

The M1 smoke run has passed on macOS on both a nondefault and the default port, so M1 is accepted for development there. Windows still needs its own CI and acceptance (DESIGN.md §13); public packages remain a later release milestone.

## License

This repository is licensed under the [Apache License 2.0](LICENSE). The design specifies Apache-2.0 for the planned software as well.
