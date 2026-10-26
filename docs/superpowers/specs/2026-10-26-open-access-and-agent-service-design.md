# Open access, the Brauser service, and Summarize & log — design

Date: 2026-10-26 · Status: proposed · Supersedes parts of DESIGN.md §2, §5.2, §5.3, §7.1, §7.3, §8, and §12.2 step 8 (listed in "Documentation changes" below)

## 1. Why

Brauser keeps a wall between itself and the browser. It holds no site access until the user grants each origin, the side panel cannot see an ungranted tab's address, privacy-widening changes need a native confirmation, and an AI denylist gates every agent run. In daily use the wall is the product's main failure: on 2026-10-26 the user could not add a note to the page in front of them; the panel said "Brauser can't see this tab's address yet. Click the Brauser toolbar button, press its keyboard shortcut, or right-click the page…".

The selective-capture and privacy machinery is being removed. Brauser will be able to see and act on any page at any time. What it *records automatically* stays narrow: adapters still decide what is captured, and only on sites the user turns on. Everything else is on demand, from two buttons that work on every page:

- **Note** — the existing side-panel page note (§5.3), which reappears whenever the user returns to the page. No more toolbar/shortcut/context-menu workaround.
- **Summarize & log** — runs a headless agent, in the way the user's DISCOVERY extension does, that summarizes the page against the user's knowledge base, writes a summary file, and logs it.

Agent runs are owned by a new per-user OS service (a macOS launchd agent) installed with the native host, which replaces Backstaff's role in DISCOVERY.

## 2. Goals, non-goals, anti-goals

**Goals**

1. The side panel always knows the active tab's URL and title on any HTTP(S) page, with no per-site grant, gesture, or prompt.
2. A note can be typed on any HTTP(S) page immediately.
3. Automatic capture happens only through adapters, only on sites the user has turned on; turning a site on is just adding its adapter.
4. One click summarizes any page with a full-capability agent (Codex or Claude Code) on the user's normal login, and the result is a structured summary file plus a daily-log entry.
5. Agent runs survive the panel closing and Chrome quitting, and the panel can reattach to them.

**Non-goals**

- Changing the adapter-template engine, frozen adapter IDs, or the append-only log format beyond adding a summary event.
- The omnibar, related pages, read later, FTS5 (M3/M4) — untouched.
- A Windows equivalent of the service (deferred; see §9).
- Moving the activity index's writes into the service (possible later; see §9).

**Anti-goals** (it would be a failure even if it technically worked)

- Any remaining prompt, grant step, or "Brauser can't see this tab" state before a note or summary on an ordinary HTTP(S) page.
- Automatic capture on a site the user never turned on.
- A summary that silently disappears: a failed or canceled run must still be visible in the panel with its reason.
- Agent text written into the user's page note.
- Any dependency on AMOS/ENDURANCE, Backstaff, Astrolabe, or DISCOVERY code (DESIGN.md principle 9 still governs). DISCOVERY is prior art only.

## 3. Decisions (made by the user)

1. **Standing access.** The manifest requests `http://*/*` and `https://*/*` as install-time `host_permissions`. Chrome's single install warning ("read and change all your data on all websites") is accepted.
2. **Remove the privacy and selective-capture ideas:** per-site Chrome grants as a gate, the native confirmation for privacy-widening changes, the AI denylist and its confirmation, and grant-revocation reconciliation.
3. **Capture through adapters only, on sites the user enables.**
4. **Notes** are the existing side-panel page note, available on every page.
5. **Summarize & log works like DISCOVERY:** a full-capability agent thread on the user's own login (for Codex, a `codex app-server` thread with `approvalPolicy: never` and full access, reopenable in Codex Desktop). Not the tool-less, cleared-environment harness in DESIGN.md §7.1.
6. **Claude Code stays as a second harness option** beside Codex.
7. **A per-user OS service manages agent jobs** (macOS launchd agent), installed alongside the native host — chosen over detached runner processes because it gives agent runs a stable environment (credentials), one owner for run state, and a home for later background work.
8. **Summary content:** metadata (date/time, document type, document category), key entities (technologies, products, people, companies), a brief description, and an assessment of the document's importance in light of the user's knowledge base.

## 4. What is removed

| Area | Today | After |
|---|---|---|
| Manifest | `optional_host_permissions` for HTTP(S); `optional_permissions: ["webNavigation"]`; `activeTab` | `host_permissions: ["http://*/*", "https://*/*"]`; `webNavigation` and `scripting` required; `activeTab` dropped |
| Panel | "can't see this tab's address" state; note start from toolbar/shortcut/context menu (`extension/panel.ts` ~L742) | Panel reads the active tab's URL/title directly on any HTTP(S) page; the toolbar action just opens the panel |
| Note origins | `extension/grants.ts` tracks origins granted for notes vs logging | Deleted |
| Grant reconciliation | Worker and settings narrow sites/adapters when Chrome revokes an origin (`worker.ts` `reconcileGrants`, `withSites`; settings revocation path) | Deleted. If the user withdraws Brauser's site access in Chrome, the panel shows one "Brauser needs site access — re-enable in Chrome" banner; no per-site logic |
| Site allowlist as a separate gate | `sites` list + Chrome grant + adapter must all agree | A site is "on" when it has at least one enabled adapter. The builder may keep `sites` as derived data or fold it into adapters (protocol v6) |
| Settings site form | Site URL + "Allowed path prefix" + Enable this site; the prefix (removed from the form 2026-10-26, now always `/`) duplicated adapter URL matching and blocked starter adapters when narrowed | Deleted. Settings lists adapters with on/off and keeps Pause capture; `path_prefix` leaves the protocol in v6 |
| Native confirmation for config changes | `consent.rs` `confirm_config`, `approval_lines`, `mint_consent`, `site_covers`, unscoped-origin "every account" disclosure | Deleted. Settings saves adapters and other config with the existing revision check only |
| AI denylist | `privacy.rs`, `agent_denylist`, `agent_denylist_confirmed`, `check_agent`'s denylist gate, settings "AI privacy list" (`extension/agent.ts`) | Deleted, including the protocol fields |
| Harness lockdown | Cleared env + allowlisted names, reviewed-argv templates, Codex version refusal and feature-disable list, event allowlist / `rejected`, host-owned 0700 throwaway work dir, single-use harness token with native confirmation | Replaced by §6. Harness discovery and the `--version` probe are kept |
| Sync caution | Warning when the notes folder is in iCloud/OneDrive/etc. | Deleted |
| `log_incognito` | Always false | Key removed; the manifest keeps `"incognito": "not_allowed"` |

**Kept unchanged:** the native folder picker and its single-use selection token for `storage.root` (it proves which folder Brauser may write, not a privacy choice); vault confinement and every file-ownership rule (page notes, artifact files, summary files never adopt or clobber unowned files); the adapter engines, frozen IDs, and the host's independent re-check of each delivered event against the adapters (it keeps automatic capture to enabled adapters even if the worker misbehaves); `strip_params` URL normalization; the append-only log; neutralizing remote-resource Markdown in agent output before it is written (it stops a Markdown viewer from fetching page-supplied images).

## 5. The Brauser service

### 5.1 Shape

- **Binary.** The same Rust `brauser` binary gains a `brauser service` subcommand. No new language or runtime.
- **Install.** A per-user LaunchAgent plist in `~/Library/LaunchAgents/`, label derived from `host/src/brand.rs`, bootstrapped into `gui/$UID`. It runs as the user, never root. Release installs register it with `SMAppService` from the signed app bundle (M5); development uses an extension of `scripts/register-dev-host.mjs` that writes the plist and runs `launchctl bootstrap` / `bootout`.
- **Socket.** A Unix domain socket in Brauser's support directory (`~/Library/Application Support/<App>/`, mode 0700). Messages use the same length-prefixed JSON framing and generated protocol types as native messaging.
- **On demand.** Not always running. When the native host needs the service and cannot connect, it runs `launchctl kickstart gui/$UID/<label>` and retries for a few seconds. The service exits after 10 minutes with no queued or running jobs. (Builder's choice: launchd socket activation is acceptable instead, but calling `launch_activate_socket` needs FFI, and unsafe code may only live in a small dedicated crate like `macos-alert/`, with a `SAFETY` comment per block.)
- **Environment.** The plist sets `HOME`, `USER`, `LANG`, and a `PATH` that includes `/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin`. At the start of every run the service also reads an optional `agent.env` file (KEY=VALUE lines, mode 0600, in Brauser's config directory) and overlays it. This is where a Claude Code on Bedrock user puts `CLAUDE_CODE_USE_BEDROCK`, `AWS_PROFILE`, `AWS_REGION`, etc. Codex needs nothing there; it uses its stored login under `~/.codex`. Settings shows which names are set, never values, and has a "Reveal agent.env in Finder" button. This resolves DESIGN.md §14's open question "where a Chrome-spawned host gets auth values" and backlog `brwr-9yc`.
- **Native host role.** The Chrome-spawned host stays short-lived and forwards run messages to the service. If the service is not installed, run messages fail with a specific `service_unavailable` error that the panel turns into setup instructions. Notes, capture, and settings do not depend on the service.

### 5.2 Jobs

- **Run record.** One directory per run under `<support>/runs/<run_id>/`: `status.json` (state `queued | running | succeeded | failed | canceled`, harness, PID/process group, heartbeat, thread or session ID, record key, URL, title, timestamps, error), `page.md` (extracted content), `stream.txt` (text so far), `result.json` (the validated structured answer), and `run.log` (harness stdout/stderr for diagnostics).
- **Queue.** At most 2 runs execute at once; the rest wait FIFO. At most one queued-or-running run per record key: a second click on the same page returns the existing `run_id` and the panel reattaches.
- **Timeout and cancel.** Default 10-minute timeout (configurable). Cancel and timeout kill the run's process group. Panel close and Chrome quitting do not cancel.
- **Recovery.** On start, the service marks any `running` record whose process is gone as `failed` with "service restarted".
- **Retention.** Keep run directories for 30 days or the newest 200, whichever is more; summary files and log entries are permanent and unaffected.
- **Config at write time (designer's call).** The service writes under the configuration current *when the run finishes*, not the one at `start_run`: it re-reads config under the config lock, re-derives the record key from the run's URL, and writes to the current `summaries_dir`. If the notes folder is gone or unconfirmed, the run fails with Retry save. A key that changed mid-run (adapter edit) is followed, not refused; `status.json` records both the start key and the written key.
- **Selection (designer's call).** `start_run`'s optional selection is passed to the agent as extra context ("the user highlighted this") inside `page.md`; it does not create a separate summary. One summary per record key.
- **Finishing.** On success the service, not the agent, writes the summary file (§7.1) and appends the summary log event (§7.2) through the existing vault writers and ownership checks. If the write fails, the run is `failed` with the reason and `result.json` stays so the panel can offer Retry save (no rerun) and Copy.

### 5.3 Messages (protocol v6)

Extension → host → service: `start_run` (record key resolved by the host, URL, title, extracted content, optional selection) → `run_id` (new or existing); `get_run` (`run_id`, `since_offset`) → state + new stream text + result/summary path; `list_runs` (recent and in-flight, for restoring cards); `cancel_run`; `retry_save`. Streaming is by polling `get_run` about once a second while a card is visible, so no host-side streaming protocol or concurrent message loop is needed (this replaces DESIGN.md §12.2 step 8.3 and backlog `brwr-jfe` / `brwr-0cj`). Every response stays under the existing 900 KiB outbound cap; `get_run` pages long stream text by offset.

## 6. Agent runs

### 6.1 Common

- **Working directory:** the **knowledge base folder** — a new setting, chosen once with the native folder picker (same token flow as `storage.root`), defaulting to `storage.root`. The agent reads it to judge importance.
- **Input:** the prompt (§6.4) names `page.md` by absolute path; page text never goes on a command line. The prompt marks the page content as untrusted data that may contain instructions to ignore.
- **Harness setup in settings:** detect installed `codex` and `claude` on the service's `PATH`, show their versions, choose a default, and run a probe (a fixed tiny prompt through the real invocation) that must succeed before the button is enabled. No native confirmation dialog and no single-use token.
- **Setup warning (shown once, and in settings):** "Summaries run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Summarize pages you trust."

### 6.2 Codex

Mirror DISCOVERY's `execute_app_thread_request` (reference only, for the `codex app-server` handshake, notification shapes, turn completion, and interrupt; read it at `~/ENDURANCE/tools/chrome-endurance-extension/native_host/endurance_native_host.py`, around line 5516, plus its `CodexAppServerSession` and `launch_codex_app_server_process`; copy no code or dependency from it): start `codex app-server`, `initialize`, `thread/start` with `cwd` = knowledge base folder, `approvalPolicy: "never"`, `sandbox: "danger-full-access"`; `thread/name/set` to "Brauser: <page title>"; `turn/start` with the prompt. Collect `agent_message` text into `stream.txt` as it arrives. Use the user's own `HOME` and `CODEX_HOME`. Record the thread ID; the run card offers **Open in Codex** (`codex://threads/<id>`). If the installed app-server accepts an output schema on `turn/start`, pass §6.4's schema; otherwise rely on the prompt and validation.

### 6.3 Claude Code

`claude -p --output-format stream-json --verbose --dangerously-skip-permissions` (the equivalent of Codex's full access; the builder confirms flag names against `claude --help` for the installed version), `cwd` = knowledge base folder, prompt on stdin. Stream assistant text deltas into `stream.txt`. Success requires exit 0 and a final `result` event with `is_error: false`. Record the session ID from the `system/init` event; the run card offers **Copy resume command** (`cd <kb> && claude --resume <id>`).

### 6.4 Prompt and structured answer

The agent's final message must be one JSON object:

```json
{
  "document_type": "report | announcement | ticket | specification | email | article | … (open)",
  "category": "technical specification | product plan | administrative | project plan | marketing material | research paper | news | … (open)",
  "entities": {
    "technologies": ["…"], "products": ["…"], "people": ["…"], "companies": ["…"]
  },
  "description": "2–5 sentences: what the page contains.",
  "importance": { "level": "high | medium | low", "reason": "one line" },
  "assessment": "A short paragraph: why this matters, or doesn't, given what is already in the knowledge base — what it adds, confirms, contradicts, or duplicates.",
  "related_notes": ["paths relative to the knowledge base folder that informed the assessment"]
}
```

Type and category are open vocabularies seeded by the lists above (stored in config so the user can edit them); the agent should pick from the list and may propose a new value when nothing fits. The service validates the JSON (types, lengths, `related_notes` exist inside the knowledge base folder — drop any that don't). On invalid output it sends one follow-up turn asking for the corrected JSON only; a second failure fails the run, keeping the raw text for Copy.

## 7. Outputs

### 7.1 Summary file

Location and identity follow DESIGN.md §5.2 as already specified: `<summaries_dir>/<slug>.summary.md`, keyed by the same record key as the page note (so `/edit` and `/view` share it), host-written `brauser.kind: summary` ownership frontmatter, replaced whole on re-summarize, never adopting an unowned file. New content:

```markdown
---
brauser:
  kind: summary
  record_key: …
canonical_url: https://…
title: …
summarized_at: 2026-10-26T14:05:00-07:00
document_type: specification
category: technical specification
importance: high
entities:
  technologies: [ … ]
  products: [ … ]
  people: [ … ]
  companies: [ … ]
harness: codex
thread: codex://threads/…     # or claude_session: …
---
[Page](https://…) · [[Page note]]

**Type:** specification · **Category:** technical specification · **Importance:** high — <reason>

**Entities:** Technologies: [[…]] · Products: [[…]] · People: [[…]] · Companies: [[…]]

## Summary
<description>

## Assessment
<assessment>

Related: [[…]], [[…]]
```

Entity and related-note links use the notes profile's link style (DESIGN.md §6.3): `[[wiki links]]` for the Obsidian preset, plain text for the neutral profile. Agent-supplied text is escaped for YAML and neutralized per DESIGN.md §5.2 step 6.

### 7.2 Log entry

A new append-only `summary` event in the daily log (same owned-marker scheme as `activity`/`why`, with a canonical machine payload): time, canonical URL, title, record key, document type, category, importance, and a link to the summary file. One line per successful run; re-summarizing adds a new line and replaces the file. **A summarized page becomes an artifact (user decision, 2026-10-26).** When no adapter recognizes the page, the summary event creates or updates a `web.page` artifact (the index's existing `web.page` / `web:<host>/<url_id>` type in `host/src/index.rs`) keyed by the page's record key. It then behaves like any captured artifact: an artifact file in `artifacts_dir` (regenerated lazily, ownership-checked), a Timeline line per summary, a row in **Recent**, and worklog wording ("Summarized: <title> — specification, high importance"). The artifact file links to the summary file. On a page an adapter does recognize, the summary event attaches to that adapter's artifact instead. This is an explicit, per-click record, not automatic capture, so it does not conflict with the anti-goal in §2. This reverses DESIGN.md §5.2 step 5's "never creates a log entry".

### 7.3 Panel

- The page area on every HTTP(S) page shows the note editor and a **Summarize & log** button (also a `summarize` keyboard command and a "Summarize with Brauser" context-menu item).
- Clicking extracts readable content with a bundled, pinned Readability-style library via `chrome.scripting.executeScript` in the top frame, then sends `start_run`.
- A run card shows state, elapsed time, streamed text (rendered as text only, never HTML), Cancel, Open in Codex / Copy resume command, and on success the type/category/importance line with a link that opens the summary file. Failures show the reason and Copy; save failures add Retry save.
- Cards for in-flight and recent runs are restored from `list_runs` when the panel opens. A page with an existing summary shows its type/category/importance line and an "open summary" link.

## 8. Constraints

- Principle 9: nothing here may assume AMOS/ENDURANCE, Backstaff, DISCOVERY, or the author's paths. Defaults stay neutral; the knowledge base folder is a user choice.
- macOS is the supported platform for this work; existing Windows code paths are not broken, and Windows CI (`brwr-bzx`), if it lands first, must still pass — the service module compiles out (`cfg`) on non-macOS.
- Rust unsafe code only in a dedicated small crate (see §5.1). New crates must pass `cargo deny check --all-features --locked`; Cookie's sandbox cannot fetch crates, so any new dependency is a **[human]** step like `brwr-mff`.
- Keep the existing quality gate green: protocol generation check, `cargo build/test/fmt/clippy` (both clippy targets), `npm ci`, typecheck, lint, extension build and tests, `npm run test:adapter-diff`, and `npm run smoke:macos:headless`.

## 9. Left to the builder

- Protocol v6 schema details, including whether `sites` survives as derived data.
- Socket activation vs `launchctl kickstart`; idle timeout value; queue size limits.
- The exact Claude Code and Codex flags, verified against the installed versions (record them in TESTING.md).
- How the headless smoke run covers the service (a fake harness script that emits the JSON answer is expected; a real Chrome + launchd run is a **[human]** check).
- Splitting the work into reviewable PRs. Suggested order: (1) open access and removals, protocol v6; (2) the service skeleton, dev registration, socket proxy, run records with a fake harness; (3) Codex and Claude Code runners, extraction, the panel card; (4) summary file, `summary` log event, index/Recent/worklog integration, knowledge base folder setting.
- Later, not in scope: moving `index.db` writes into the service as the single writer (would mostly resolve `brwr-7vf`); a Windows service equivalent.

## 10. Documentation and backlog changes (part of the work)

- **DESIGN.md:** principle 2 (least privilege → "Brauser can see any page; it records automatically only through enabled adapters"), principle 8 (an on-demand per-user service now exists), §5.2 (rewritten to §6–§7 here), §5.3 (no first-note gesture), §7.1 (harness lockdown replaced), §7.3 (only `strip_params` remains), §8 (permission table), §12.2 (new step for this work replacing 8.2–8.5's harness parts; record keys and summary-file identity carry over), §14 (record these decisions with the date; close the auth question).
- **AGENTS.md:** "Current state" and the Security rules (remove the denylist, harness-token, cleared-env, reviewed-argv, and per-site grant rules; add the service, `agent.env`, and full-access agent rules).
- **README.md / TESTING.md:** install now includes the service; how to register the dev service; what headless cannot prove (launchd, real harness logins).
- **BOSUN_FEATURE_BACKLOG.md:** add items for the four PRs above; close `brwr-9yc` (decided) and mark `brwr-jfe` / `brwr-0cj` superseded by polling; rewrite `brwr-6wz`, `brwr-0ij`, `brwr-deo` to point here; note that `brwr-1hj` Capture Site's ungranted-tab flow and grant rollback no longer apply.
