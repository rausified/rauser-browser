# Brauser: The Rauser Browser Assistant

The Rauser Browser Assistant (Brauser) is a local-first browser assistant designed to help people keep useful context from the pages they visit. It uses a browser sidebar, Markdown notes, an optional native host,  and local Agent Harness integrations.

## Status

M0 is merged. M1 development code includes a Chrome side panel, opt-in capture, and native folder selection. The side panel was then reworked (before M2) so page notes are editable and autosaving instead of create-only: the panel follows the active tab, shows its note, and saves the whole note about a second after typing stops, on tab change, and on close. M1.5a now makes configured adapter templates active: only recognized artifact URLs pass the worker's private buffer, and the host independently rechecks every delivery. Settings includes a full adapter review workspace and development starter drafts for enabled Jira Cloud, SharePoint/OneDrive, and scoped GitHub origins; the drafts use cross-engine recorded fixtures but still need the live-tool checks documented beside them before release certification. Automatic capture records hourly artifact activity (cold opens, focused and background time, modes, refs, trails, and explicit `why` context), coalesces under buffer pressure, and delivers day-bounded batches through one compact durable intent and one log append. Durable deliveries update a disposable SQLite artifact index, while owned artifact Markdown regenerates lazily with bounded Timeline rollups; `brauser reindex` rebuilds both from the log. The panel now turns that history into an artifact card, one-line context, a filtered cross-tool Recent view, and copyable daily or weekly worklogs. Host-derived record keys make alternate routes share a note, and settings can regenerate artifact files while reporting unowned-path conflicts. Protocol version 6 gives Brauser standing HTTP(S) access from install: the panel reads any HTTP(S) tab's address and title with no per-site grant, prompt, or gesture, and a note can be typed on any page at once. A site is on exactly when it has an adapter, settings saves with the host's revision check and no native confirmation, and the AI denylist, `log_incognito`, and the harness setup dialog are gone. A knowledge base folder, chosen with the native picker, is the folder a summary agent reads. On macOS the `brauser` binary also runs as an on-demand per-user service (`brauser service`, a LaunchAgent) that runs summary agents: the host forwards run messages to it, starts it when needed, and replaces a service left over from an older build; it stops after 10 idle minutes. It keeps run records, a two-run queue, timeouts, cancel, and recovery, and reads an optional `agent.env` for harness credentials. **Summarize & log** is built on a development branch and not yet merged: settings finds Codex or Claude Code on the service's `PATH` and runs a test prompt through it before using it, and every HTTP(S) page gets a **Summarize & log** button in the panel, a keyboard command, and a context-menu item. Brauser extracts the page's readable text with a pinned, bundled Readability, and the agent runs with full access to your files, on your normal login, in the knowledge base folder. A run card in the panel shows progress, the agent's text, and a checked type, category, and importance line, with Cancel and a way back into the agent's own session. Summaries are not saved to the notes folder yet; the summary file and its log entry are the next step. The extension is an unpacked development build; there is no Chrome Web Store listing or installable release yet. M1 is accepted for development on macOS. Windows is back in scope (DESIGN.md §13, §14) and Windows CI is green, but no Windows run has been accepted yet. It is not yet ready for general download.

Read the [design specification](DESIGN.md) for the architecture, security model, planned features, and release plan.

## Planned support

- Google Chrome Stable on macOS 15 or later (Intel and Apple silicon)
- Edge and other browsers, and Linux, are out of scope; Windows was reinstated on 2026-10-16 and is not yet accepted (DESIGN.md §13, §14)

The release plan calls for the browser extension to be distributed through the Chrome Web Store and a signed native-host installer to be provided for macOS. See [Distribution](DESIGN.md#11-distribution) for details.

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
npm run test:scripts
```

### Try the M1 development build

1. Build the native host with `cargo build --locked -p brauser` and the extension with `npm ci --ignore-scripts && npm run build:extension`.
2. In Google Chrome, open `chrome://extensions`, enable Developer mode, and load `extension/dist/` as an unpacked extension. Copy the extension ID shown there.
3. Register the development host for that ID: run `node scripts/register-dev-host.mjs YOUR_EXTENSION_ID "$(pwd)/target/debug/brauser"` after replacing `YOUR_EXTENSION_ID`. On macOS this also registers the development Brauser service as a per-user LaunchAgent (`~/Library/LaunchAgents/com.rauser.brauser.service.plist`, logging to `~/Library/Logs/Brauser/service.log`) for the same binary. Run it again after a rebuild: it boots out the registered service and bootstraps it again, replacing it. Add `--no-service` to register only the native host, and run `node scripts/register-dev-host.mjs --unregister-service` to remove the development service. The service starts only when the host asks for it.
4. Open the Brauser side panel (the toolbar button opens it). Until setup is complete it warns that Brauser isn't set up; click the gear icon or **Open settings** to open settings in a tab and choose a notes folder.
5. On a Jira Cloud, SharePoint/OneDrive, or GitHub page, return to the panel and click **Capture Site**. Review what it remembers, whether the current page matches, and the safety-check results, then save without opening JSON. GitHub asks separately for the work organizations whose pull requests and issues to remember; uncheck a kind you do not need. The first site starts capture; adding a site while capture is paused keeps it paused. These are development starter drafts pending live-tool certification.
6. Manage existing site rules in settings. Unknown-site automatic drafting is still in development; for other tools, enter a site URL and save an adapter in settings. A site is on exactly when it has an adapter. Settings also offers the same starter drafts in its advanced rule editor, with a match and safety-probe preview before saving.

### Set up a summary harness and Summarize & log

Summaries need the development service from step 3 and an installed, logged-in Codex (`codex`) or Claude Code (`claude`).

1. In settings, under **Summary harness**, read the warning: summaries run an AI agent with full access to your files, on your normal login, and a page could contain instructions aimed at the agent. Summarize pages you trust.
2. Choose a knowledge base folder if it should differ from the notes folder. The agent runs there and reads it to judge how a page fits what you already have; Brauser never writes there.
3. Click **Detect harnesses**. The service looks on its own `PATH` (which includes `/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin`) and lists each harness with its version. Pick one, set the run time limit in minutes (10 by default), and click **Test and use**: the service runs one test prompt through the real invocation and saves the choice only if it answers.
4. Codex uses its stored login under `~/.codex` and needs nothing else. For Claude Code on Bedrock, Vertex, or an API key, click **Reveal agent.env in Finder** and add `KEY=VALUE` lines such as `CLAUDE_CODE_USE_BEDROCK=1`, `AWS_PROFILE=…`, and `AWS_REGION=…`. The file must stay mode 0600; settings shows its names, never its values. It is read at the start of every run.
5. Optionally edit the document types and categories the agent picks from, one per line, and click **Save vocabulary**.
6. On any HTTP(S) page, click **Summarize & log** in the panel, press Command+Shift+9, or choose **Summarize with Brauser** from the page's context menu. The card shows the streamed text, then "Summarized" with the type, category, and importance. **Open in Codex** reopens a Codex run's thread in Codex Desktop; **Copy resume command** copies `cd '<knowledge base>' && claude --resume <session>` for a Claude Code run. Closing the panel does not stop a run; reopening it brings the card back.

The panel updates when settings change, can send pending activity and pause capture, and follows whichever tab is active. On any HTTP(S) page it shows the address and title, loads that page's note (if any), and autosaves what you type. Recognized artifact pages also show measured activity and one-line context; **Recent** combines artifacts across tools and generates a copyable worklog. Brauser holds HTTP(S) site access from install; if you withdraw it in Chrome, the panel shows one banner with a button that opens Brauser's Chrome extension settings so you can turn it back on.

If **Choose folder** is disabled and the settings page says "Native host unavailable," check that the extension ID in the native-host registration matches the ID shown on `chrome://extensions`, rebuild the host at the registered path, then reload the settings page and reopen the panel. The development host registration is per user; the guided macOS smoke run below sets up its own isolated profile automatically.

For a guided macOS Chrome smoke run, build both components as above, then run:

```sh
npm run smoke:macos
```

The runner opens an isolated Chrome profile and a localhost fixture, loads `extension/dist/` through a local Chrome DevTools pipe, and verifies Chrome can exchange a `hello` with the native host. It then drives the side panel, the settings page, and the fixture tab itself, and stops only for the macOS folder picker, which Brauser now uses for both the notes folder and the knowledge base folder. Brauser holds site access from install, so Chrome shows no permission prompt, and there is no other native dialog. Each prompt is named in the terminal, and the runner continues on its own once it sees the answer. Choose any empty folder when asked. At the end it asks whether a busy cursor stayed on screen after a dialog. The debugging flag and pipe apply only to the disposable profile; no remote debugging port is opened. It checks the host configuration, the visit log, page note autosave (including a simulated conflict from a stale save), pausing from the panel and resuming from settings (the panel's "Capture off" strip with no setup warning, **Resume capture** turning capture back on with no dialog, and capture working again), and site removal after each step. The native host uses a temporary `HOME`, and its registration lives only in the isolated Chrome profile; the runner does not change the normal Chrome host registration. It preserves its profile and notes under the printed temporary directory for inspection. Run `node scripts/smoke-macos.mjs --help` for an alternate host or Chrome binary path, `--default-port` to serve the fixture on port 80, or `--auto` to answer every prompt through macOS UI scripting with no person present. `--auto` needs Accessibility access for the terminal (System Settings > Privacy & Security > Accessibility) and the Xcode command-line tools' `swiftc`; it takes keyboard focus while dialogs are open.

For routine checks that put nothing on screen, build the test host with `cargo build --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs` and run `npm run smoke:macos:headless`. It runs the same steps in headless Chrome. The host's dialog child takes each folder-picker answer from a file in the run directory instead of showing the picker; the host still mints and checks every purpose-bound folder token, and the runner checks each picker's title. The number of dialogs the run answers must equal the number the host logged. The extension's install-time HTTP(S) access needs no grant step. The run checks that no test process has a window or Dock icon. It also sets up a summary harness and summarizes pages using only fake `codex` and `claude` scripts in the run directory, which speak just enough of Codex's app-server JSON-RPC and Claude Code's stream-json for the real runners: the host wrapper starts the host and the service with `env -i`, so they see only a temporary `HOME`, a `PATH` of that fake directory plus `/usr/bin:/bin`, and a log file that stands in for `/usr/bin/open`. The run checks detection and a test run, a Codex summary of a fixture article (the extracted page keeps the article and drops its navigation, the thread starts with full access in the knowledge base folder, and no command line carries page text), Open in Codex, a Claude Code run that needs one repair turn and resumes its session, a second invalid answer, a failing harness, Cancel killing the run's process group, cards restored after the panel reopens, and `agent.env` names shown without values. No real harness or model is ever run. Guided and `--auto` runs pin `PATH` the same way but skip these steps. It does not exercise the real picker or the busy-cursor fix, so run `--auto` or the guided run before accepting picker changes. The feature does not compile in release builds, and the runner refuses to use a scripted host outside `--headless`.

To check the Chrome behavior M1.5a artifact logging relies on (new-tab events, transition types, worker stops, idle, and discarded or restored tabs; DESIGN.md §12.2 step 6.1), run `npm run probe:m15a`. It needs no build, runs headless Chrome on a temporary profile with nothing on screen, and exits 1 if Chrome disagrees with the design; `--only <probe>` runs one group and `--json <file>` saves the report.

The development registration in step 3 changes only the current user's Chrome native-host entry. It is for development; the signed installers planned for M5 will own installation and uninstallation. A folder path previously entered directly in an M0 config must be selected again through the native picker before M1 can use it. The host backs up malformed or incompatible config files during a revision-checked repair.

The host binds the chosen folder to its filesystem identity. If that folder is moved or replaced, choose it again before capture resumes. M1 keeps a synced visit-ID intent file in the OS config directory for each saved visit so retries can recover across log dates and interrupted appends. These files remain indefinitely in the development build; compacting the index is part of the public-release work.

Page notes are replaced whole under a version the panel names on every save (`load_note`/`save_note`); a save based on an out-of-date version is refused, and the host returns the note's current content instead of writing anything, so the panel can show it and let the user copy back what it could not save. The host checks the existing file's recorded canonical URL and Brauser ownership before replacing it; a mismatch is left untouched. Automatic capture still checks the original URL against the host's adapters before normalizing or logging it, but notes work on any HTTP(S) page whether or not an adapter covers it. See [protocol/README.md](protocol/README.md) for the message contract.

Removing a site or changing its capture rules keeps one shared note per
artifact on every route the removed or earlier rules recognized, because
config.toml keeps those rules for note keys (never for capture). A page those
rules do not recognize, such as a Jira board whose issue modal shares its
URL once `selectedIssue` is stripped, keeps its own note. Older URL notes stay
separate and viewable. If config.toml no longer records any rule for a site
(a repair replaced it, or the site was removed before Brauser kept those
rules), the host cannot tell that board from its modals, so every page there
uses its own URL note; the shared artifact note is left untouched in the
notes folder. Notes alone do not turn capture back on.

The M1 smoke run has passed on macOS on both a nondefault and the default port, so M1 is accepted for development there. Windows is in scope but has no accepted run yet (DESIGN.md §13, §14); the public package remains a later release milestone.

## License

This repository is licensed under the [Apache License 2.0](LICENSE). The design specifies Apache-2.0 for the planned software as well.
