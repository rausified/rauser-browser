# Brauser: The Rauser Browser Assistant contributor instructions

## Current state

M0 is merged. M1 development code adds a Chrome side panel and settings page,
opt-in site capture, native folder selection and consent, and durable visit IDs.
The side panel was then reworked, before M2 (DESIGN.md §12.2 step 4): page
notes are no longer create-only with sibling review drafts. The panel follows
the active tab, loads that page's note on any HTTP(S) page (not gated by the
logging allowlist), and autosaves the whole note about a second after typing
stops, on tab change, and on panel close, via `load_note`/`save_note` protocol
messages (protocol version 3) that reuse `get_config`/`update_config`'s
revision-check pattern. A save based on a stale version is refused; the host
returns the note's current content and the panel shows a copy-out of the text
it could not save. The first note on a page the extension has no standing
Chrome access to starts from the toolbar action, the `note` keyboard command,
or a page context-menu entry, which brings `activeTab` and `contextMenus`
forward from M2 (and raises `minimum_chrome_version` to 116). The capture
housekeeping controls from M1 collapsed into a one-line, expandable capture
strip. The answer card, Summary section, and full omnibar are not built
(M2/M3).

M2 step 1 (harness setup) is built. Protocol version 4 adds `summaries_dir`,
the agent denylist and its host-only confirmation, `log_incognito` (always
false), a host-validated harness entry, `agent_status` in `get_config`, and a
`check_agent` readiness gate (`host/src/privacy.rs` holds denylist validation
and matching). `host/src/harness.rs` holds harness discovery on an injected
search path, bounded runs (cleared environment, host-owned 0700 work directory,
output caps, timeout, process-group kill), the `--help` flag and Codex feature
checks, and the setup probe, tested with fake harness scripts.
`discover_harnesses` and `confirm_harness_setup` wire it to the protocol: the
host offers what it found, shows a native confirmation, runs the probe, and
mints the single-use harness token that alone can set the harness entry and
`agent_denylist_confirmed` (`host/src/consent.rs`). Codex setup runs
`codex --version`, `codex exec --help`, and `codex features list` with the
reviewed extra disables, under the user's own `HOME` and `CODEX_HOME` (passed
whenever the host has it); features list gets an empty host-owned `CODEX_HOME`
instead, because runs ignore the user's `config.toml`. Every harness run gets
its own new work directory, removed afterwards; its confirmation and the
settings page say that the user's Codex instructions and skills can shape its
output (§14). The settings page ("AI harness (optional)") drives setup: it
detects harnesses, lets the user pick optional environment names and a
summaries folder, edits the AI privacy list (`extension/agent.ts` normalizes
entries; suggestions only fill the box), and removes the harness. No AI
command exists yet, so nothing runs a harness beyond setup's own checks and
probe. The headless smoke run covers setup with fake harnesses on a pinned
`PATH` and a cleared host environment.

M1.5a is in progress on protocol version 5. The adapter-template language
(`adapter-templates.md`) is built as a Rust crate (`adapter-templates/`) and a
TypeScript twin (`extension/adapter-templates.ts`) checked against each other
via the shared vectors in `protocol/adapter-templates/vectors.json`
(`npm run test:adapter-diff`). `ConfigSnapshot.adapters` round-trips as
`[adapters] format = 1` plus `[[artifacts]]`; the host validates it with the
Rust engine and enabled site origins, reports overlap warnings or an
adapter-only policy issue, and puts adapter additions and edits behind native
confirmation (including the every-account disclosure for unscoped shared
origins). A missing table defaults to an empty format-1 document; an
unsupported format or unknown future `[adapters]` key is preserved while
automatic capture is suspended.
Existing adapter IDs are frozen until alias migrations exist. Settings has the
adapter review workspace and development starter drafts for Jira Cloud,
SharePoint/OneDrive, and scoped GitHub origins; the drafts still require the
documented live-tool checks before release certification.

The confirmed adapter document now travels in the worker's revisioned policy
lease. The TypeScript worker validates and compiles it before buffering and
prefilters unrecognized or out-of-scope navigations; the host independently
rechecks each delivered event with the Rust engine before vault I/O. Automatic
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
markers. The artifact panel card, Recent list, worklog view, record keys, and
the settings-facing update-files action remain unfinished.

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
- `host/` contains the native host. Keep host policy checks on the privileged side of the messaging boundary.
- `macos-alert/` is the host's only unsafe code: a small macOS-only wrapper around `CFUserNotification`, so the Yes/No confirmation can time out and be canceled when the host goes away. The alert is drawn by `UserNotificationCenter`, so killing the dialog process does not close it. Keep new unsafe code there, with a `SAFETY` comment per block; every other crate stays `#![forbid(unsafe_code)]`.
- `extension/` contains the MV3 Chrome development build. `npm run build:extension` emits `extension/dist/` for unpacked loading.
- Product naming (app name, native host ID, storage key, marker, and temp-file prefixes) lives in `extension/brand.ts` (also used by the build and dev scripts) and `host/src/brand.rs`. Page and manifest text use `{{APP_NAME}}`/`{{FULL_NAME}}`, filled in by `extension/build.mjs`. Crate, package, and binary names in `Cargo.toml` and `package.json` still change separately.
- `scripts/register-dev-host.mjs` registers a per-user development native host for one unpacked extension ID; installers own release registration.
- `.github/workflows/ci.yml` runs build and quality checks on macOS, the only supported platform, including the headless smoke run (`npm run smoke:macos:headless`).
- `TESTING.md` is this repo's own testing strategy (headless end-to-end recipe, dialog seams, what headless cannot prove). It is self-contained; nothing in this repo should point outside it for testing guidance.

## Local checks

Run the commands in [README.md](README.md#development) before proposing a code change. The Rust version is pinned in `rust-toolchain.toml`. `deny.toml` is the Rust dependency policy (license allowlist; no wildcard versions, unknown registries, or unknown git sources) that CI's `cargo-deny` job enforces — run `cargo deny check --all-features --locked` before adding or upgrading a Rust dependency. After changing either adapter-template engine, run `npm run test:adapter-diff` (differential testing against the shared vectors, `adapter-templates.md` §12).

For end-to-end checks, use `npm run smoke:macos:headless`; it never shows a window or takes focus. Only when the user asks for it, run `npm run smoke:macos -- --auto` instead (real dialogs, UI scripting; warn them before it takes focus) — `--auto` and `--headless` are mutually exclusive flags of the same script, not combinable. See [TESTING.md](TESTING.md) for what each check (`smoke:macos:headless`, `check:macos-alert`, `probe:m15a`) actually does, how to run the real-dialog variants, and what headless cannot prove — it is the single, self-contained source for testing guidance; update it with anything you learn here.

## Security rules

- Treat all extension messages and page-derived content as untrusted.
- Keep vault paths rooted in a user-selected directory; never accept a raw file path from a page or extension request.
- Preserve existing user files. Page notes and generated artifact files are the two implemented file kinds the host replaces whole. A page note is replaced only after checking the caller's version and the file's recorded canonical URL and Brauser ownership (`host/src/note.rs`); an artifact is replaced only after finding exactly one matching `brauser.kind: artifact` and `artifact_id` in its frontmatter (`host/src/index.rs`). An unrelated or unowned file at either path is a conflict, never adopted or overwritten. Every other content location (log, later) stays create-only.
- Keep capture off until a folder is chosen, Chrome grants the requested site origin, and the native host confirms the policy. Any stored config whose notes folder was never confirmed through the native picker, or whose folder no longer matches its recorded identity (unmounted or remounted drive), must block capture and require reselection through the native picker before it can be used again (host/src/config.rs, `DiskState` load path).
- Keep the AI privacy denylist fail-closed: a page whose URL cannot be parsed, uses a non-HTTP(S) scheme, or has no classifiable host is always denied to agents, never allowed by default (host/src/privacy.rs `denylisted`); accept only normalized, punycode hostnames or bracketed IP literals as entries (`validate_denylist`). Only native harness setup may set or change `agent_denylist_confirmed` or the harness entry — the extension may only echo them unchanged or remove the harness (host/src/consent.rs `check_agent_fields`); removing or replacing a denylist entry always needs a fresh native confirmation, adding one does not (`approval_lines`).
- A harness setup token is single-use, expires after a few minutes, and is rejected if the revision, the exact before/after config, or the on-disk identity of the confirmed harness binary no longer matches; never combine it with a folder-picker or settings-consent token (host/src/consent.rs `confirm_harness_setup_with`, `mint_harness`, `check_harness`).
- Scope every AI harness run's environment to exactly the names discovery offered as required or requested-and-offered for that adapter, and run it with a cleared environment in its own fresh host-owned 0700 work directory, in its own process group killed on timeout, output overflow, or normal exit, under capped stdout/stderr (host/src/consent.rs `requested_env`; host/src/harness.rs `run_bounded`).
- Before trusting a harness binary — at discovery, confirmation, probe, and every authorized run — verify it and its parent directory are not symlinks, are owned by the current user or root, and are not group/world-writable, and that its path/size/mtime/inode identity hasn't changed since last checked; treat any mismatch as "harness changed" and refuse the action (host/src/harness.rs `check_install`, `identity`).
- Run a harness only with its single, hardcoded, security-reviewed argument template for the detected adapter and (for Codex) version; refuse an unreviewed Codex version outright, and never pass page or prompt content as a command-line argument — only as standard input (host/src/harness.rs `template_args`, `template_matches`, `codex_args`).
- Do not claim an installer, browser extension, or agent mode works until it has been built and exercised.
