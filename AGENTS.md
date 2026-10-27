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
`adapter_issue` is set). Turning capture on needs only a notes folder.
Path-prefix narrowing is dropped. A version 5 `config.toml` loads unchanged and
drops `sites`, `agent_denylist`, `agent_denylist_confirmed`, and `log_incognito`
on its next save. `StorageConfig.knowledge_base_dir` (null means the notes
folder) is the folder a later summary agent reads; it is chosen with the same
native picker, whose token names its `FolderPurpose` (`notes_folder` or
`knowledge_base`) so one selection cannot authorize the other, and the host
records the folder's identity as it does for the notes root.

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
shows the full-access warning, the summaries folder, and the knowledge base
folder. No AI command exists yet, so nothing runs a harness beyond setup's own
checks and probe; the argv templates and cleared environment remain until the
service runners replace them (PR 3). The headless smoke run covers setup with
fake harnesses on a pinned `PATH` and a cleared host environment.

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
delivery and lazily regenerates ownership-checked artifact Markdown at
connection boundaries. `brauser reindex` recreates both from the owned log
markers. The panel now exposes the current artifact card and one-line context,
a cross-tool Recent list, and copyable daily or weekly worklogs. Notes resolve
to host-derived artifact keys (with guarded migration of older URL-keyed
notes), and settings can regenerate artifact files while reporting every
unowned-path conflict it leaves untouched.

The service worker delivers buffered events and renews its policy lease
itself, from `alarms` and at browser startup, so capture does not depend on
opening the panel (DESIGN.md §3.1); the panel's Send pending visits asks the
worker to sync now. The rest of the agent integration, search index, signed
installers, Chrome Web Store listing, and public release do not exist yet. M1
still needs interactive Chrome checks on macOS, the only supported platform
(DESIGN.md §13; Windows support was dropped, §14).

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
- `.github/workflows/ci.yml` runs build and quality checks on macOS, the only supported platform, including the headless smoke run (`npm run smoke:macos:headless`).
- `TESTING.md` is this repo's own testing strategy (headless end-to-end recipe, dialog seams, what headless cannot prove). It is self-contained; nothing in this repo should point outside it for testing guidance.

## Local checks

Run the commands in [README.md](README.md#development) before proposing a code change. The Rust version is pinned in `rust-toolchain.toml`. `deny.toml` is the Rust dependency policy (license allowlist; no wildcard versions, unknown registries, or unknown git sources) that CI's `cargo-deny` job enforces — run `cargo deny check --all-features --locked` before adding or upgrading a Rust dependency. After changing either adapter-template engine, run `npm run test:adapter-diff` (differential testing against the shared vectors, `adapter-templates.md` §12).

For end-to-end checks, use `npm run smoke:macos:headless`; it never shows a window or takes focus. Only when the user asks for it, run `npm run smoke:macos -- --auto` instead (real dialogs, UI scripting; warn them before it takes focus) — `--auto` and `--headless` are mutually exclusive flags of the same script, not combinable. See [TESTING.md](TESTING.md) for what each check (`smoke:macos:headless`, `probe:m15a`) actually does, how to run the real-dialog variants, and what headless cannot prove — it is the single, self-contained source for testing guidance; update it with anything you learn here.

## Security rules

- Treat all extension messages and page-derived content as untrusted.
- Keep vault paths rooted in a user-selected directory; never accept a raw file path from a page or extension request.
- Preserve existing user files. Page notes and generated artifact files are the two implemented file kinds the host replaces whole. A page note is replaced only after checking the caller's version and the file's recorded canonical URL and Brauser ownership (`host/src/note.rs`); an artifact is replaced only after finding exactly one matching `brauser.kind: artifact` and `artifact_id` in its frontmatter (`host/src/index.rs`). An unrelated or unowned file at either path is a conflict, never adopted or overwritten. Every other content location (log, later) stays create-only.
- Keep capture off until a notes folder is chosen and the host has a valid adapter document; automatic capture runs only on the exact origins of that document's adapters (`ConfigStore::capture_origins` in host/src/config.rs, `is_authorized` in host/src/capture.rs). Any stored config whose notes folder was never confirmed through the native picker, or whose folder no longer matches its recorded identity (unmounted or remounted drive), must block capture and require reselection through the native picker before it can be used again (host/src/config.rs, `DiskState` load path). The knowledge base folder follows the same rule, and a picker token is valid only for the `FolderPurpose` it was minted for (host/src/consent.rs).
- The host re-checks every delivered event against its own adapters with the Rust engine before any vault I/O; a worker can only narrow what is recorded, never widen it. Never let a Chrome permission, a worker claim, or extension state substitute for that check.
- Carry pending site removals across upgrades. Origins an earlier build removed or saw revoked are migrated into the worker's one pending-removal list (`migratePendingRemovals` in extension/worker.ts), their adapters stay out of every capture lease, and the list clears only when a lease the host actually sent holds no adapter on those origins. An unreadable list pauses capture (fail closed).
- Only native harness setup (`set_up_harness`, host/src/harness_setup.rs) may set the harness entry; it names a detected `harness_id`, never a path or arguments, and the host rediscovers and probes that harness before committing. `update_config` may only echo the harness entry unchanged or remove it (host/src/consent.rs `check_agent_fields`).
- Scope every AI harness run's environment to exactly the names discovery offered as required or requested-and-offered for that adapter, and run it with a cleared environment in its own fresh host-owned 0700 work directory, in its own process group killed on timeout, output overflow, or normal exit, under capped stdout/stderr (host/src/harness_setup.rs `requested_env`; host/src/harness.rs `run_bounded`). These lockdown rules hold until the service runners replace them (PR 3 of the open-access spec).
- Before trusting a harness binary — at discovery, setup, probe, and every run — verify it and its parent directory are not symlinks, are owned by the current user or root, and are not group/world-writable, and that its path/size/mtime/inode identity hasn't changed since last checked; treat any mismatch as "harness changed" and refuse the action (host/src/harness.rs `check_install`, `identity`).
- Run a harness only with its single, hardcoded, security-reviewed argument template for the detected adapter and (for Codex) version; refuse an unreviewed Codex version outright, and never pass page or prompt content as a command-line argument — only as standard input (host/src/harness.rs `template_args`, `template_matches`, `codex_args`).
- Do not claim an installer, browser extension, or agent mode works until it has been built and exercised.
