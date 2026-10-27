# Brauser: The Rauser Browser Assistant contributor instructions

## Current state

M0 is merged. M1 development code adds a Chrome side panel and settings page,
site capture, native folder selection, and durable visit IDs. The side panel
was then reworked, before M2 (DESIGN.md §12.2 step 4): page notes are no longer
create-only with sibling review drafts. The panel follows the active tab, loads
that page's note on any HTTP(S) page, and autosaves the whole note about a
second after typing stops, on tab change, and on panel close, via
`load_note`/`save_note` protocol messages that reuse `get_config`/`update_config`'s
revision-check pattern. A save based on a stale version is refused; the host
returns the note's current content and the panel shows a copy-out of the text
it could not save. The capture housekeeping controls from M1 collapsed into a
one-line, expandable capture strip. The answer card, Summary section, and full
omnibar are not built (M2/M3).

Open access (protocol version 6, PR 1 of
`docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md`)
removed the per-site grant wall. The manifest requests `http://*/*` and
`https://*/*` as install-time `host_permissions` with `webNavigation` and
`scripting` required, and drops `activeTab`; the toolbar action only opens the
panel, and the panel reads the active tab's URL and title on any HTTP(S) page.
If the user withdraws Brauser's site access in Chrome, the panel shows one
site-access banner with a button to re-enable it. Gone with v6: the native Yes/No
confirmation (and the `macos-alert` crate), `confirm_config` and consent tokens,
the AI denylist and its confirmation, `log_incognito`, Chrome grant
reconciliation, the sync caution, and the harness setup dialog and its token.
Settings saves config with the revision check only. A site is on exactly when
it has an adapter: `ConfigSnapshot.sites` is gone, and the host derives capture
origins from the exact origins of a valid adapter document (none while
`adapter_issue` is set). Turning capture on needs only a home folder.
Path-prefix narrowing is dropped. A version 5 `config.toml` loads unchanged and
drops `sites`, `agent_denylist`, `agent_denylist_confirmed`, and `log_incognito`
on its next save. `StorageConfig.knowledge_base_dir` (null means the notes
folder) is the folder a later summary agent reads; it is chosen with the same
native picker, whose token names its `FolderPurpose` (`notes_folder` or
`knowledge_base`) so one selection cannot authorize the other, and the host
records the folder's identity as it does for the home folder.

The home folder (2026-10-27, `brwr-hme`): `storage.root` is the one folder
the user picks, and the content folders inside it are fixed
(`host/src/layout.rs`: `notes/`, `logs/`, `artifacts/`, `summaries/`,
`later/`). The wire `StorageConfig` keeps its `*_dir` fields with those
values, and `config::validate_update` refuses any others, so settings has no
per-folder fields and harness setup no summaries name. An older config.toml
is moved once by `ConfigStore::migrate_layout` (host start, `hello`,
`get_config`, and a same-folder save in `update_with_grants`): under the
config and capture locks it renames each existing older folder no-clobber,
records a `log/` rename in `visit-ids/log-relocations.json` for intent
replay (`CaptureStore::open_original_day_dir`), then writes the fixed names.
A fixed name already present, or a nested, symlinked, or case-variant older
folder, refuses with a `config_issue` and changes nothing; until the move
succeeds the older layout is never used for vault I/O.

Harness setup (M2 step 1, revised in v6) lives in `host/src/harness.rs` and
`host/src/harness_setup.rs`: discovery on an injected search path, bounded runs
(cleared environment, host-owned 0700 work directory, output caps, timeout,
process-group kill), the `--help` flag and Codex feature checks, and the setup
probe, tested with fake harness scripts. `discover_harnesses` offers what the
host found; `set_up_harness` names a detected `harness_id`, and the host detects
that harness again, runs one test prompt, and commits it with no native dialog
and no token. `update_config` cannot set a harness entry; the extension may only
echo it or remove it. Codex setup runs with the user's own `HOME` and
`CODEX_HOME`, so the settings page says the user's Codex instructions and skills
can shape its output (§14). The settings page ("AI harness (optional)") also
shows the full-access warning and the knowledge base folder. No AI command exists yet, so nothing runs a harness beyond setup's own
checks and probe; the argv templates and cleared environment remain until the
service runners replace them (PR 3). The headless smoke run covers setup with
fake harnesses on a pinned `PATH` and a cleared host environment.

The Brauser service skeleton (PR 2 of the open-access spec, DESIGN.md §12.2
step 10.2) is built on `feat/agent-service-pr2` in `host/src/service/`
(built on every Unix, compiled out on Windows; only the launchd glue is
macOS-only, so the service and its tests build and run on Linux, where the
support directory is the XDG config directory `~/.config/brauser` and the host
never starts the service itself: it uses one started with `brauser service`
and otherwise answers `service_unavailable`). On macOS `brauser service` is a per-user
LaunchAgent (`com.rauser.brauser.service`) that listens on a Unix socket in
Brauser's support directory with the native-messaging framing, and exits after
10 idle minutes. The short-lived native host forwards `start_run`, `get_run`,
`list_runs`, `cancel_run`, and `retry_save` (`host/src/service/client.rs`),
resolving the record key itself; it starts a missing service with
`launchctl kickstart`, checks the service's build identity with
`service_hello`, and answers `service_unavailable` when the service is not
installed or stays on another build. A host older than the binary on disk
never restarts the service; it asks for an extension reload instead. The service keeps one record directory
per run under `runs/`, runs at most two at once with one queued-or-running run
per record key, times runs out after 10 minutes, kills the process group on
cancel, marks orphaned runs failed on restart, prunes old records, and overlays
an optional `agent.env` on each run's environment. The panel will poll
`get_run`; there is no host-side streaming protocol. No real harness is
registered yet: only the test-only fake harness (`scripted-dialogs` builds)
runs, so a normal build answers `start_run` with `not_configured` until PR 3.
`node scripts/register-dev-host.mjs` now also writes and bootstraps the dev
LaunchAgent; `--no-service` skips it and `--unregister-service` removes it.

M1.5a is built on top of that. The adapter-template language
(`adapter-templates.md`) is built as a Rust crate (`adapter-templates/`) and a
TypeScript twin (`extension/adapter-templates.ts`) checked against each other
via the shared vectors in `protocol/adapter-templates/vectors.json`
(`npm run test:adapter-diff`). `ConfigSnapshot.adapters` round-trips as
`[adapters] format = 1` plus `[[artifacts]]`; the host validates it with the
Rust engine and reports overlap warnings or an adapter-only policy issue. A
missing table defaults to an empty format-1 document; an unsupported format or
unknown future `[adapters]` key is preserved while automatic capture is
suspended. Existing adapter IDs are frozen until alias migrations exist.
Settings lists each site with an adapter, with its adapter review workspace and
development starter drafts for Jira Cloud, SharePoint/OneDrive, and scoped
GitHub origins; the drafts still require the documented live-tool checks before
release certification.

The enabled adapter document now travels in the worker's revisioned policy
lease. The TypeScript worker validates and compiles it before buffering and
prefilters unrecognized or out-of-scope navigations; the host independently
rechecks each delivered event with the Rust engine before vault I/O. Origins a
pre-v6 build removed or saw revoked are carried into one pending-removal list,
and the worker keeps their adapters out of capture until a host-sent lease no
longer holds them. Automatic
capture records offset-qualified hourly artifact activity instead of creating
new M1 visit rows: cold opens, focused and background time, modes, refs, and
trails freeze into idempotent `activity` events, with append-only `why` events
for explicit context. Older buffered M1 visits still drain after upgrade. The
worker groups up to 64 same-local-day records under a stable batch ID and
overflow snapshot; the host validates the complete batch, then uses one
compact crash-recovery intent, one append, and one log sync. Queue pressure
coalesces compatible undispatched activity without exceeding the byte cap,
lights the toolbar badge at 80%, and leaves durable overflow evidence. The
native host now updates a rebuildable SQLite `index.db` only after durable log
delivery and lazily regenerates ownership-checked artifact Markdown when the
panel's connection closes and on the first connection of each local day
(never on worker-sync or settings connections), checkpointing long passes. `brauser reindex` recreates both from the owned log
markers; a catch-up or rebuild larger than one pass's 64 MiB read bound saves
its progress and resumes on the next pass (DESIGN.md §6.7). The panel now exposes the current artifact card and one-line context,
a cross-tool Recent list, and copyable daily or weekly worklogs. Notes resolve
to host-derived artifact keys (with guarded migration of older URL-keyed
notes), and settings can regenerate artifact files while reporting every
unowned-path conflict it leaves untouched.

After adapter/site removal, recognition narrowing, or while the adapter
document is inert, note keys come from the note-only contracts still recorded
(`[[adapter_id_history]]`, the inert document, and contracts removed from
config.toml by hand, which the host keeps in its own `adopted-contracts.json`
state file; never capture), so every route a contract matched keeps one note
(`ConfigStore::note_key_rule`). An unusable state file makes identity on
every page the current policy does not recognize unavailable. Those contracts alone decide an origin they
cover, so a stripped canonical URL shared with another page (a Jira board and
its issue modal) is never identity evidence, and older-note candidates must
map to the same artifact. An origin with no recorded contract uses the URL
key with no history lookup or note scan: its recorded URLs are stripped forms
and cannot name an artifact, and its URLs never count as older-note
candidates. Ambiguous contracts fall back to the URL key, while an
uncompilable recorded contract refuses loads and saves instead of guessing an
autosave target.

The panel's Capture Site flow now sets up previously unconfigured Jira Cloud,
SharePoint/OneDrive, and GitHub origins using the existing development starter
catalog. It reviews the current page, illustrative examples and fixed safety
probes inline, with separate GitHub pull-request and issue organization lists,
then saves once through revisioned `update_config`. It preserves configured
sites and global pauses, refuses pending local pauses/removals and stale
reviews, and saves old note edits before reloading their host-derived identity.
Installing policy from a page seeds already-open recognized tabs for attention
measurement without inventing cold opens; background renewals retain their
normal navigation accounting.
The separate **Page patterns** panel review now inspects a URL and up to 500
same-page anchors on an explicit click, in the top frame's isolated world.
Its self-contained shaper (`extension/site-patterns.ts`) replaces identifier
and query values with fixed placeholders, keeps only short lowercase route
words, generalizes link-only singleton literals and returns at most 50 link
shapes. Link shapes stay only in the review and are discarded on navigation,
close or collapse. The worker independently samples at most 20 visited shapes
per actively enabled origin for seven days in trusted-only local storage;
settings can show or clear them without the host. Private tabs are excluded,
pauses/removals purge samples, and samples never enter delivery or the vault.
This is the input/review slice of `brwr-t40`, not unknown-site capture: the
host structural drafter, selected-rule enable flow and recurring-page hint
remain unbuilt; starter live-tool certification remains human work.

The service worker delivers buffered events and renews its policy lease
itself, from `alarms` and at browser startup, so capture does not depend on
opening the panel (DESIGN.md §3.1); the panel's Send pending visits asks the
worker to sync now. The rest of the agent integration, search index, signed
installers, Chrome Web Store listing, and public release do not exist yet. M1
still needs interactive Chrome checks on macOS, the primary platform. Windows
support was reinstated on 2026-10-16 (DESIGN.md §13, §14): Windows code must
keep compiling and passing CI, and its interactive checks are separate
[human] work.

DESIGN.md's principle 9 (standalone product) governs this repo: no file, build
step, test, or default here may assume AMOS/ENDURANCE, Backstaff, or Astrolabe
exist, even though development happens inside the ENDURANCE vault. No AMOS
inventory entry or feature briefing exists yet for Brauser (checked via
`grep -ril "brauser\|rauserBrowser" AMOS/inventory AMOS_docs` from the ENDURANCE
root); creating one is a separate follow-up, not covered here. Use
[DESIGN.md](DESIGN.md) for intended behavior and update it when implementation
changes a contract.

## Repository map

- `protocol/schema.json` defines native messages. Run `python3 protocol/generate.py` after changing it and commit the generated Rust and TypeScript files together.
- `adapter-templates/` is the Rust crate implementing the adapter-template grammar specified in `adapter-templates.md`; `extension/adapter-templates.ts` is its TypeScript twin. Keep both in sync with the shared vectors in `protocol/adapter-templates/vectors.json` — run `npm run test:adapter-diff` (`scripts/adapter-templates-diff.mjs`) after changing either engine; CI also checks `adapter-templates/fuzz` compiles.
- `host/` contains the native host. Keep host policy checks on the privileged side of the messaging boundary. `host/src/consent.rs` holds the purpose-bound folder-picker grants and the `update_config` field checks; `host/src/dialog.rs` runs the native folder picker, the only native dialog. There is no `privacy.rs`, `extension/grants.ts`, or `macos-alert/` crate any more (removed with protocol v6).
- Every first-party crate is `#![forbid(unsafe_code)]`.
- `extension/` contains the MV3 Chrome development build. `npm run build:extension` emits `extension/dist/` for unpacked loading.
- Product naming (app name, native host ID, storage key, marker, and temp-file prefixes) lives in `extension/brand.ts` (also used by the build and dev scripts) and `host/src/brand.rs`. Page and manifest text use `{{APP_NAME}}`/`{{FULL_NAME}}`, filled in by `extension/build.mjs`. Crate, package, and binary names in `Cargo.toml` and `package.json` still change separately.
- `scripts/register-dev-host.mjs` registers a per-user development native host for one unpacked extension ID; installers own release registration.
- `.github/workflows/ci.yml` runs the build and quality checks on a `macos-15` and `windows-latest` matrix, with the headless smoke run (`npm run smoke:macos:headless`) and fuzz-target compile check on macOS only; a `Build and check` gate job, the check branch protection requires, passes only when every platform build passes.
- `TESTING.md` is this repo's own testing strategy (headless end-to-end recipe, dialog seams, what headless cannot prove). It is self-contained; nothing in this repo should point outside it for testing guidance.

## Local checks

Run the commands in [README.md](README.md#development) before proposing a code change. The Rust version is pinned in `rust-toolchain.toml`. `deny.toml` is the Rust dependency policy (license allowlist; no wildcard versions, unknown registries, or unknown git sources) that CI's `cargo-deny` job enforces — run `cargo deny check --all-features --locked` before adding or upgrading a Rust dependency. After changing either adapter-template engine, run `npm run test:adapter-diff` (differential testing against the shared vectors, `adapter-templates.md` §12).

For end-to-end checks, use `npm run smoke:macos:headless`; it never shows a window or takes focus. Only when the user asks for it, run `npm run smoke:macos -- --auto` instead (real dialogs, UI scripting; warn them before it takes focus) — `--auto` and `--headless` are mutually exclusive flags of the same script, not combinable. See [TESTING.md](TESTING.md) for what each check (`smoke:macos:headless`, `probe:m15a`) actually does, how to run the real-dialog variants, and what headless cannot prove — it is the single, self-contained source for testing guidance; update it with anything you learn here.

## Security rules

- Treat all extension messages and page-derived content as untrusted.
- Keep vault paths rooted in a user-selected directory; never accept a raw file path from a page or extension request.
- Preserve existing user files. Page notes and generated artifact files are the two implemented file kinds the host replaces whole. A page note is replaced only after checking the caller's version and the file's recorded canonical URL and Brauser ownership (`host/src/note.rs`); an artifact is replaced only after finding exactly one matching `brauser.kind: artifact` and `artifact_id` in its frontmatter (`host/src/index.rs`). An unrelated or unowned file at either path is a conflict, never adopted or overwritten. Every other content location (log, later) stays create-only.
- Keep capture off until a home folder is chosen and the host has a valid adapter document; automatic capture runs only on the exact origins of that document's adapters (`ConfigStore::capture_origins` in host/src/config.rs, `is_authorized` in host/src/capture.rs). Any stored config whose home folder was never confirmed through the native picker, or whose folder no longer matches its recorded identity (unmounted or remounted drive), must block capture and require reselection through the native picker before it can be used again (host/src/config.rs, `DiskState` load path). The knowledge base folder is set only through its own purpose-bound picker token; a picker token is valid only for the `FolderPurpose` it was minted for (host/src/consent.rs). Its identity is checked against the folder when it is chosen and recorded at save time (`update_with_grants` in host/src/config.rs); later saves that leave it unchanged carry the recorded identity without touching the folder, so a missing knowledge base never blocks capture or unrelated settings. The summary agent must re-check that identity before it reads the folder (PR 3).
- The host re-checks every delivered event against its own adapters with the Rust engine before any vault I/O; a worker can only narrow what is recorded, never widen it. Never let a Chrome permission, a worker claim, or extension state substitute for that check. That includes every activity trail source: the host rejects the event rather than drop a trail, so the worker strips trails whose source left the lease before dispatch (`trailsUnder` in extension/worker.ts) and never edits a dispatched event.
- Carry pending site removals across upgrades. Origins an earlier build removed or saw revoked are migrated into the worker's one pending-removal list (`migratePendingRemovals` in extension/worker.ts), their adapters stay out of every capture lease, and the list clears only when a lease the host actually sent holds no adapter on those origins. An unreadable list pauses capture (fail closed).
- Only native harness setup (`set_up_harness`, host/src/harness_setup.rs) may set the harness entry; it names a detected `harness_id`, never a path or arguments, and the host rediscovers and probes that harness before committing. `update_config` may only echo the harness entry unchanged or remove it (host/src/consent.rs `check_agent_fields`).
- Scope every AI harness run's environment to exactly the names discovery offered as required or requested-and-offered for that adapter, and run it with a cleared environment in its own fresh host-owned 0700 work directory, in its own process group killed on timeout, output overflow, or normal exit, under capped stdout/stderr (host/src/harness_setup.rs `requested_env`; host/src/harness.rs `run_bounded`). These lockdown rules hold until the service runners replace them (PR 3 of the open-access spec).
- Before trusting a harness binary — at discovery, setup, probe, and every run — verify it and its parent directory are not symlinks, are owned by the current user or root, and are not group/world-writable, and that its path/size/mtime/inode identity hasn't changed since last checked; treat any mismatch as "harness changed" and refuse the action (host/src/harness.rs `check_install`, `identity`).
- Run a harness only with its single, hardcoded, security-reviewed argument template for the detected adapter and (for Codex) version; refuse an unreviewed Codex version outright, and never pass page or prompt content as a command-line argument — only as standard input (host/src/harness.rs `template_args`, `template_matches`, `codex_args`).
- The Brauser service (`host/src/service/`) runs as the user, never root, as a per-user LaunchAgent started on demand. Its support directory must be a real directory owned by the user with mode 0700 (refuse a symlink or another owner, `ServicePaths::ensure_private`), its socket is mode 0600, and the service refuses a support path too long for a Unix socket rather than moving the socket elsewhere. Only the native host talks to it; the host accepts no `service_hello` from the extension and always chooses the run's record key itself.
- `agent.env` is read at the start of every run and must be a regular, non-symlink file of mode 0600, or the run fails before it starts (host/src/service/agent_env.rs). Never put a value from it in an error, log line, protocol message, or settings view; settings may show its names only.
- Before forwarding a run message, the host compares the service's `service_hello` `build_id` with its own and restarts a stale service once; a persistent mismatch is `service_unavailable`, never a request sent to old code (host/src/service/client.rs).
- Tests never call `launchctl` or touch the real `~/Library`: they use temp support directories, an injected launch function, and the test-only `BRAUSER_SERVICE_SPAWN` seam (scripted-dialogs builds only) that spawns the service directly. `scripts/register-dev-host.mjs` takes an injectable `run` for the same reason.
- Do not claim an installer, browser extension, or agent mode works until it has been built and exercised.
