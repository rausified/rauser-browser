# Brauser: The Rauser Browser Assistant — Design Specification

> Status: Draft v0.6 · M0 merged, M1 accepted for development on macOS, side-panel rework, M2 step 1 (harness setup), service-worker background delivery, and M1.5a adapter-only capture with settings review UI, first-tool starter-draft gallery, hourly artifact activity, batched durable delivery, rebuildable indexing, lazy artifact files, record keys, and artifact-facing views merged; open access with protocol v6 (standing HTTP(S) access, sites derived from adapters, no native confirmation, AI denylist, or setup token, and a knowledge base folder) merged (open access PR 1, #69); the on-demand per-user Brauser service skeleton (run records, queue, polling run messages, and `agent.env`, open access PR 2, #75); Summarize & log with full-access Codex app-server and Claude Code runners, the §6.4 prompt and a validated answer, pinned Readability extraction, and the polling run card merged (open access PR 3, #98); summary files, the `summary` log event, summarized-page artifacts, and summaries in Recent, the worklog, and the panel built on a branch (open access PR 4); Capture Site for starter and unknown sites (the host's structural drafter, per-rule checks with URL-prefilled shared-origin tenants, and a plain-language panel review, `brwr-t40`) merged; Capture Site's Improve with AI agent refinement through the service, held to the same host checks (`brwr-f45`), built; the side panel's task bar (free-text agent tasks through the service, run kind `task`, additive within protocol v8) built; live-tool certification is next, no public package yet · Short name "Brauser"; CLI and binary `brauser` · License: Apache-2.0 · Platforms: Google Chrome on macOS; Windows reinstated 2026-10-16, Windows CI green and no Windows acceptance recorded yet (§13, §14)
>
> Companion specification: [adapter-templates.md](adapter-templates.md), the adapter-template language artifact adapters are written in (§7.2), built in §12.2 step 6.2.
>
> History: [docs/history.md](docs/history.md) is the append-only record of dated decisions, as-built notes, probe and acceptance results, review reproductions, and superseded designs. This file is the contract: what Brauser does and must do now. Record a new decision or as-built note in docs/history.md under its section, and change this file only to state the resulting current rule.

## 1. Purpose

Brauser turns the browser into a place where context accumulates instead of disappearing. It runs as an always-on sidebar that:

- **Remembers the work artifacts** the user opens (tickets, docs, pull requests, designs) as one record per artifact, with how long they focused on it and how they got there, and shows them in one Recent list and an optional worklog across every tool (§5.1)
- **Summarizes** pages into a separate summary file per page, using the user's own local AI agent
- **Annotates** sites with persistent, per-page notes
- **Saves** pages for later
- **Surfaces** related pages from the user's history and notes
- Provides an **omnibar** for commands and free-text workflows, usually operating on the current page

Captured notes, the artifact log, artifact files, and read-later items are plain Markdown in a folder the user chooses. Operational config and a rebuildable search index stay in standard per-user OS locations. No cloud service, no account, no telemetry: local diagnostics files (§15) stay on the computer and are never sent anywhere.

## 2. Design Principles

1. **The user owns the data.** Markdown files are the source of truth. Everything else, such as the search index, is a rebuildable cache.
2. **Brauser can see any page; it records automatically only through enabled adapters.** The extension holds standing HTTP(S) access from install, so the panel can show, note, and (later) summarize any page on demand. Automatic capture is narrow: only URLs an enabled adapter recognizes, on the exact origins of those adapters, and the host re-checks every delivered event. The extension cannot touch the filesystem, and the host cannot do anything it isn't configured to do (decided 2026-10-26, §14).
3. **Page content is untrusted.** Any page the user visits may contain hostile content, including prompt injection aimed at the agent.
4. **Never clobber user edits.** Brauser only modifies what it owns. Brauser owns a few kinds of file whole: summary files (§5.2), artifact files and optional worklog files regenerated from the log (§5.1, §6.7), and page notes, which are edited only in the side panel (§5.3). Any edit made elsewhere to a summary, artifact, or worklog file is replaced when that file is next written. Brauser never adopts or replaces a file whose recorded kind and identity do not match.
5. **Format-neutral, convention-friendly.** Output works in any markdown tool. Obsidian is a first-class preset, not a dependency.
6. **Configuration over code.** Agent harnesses, artifact adapters, and omnibar commands are defined in config, not hardcoded.
7. **Assume nothing about the user's setup.** No default vault location, agent, or site list. Brauser ships with examples, and the user makes every choice explicitly at setup. Brauser writes only inside the home folder the user chose (plus its own state in the OS config and cache directories); inside the home it keeps a fixed layout of human-facing folders (§6.1) instead of asking the user to name each one (decided 2026-10-27).
8. **No always-running native service.** The host runs only while the side panel or settings page is open, or briefly while the service worker delivers buffered events and renews its policy lease (§3.2). If the user enables artifact logging, the browser's event-driven extension service worker may wake on navigation, focus changes, and its alarms while the browser is open to keep hourly activity totals on enabled origins. One on-demand per-user service exists for agent runs (2026-10-26, §12.2 step 10): `brauser service`, a per-user LaunchAgent that launchd starts only when the host asks for it (`launchctl kickstart`), that runs as the user and never as root, and that exits after 10 minutes with no queued or running run. A run it started may finish after the browser closes; otherwise nothing runs when the browser is closed.
9. **Brauser is a standalone product.** It is built, tested, documented, and configured as something anyone can clone, build, and run with no dependency on its author's personal tooling, vault, or any other system of his, such as AMOS/ENDURANCE, Backstaff, or Astrolabe. No file, build step, test, or default in this repository may assume one of those systems exists. Interop runs in one direction only: Brauser emits plain, documented data (the log, artifact files, its own ID scheme) that another system is free to read and map to its own concepts; Brauser itself never adopts another system's scheme, vocabulary, or presence check. This is also why principle 6 (configuration over code) and principle 7 (assume nothing about the user's setup) exist, and it governs every integration decision, including Brauser's own ID scheme (§14).

## 3. Architecture

```
┌──────────────────────────────┐        native messaging        ┌──────────────────────────────┐
│  Browser extension (TS, MV3) │  ◄──── JSON, versioned ────►   │  Native host (Rust)          │
│                              │                                │                              │
│  • Sidebar UI + omnibar      │                                │  • Policy enforcement        │
│  • Navigation observation    │                                │  • Vault read/write          │
│  • Content extraction        │                                │  • Search index (SQLite FTS5)│
│  • No secrets, no FS access  │                                │  • Agent invocation          │
└──────────────────────────────┘                                │  • Config owner              │
                                                                └──────────┬───────────────────┘
                                                                           │ argv, no shell
                                                             ┌─────────────┴─────────────┐
                                                             │  Local agent harness       │
                                                             │  (Claude Code, Codex, CLI) │
                                                             └───────────────────────────┘
```

### 3.1 Browser extension

- TypeScript in strict mode, Manifest V3, bundled with a pinned build toolchain.
- Targets Google Chrome Stable on macOS for the first release. Other browsers and operating systems are out of scope (§13).
- Sidebar via the Chromium `sidePanel` API for the current page: its note, summary, related pages, and the omnibar, with capture status in a collapsible strip (§5.7). The side panel's header has a settings (gear) button, and the panel shows a warning with a link to settings until a home folder is chosen (protocol v6; before it, the warning also waited for an enabled site and capture). If the user withdraws Brauser's site access in Chrome, the panel shows one "Brauser needs site access — re-enable in Chrome" banner on HTTP(S) tabs, with a button that opens Brauser's Chrome extension settings.
- A dedicated settings page (`options_ui`, opened in a tab) holds configuration: home folder, enabled artifact adapters and their origins, and pause. A full tab hosts the native folder picker better than the narrow panel. The panel keeps a one-click pause as an emergency stop.
- Holds UI state only. It stores no secrets and makes no network requests to external services.
- No remote code: no `eval`, no dynamically loaded scripts, and a strict extension CSP. From M2 the manifest sets `content_security_policy.extension_pages` to `default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; font-src 'none'; media-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'`. Native messaging does not go through CSP.

### 3.2 Native host

- One Rust host binary per supported OS and CPU architecture, with `#![forbid(unsafe_code)]` in all first-party crates. (history: [docs/history.md §3.2](docs/history.md#32-native-host--history))
- The only component with filesystem and process privileges.
- Treats every inbound message as untrusted, because a compromised page or extension context could influence it.
- Registered through a native messaging manifest whose `allowed_origins` lists only the official extension IDs.

**Lifecycle.** The host runs only while an extension context holds a native messaging connection to it. It is never a daemon, service, or login item. The side panel and settings page each open their own connection when they load, and the service worker opens one for each background sync (below); the browser spawns a host process per connection, and that host exits when the connection closes (stdin EOF). Work the host would otherwise do in the background, such as catching up on external edits to notes, happens on connect instead (see §5.5).

Capture does not depend on the panel. The extension's service worker observes navigation on enabled origins and buffers qualifying events in `chrome.storage.local` (with artifact logging, hourly `activity` events; see below). Each record has a stable event ID, original HTTP(S) URL, optional title, and timestamp. The buffer is capped by count and bytes. Before writing any record, the worker awaits `setAccessLevel({accessLevel: "TRUSTED_CONTEXTS"})` and loads its policy lease, a revisioned, host-sent policy with a bounded expiry; an absent or expired lease fails closed. The host independently re-checks every delivered event against its own adapters on the original URL, before any lossy normalization. Events leave the buffer only after a terminal host acknowledgement (`persisted`, `suppressed`, or permanent-policy `rejected`); transient errors retain the event for bounded retry and appear in the panel alongside rejections and queue overflow. An invalid or newer host config suspends the extension policy without deleting buffered visits; the panel offers an explicitly confirmed discard action during repair. Disabling logging purges affected records. The side panel and settings page serialize config changes across every open window through one shared lock. The worker accepts messages only from those two extension pages. Each page watches the worker's policy lease and re-reads the host when another page publishes a different revision. **Background sync.** The service worker, not the panel, delivers the buffer and keeps its policy lease current. About 30 seconds after a capture (a one-shot `alarms` alarm, which also covers the title grace period), every 5 minutes (a periodic alarm, for retries), and at browser startup, it starts a host over native messaging, delivers every ready event, and closes the connection. When the lease is within an hour of expiry it first reads the host config under the shared config lock and republishes it, exactly as a page's reload does; a config issue suspends the lease without deleting events. A navigation that finds the lease already expired, for example the first after Chrome was closed for a day, renews it before capturing. A sync with nothing to deliver or renew starts no host. The panel's **Send pending visits** asks the worker to sync now, and a background failure to reach the host is shown in the capture strip. A background sync sends only `get_config` and `record_batch`; config changes stay with the panel and settings page, and the host re-checks every delivered event against its own config either way. Agent runs happen in the Brauser service (principle 8) and continue after the panel closes; the panel restores their cards when it reopens (§5.2). (history: [docs/history.md §3.2](docs/history.md#32-native-host--history))

**Open access.** Brauser holds `http://*/*` and `https://*/*` from install, so the worker no longer checks a per-origin Chrome grant, reconciles revoked grants, or narrows sites when access changes, and there is no consent or native dialog in the policy path. The lease is still revisioned, host-sent, bounded, and fail-closed; it carries the enabled adapter document, and the worker captures only on that document's origins. Origins an earlier build had removed or seen revoked are migrated into one pending-removal list whose adapters stay out of the lease until the host no longer holds them. The host still re-checks every delivered event against its adapters before vault I/O. (history: [docs/history.md §3.2](docs/history.md#32-native-host--history))

With artifact logging (§5.1), recognition has to happen while the page is live, because the host is usually not running when a visit happens. The host-confirmed policy lease the worker already caches therefore also carries each enabled adapter's confirmed adapter templates (match rules, `id` and title templates, mode map, and scope; §7.2) and, from M1.5b, its Tier 1 selectors, under the same revision and expiry. The worker prefilters with them: a visit no adapter matches is dropped before it reaches the buffer, and a matching visit updates the pending hourly total for its provisional `artifact_id`, with any Tier 1 values read from the live page. The prefilter only narrows capture. At delivery the host re-runs its own recognition on the original URL and title and discards any field the confirmed adapter does not declare, so a compromised worker can drop visits but cannot widen what is recorded. An absent or expired lease fails closed, as in M1. The worker also accumulates attention into one pending total per artifact per local hour in trusted-only `storage.local`. The total stays pending until its hour ends; the worker then freezes it into one `activity` event in the buffer (§5.1).

### 3.3 Why this split

The extension runs in a hostile environment (arbitrary web pages), and the host holds real privileges (files, processes). Putting all policy in the host means a bug in the extension cannot escalate into arbitrary file writes or command execution. Rust gives memory safety on the side of the boundary where it matters most.

## 4. Security Model

### 4.1 Threats in scope

| Threat | Mitigation |
|---|---|
| Malicious page influences extension messages | Host validates and authorizes every request; extension is not trusted |
| Path traversal / symlink escape out of the vault | Host builds all paths from its own slugs; canonical-root confinement; capability-based FS access (e.g. `cap-std`) |
| Command injection via agent invocation | Harnesses run via a fixed argv with no shell; page text never goes on a command line: the service saves it as the run's `page.md` and the prompt names that file by path (Claude Code gets the prompt on stdin, Codex over its app-server pipe) (§7.1) |
| Prompt injection from page content | The prompt marks the page untrusted and tells the agent to ignore instructions in it. Since protocol v6 (2026-10-26, §14) the agent runs with full access on the user's login (Codex `danger-full-access`, Claude Code `--dangerously-skip-permissions`), so this is accepted, not prevented: only a summary the user asks for on one page runs an agent, and settings warns that a page could carry instructions aimed at the agent. The answer is validated before use, streamed text and answers are shown as text only, and the summary file is stored with remote embeds neutralized (§5.2, §7.1) |
| Sensitive pages sent to an agent | Removed with protocol v6 (2026-10-26, §14): there is no agent denylist. An agent runs only when the user asks for it on one page, and settings warns that a page could carry instructions aimed at the agent |
| Unwanted browsing surveillance | Automatic logging is opt-in per adapter and origin. Only artifact activity reaches the delivery buffer: the worker drops other pages on an enabled origin before buffering, using the confirmed rules in its policy lease, and the host re-checks every delivered event (§3.2, §5.1). The one exception is the Tier 2 hint's sample in the worker: at most 20 URL shapes per enabled origin, with IDs and title-like text replaced by placeholders and no titles, kept for 7 days and never written to the home folder. It records what kinds of page the origin has, not which pages were visited (§5.1). Create adapter's link shapes are read only on the user's click, shaped inside the page, and kept only for that draft (§5.1). A compromised worker could still buffer or deliver anything it sees (from protocol v6 it can see any HTTP(S) page), but the host records only what an enabled adapter matches, on that adapter's origin. Incognito is never logged. On pages outside enabled origins, Brauser keeps only what the user explicitly creates: a page note, a `/log` entry, or a summary |
| Hostile adapter templates or captured fields | Adapters use adapter templates, not regular expressions: matching is linear in its capped input in both engines by construction, with no backtracking, and both engines are checked against shared vectors, differential tests, and fuzzing (§7.2). No capture type admits `/`, `:`, `%`, whitespace, or control characters, so captures cannot form paths or ambiguous IDs (the host builds slugs). Titles and DOM fields are untrusted text, escaped and embed-neutralized before writing (§7.2) |
| Invisible formatting spoofs generated history | A shared host presentation sanitizer replaces Unicode format characters (category Cf), control characters, and Unicode line/paragraph separators with spaces in daily row titles/context, generated artifact titles/context, and worklogs. Markdown prose also escapes embed syntax and Obsidian comment, highlight, and tag delimiters. YAML titles are sanitized before JSON-compatible quoting. Canonical event payloads and identities stay unchanged; rebuilding artifacts never rewrites source logs |
| Personal activity recorded on shared origins | On origins that host many tenants (GitHub, Google Docs, Notion, Figma, Linear), an adapter must restrict captures to listed tenants with `scope`, or carry an explicit `unscoped` acknowledgment, which settings explains records every account in the Chrome profile, recommending a separate work profile (§7.2; protocol v6 removed the native confirmation) |
| A page choosing which note or summary it gets | The host derives every record key from the URL through confirmed Tier 0 rules and aliases only; the extension never sends a key, and Tier 1 fields and page content never affect one (§6.2) |
| Page-controlled text steering a drafted adapter | The sample and Create adapter's link shapes hold only URL shapes, never titles, and a shape keeps only short lowercase route words, so a page has almost no text of its own to plant. A page can still plant links to steer the structural drafter, so link shapes get the same treatment as the sample (§5.1). Drafts from either drafter are still untrusted, limited to Tier 0, and written in adapter templates, which have no free-form patterns; the host drops draft rules that break the draft structure rules, overlap an existing adapter, or match the origin root, search pages, or probe URLs, and shows a match preview of every sampled, visited, and link shape and probe URL before the native confirmation (§5.1). Against planted links: a literal seen in only one link shape becomes `{slug}` inside the page, a rule backed only by link shapes starts unchecked, and visited shapes rank first |
| Confidential titles leaving the machine through a synced home folder | The user chooses the folder; protocol v6 removed the sync-location warning (§5.1, §14) |
| Other local extensions talking to the host | `allowed_origins` restricted to official extension IDs |
| Supply chain compromise | Pinned dependencies, `cargo-audit`/`cargo-deny`, npm lockfile audit, signed releases with provenance |

### 4.2 Threats out of scope

- A compromised local user account or OS
- A compromised agent harness binary
- Malicious config files written by the user

### 4.3 Host message handling

- Messages are length-prefixed JSON. M0 caps extension-to-host bodies at 4 MiB and host-to-extension bodies at 900 KiB, below Chrome's 1 MiB outbound limit. A response that would exceed the outbound cap is replaced by a correlated `message_too_large` error; the host keeps serving. Page-note bodies are capped at 98304 characters (counted as UTF-16 units, as the panel's textarea counts them) and 384 KiB, and a save that could not be returned whole by a later load is refused, so a saved note always loads; a note grown past either limit outside the panel loads as `message_too_large`, never into a textarea that could not accept further typing. A new note's title is clamped to 2048 bytes, not refused.
- Deserialized into typed structs with `deny_unknown_fields`. Unknown message types are rejected.
- Every message carries a protocol version and request ID. Version mismatch returns a structured error.
- Single and batched activity delivery share `check_activity_policy` under the
  current configuration lock, before opening the home folder. Both enforce
  collection bounds, recognition and scope, artifact identity, mode evidence,
  trail sources, and title references in the same order. Unavailable policy
  remains retryable; intentionally paused capture remains suppressed.
- Parser and path handling are fuzzed continuously (see §10).

### 4.4 Filesystem rules

- The host stores the selected folder's filesystem identity after the native picker. Each privileged operation opens a root-scoped directory handle and checks that identity; a moved or replaced folder requires reselection, including repair at the same path. The identity is the volume device and file ID, and a remounted external or network volume can get a new device number, so it may need reselection too. A missing or mismatched folder blocks capture and notes but keeps every other setting, including the adapters, for the repair; a folder that comes back with its old identity needs no repair.
- Filenames are generated by the host from normalized slugs. The extension never supplies a raw path.
- M0 creates new page files only: write a temp file in the same directory, fsync it, then publish with an atomic no-clobber hard link. It never replaces an existing file.
- Page notes are edited only in the side panel (§5.3), so the host replaces a note whole: write a temp file in the same directory, fsync it, then atomically rename it over the note. Each save names the note version it was based on; if the file has changed since then (for example, from another Brauser window), the host refuses the save and returns the current note. Before replacing, the host checks the file's recorded record key (§6.2) and Brauser ownership, as for summaries (§5.2).
- Summary files (§5.2) are also replaced whole, without a version check. The service's `SummaryFinisher` writes them through the shared ownership-checked `replace_owned_file` (host/src/vault.rs), and a failed write surfaces as the run's `save_failed` with Retry save, not a `retryable` outcome; §5.2 step 7. Brauser owns the whole file, so the `/summarize` action is the confirmation for replacing it. The host writes and fsyncs a temp file in the summaries directory, then publishes with a no-clobber hard link if the name is absent, or renames over the existing name only if it is a regular file (not a symlink) whose leading frontmatter has `brauser.kind: summary` and the requested record key (§6.2). Anything else is a `conflict` and is left untouched. The window between the check and the rename is accepted for summary files only: an external save in that window loses only an edit to Brauser's summary text, never note text. Logs and read-later files stay create-only. (history: [docs/history.md §4.4](docs/history.md#44-filesystem-rules--history))
- Artifact files (§5.1, §6.7) are regenerated from the log and replaced whole with the same checked rename as summary files, matched on `brauser.kind: artifact` and the requested `artifact_id`. Anything else at the path is a `conflict` and is left untouched. Regeneration is lazy (§6.7), and a file whose new bytes equal its current bytes is not rewritten, so sync clients and version control see a change only when the content changed.
- The one-time move of an older folder layout (§6.1) renames whole folders inside the home with a no-clobber rename, under the locks every vault writer holds. It never replaces or merges into an existing folder; a conflict leaves everything as it was and asks the user to move the folder in the way.
- Destructive operations, such as deleting a note or read-later file, require explicit user confirmation in the sidebar. Saving a note's text is not destructive.

### 4.5 Agent invocation rules

- Agents run in the per-user Brauser service (§3.2, §12.2 step 10), never in the Chrome-spawned host. The harness is one of two built-in runners, Codex or Claude Code, chosen at setup by its id; no config entry names a binary path or arguments for a run.
- A run has full access by design (§14, 2026-10-26): it works in the knowledge base folder with the service's environment plus `agent.env`, on the user's normal login, under the per-run timeout, and cancel or timeout kills its process group.
- Page text never goes on a command line. The service saves the extracted page as the run's `page.md`, and the prompt names that file by absolute path and marks its content as untrusted data that may contain instructions to ignore (§7.1).
- The agent never writes a Brauser record. The service validates the agent's final JSON answer (§7.1) and, from PR 4, writes the summary file and log event itself.
- **Task runs (the task bar, §5.7; 2026-10-28).** A task is a third run kind, `task`, started only by `start_task` from a user gesture in the side panel; the host bounds the instruction (1 to 4096 characters, not blank, no control character but newline and tab; the service checks it again), resolves the record key and forwards a `start_run` with `kind: "task"`. The rules:
  - **Trust split.** The user's typed instruction is the only directive. The prompt fences it between `----- BEGIN USER REQUEST -----` and `----- END USER REQUEST -----` lines. The page (title, URL, content, selection) goes only to `page.md`, which the prompt names by path and marks untrusted; the page title is not in the prompt, and the instruction is never in `page.md` or on any command line. The agent is told not to send the page or any local file anywhere unless the request asks for that.
  - **Page steps.** The page never authorises anything and never widens the request. The agent carries out steps written on the page only when the user's request explicitly asks for them, and then only the steps it covers; any other instruction on the page is not followed and is mentioned in the report. A `BEGIN USER REQUEST`/`END USER REQUEST` line inside `page.md` is page text, and so is the "Highlighted by the user" section, whose heading the page itself can also write. The prompt calls that section text selected on the page, not text the user selected, because a page can make its own selection.
  - **Working directory and protected locations.** A task runs in the knowledge base folder, with a summary's harness, environment, process-group kill and the knowledge-base identity recheck when it leaves the queue, and a time limit of at least 30 minutes (the larger of `[summarizer] timeout_secs` and 1800 seconds, at most 3600; §7.1). With no separate knowledge base that is the home folder itself. The prompt lists Brauser's own locations as off limits, by absolute path: the home folder's `notes/`, `logs/`, `artifacts/`, `summaries/` and `later/`; the configuration folder, which is also the service's support folder holding `runs/` (reading the run's own `page.md` is allowed); and the cache folder. The prompt also forbids reading anything in that support folder other than the run's own `page.md`, and names `agent.env` by absolute path as never to be read, because it can hold secrets. This is prompt-level protection, not enforcement: a full-access agent can still read and write there. If those folders cannot be found, the service refuses the start with `internal` and queues nothing (fail closed). When a queued task leaves the queue, the service recomputes those paths under the config lock; if they differ from the ones recorded for its prompt (the home folder changed while it waited), the task fails before its harness starts with "Brauser's home folder changed while the task waited. Run it again."
  - **Output.** The agent's final message is a plain-text report. Codex gets no `outputSchema`. The service replaces control characters but newline and tab, and invisible format characters (category Cf: bidi controls, zero-width characters, the byte-order mark), with spaces, trims, and keeps up to 16 KiB on a character boundary, with a note when it shortens it; an empty reply gets one repair turn that says not to redo any work and asks for no JSON, and a second empty reply fails as `invalid_output`. A task never fails on content, because the agent may already have acted.
  - **Nothing written.** A task writes no summary file, daily-log event, page note or config (`TaskFinisher`), and a task view never offers Retry save. A report that cannot be kept fails the run with "The agent finished, but its report could not be kept. Changes it made stay."
  - **Queue.** A task never joins another run, even for the same page. At most one task runs at a time, inside the two-run limit, so summaries and site drafts always keep a slot; at most four tasks are queued or running, and a fifth is refused with `conflict`. A task is never resumed after a service restart: one still queued then fails as `service_restarted` with "The Brauser service restarted before this task ran. Run it again if you still want it.", so an old full-access instruction never runs unattended (queued summaries and site drafts still resume).
  - **Where the instruction is kept.** In the prompt (stdin for Claude Code, JSON-RPC for Codex); in the run's `status.json` (`task.instruction`), under the 30-day/200-run retention; in the Codex thread's name, "Brauser: Task: " plus its first 80 characters; in `stream.txt` wherever the harness echoes it; and in the harness's own session store (Codex threads, Claude Code sessions), which Brauser does not prune. It never reaches a diagnostics log (§15).
- Earlier agent-run safeguards and their removal: (history: [docs/history.md §4.5](docs/history.md#45-agent-invocation-rules--history))
- **The service.** The service listens on a 0600 Unix socket in Brauser's 0700 support directory (the config directory) with the native-messaging framing, and exits after 10 idle minutes. The short-lived host forwards `start_run`, `get_run`, `list_runs`, `cancel_run`, and `retry_save`, resolving the record key itself and checking `requested_at`; it starts a missing service with `launchctl kickstart`, compares the service's `service_hello` build identity with its own, restarts a stale build once, and otherwise answers `service_unavailable`. Each run has a record directory under `runs/` (`status.json`, `page.md`, `stream.txt`, `result.json`, `run.log`); at most two run at once, the rest wait FIFO, and a page has at most one queued or running summary or site-draft run (a task never joins another run, and at most one task runs at a time; see Task runs above). Runs time out after `[summarizer] timeout_secs` (10 minutes by default), a task after at least 30 minutes (§7.1), cancel kills the run's process group, a service restart marks orphaned runs `failed`, and records are kept for 30 days or the newest 200. An optional 0600 `agent.env` is overlaid on the service environment at each run. The service builds on every Unix and is compiled out on Windows; only the launchd glue (`launchctl kickstart`, the LaunchAgent) is macOS-only. Linux is not a product platform, but development and CI workers build and test the service there: its support directory is the config directory (`$XDG_CONFIG_HOME/brauser`, normally `~/.config/brauser`), and the host never starts it, answering `service_unavailable` unless a `brauser service` is already running (the `scripted-dialogs` spawn seam works the same as on macOS). (Moved here from §12.2 step 10.2.)

## 5. Features

### 5.1 Artifact logging

Brauser remembers the tickets and docs the user works on. It does not keep a list of visited URLs. The unit of history is the **artifact**, such as a Jira issue, a Confluence page, a Google Doc, or a GitHub pull request, identified by a canonical ID and not by a URL. Six URLs for the same Jira issue (the issue page, a board modal, a link with tracking parameters) resolve to one record. Pages that are not artifacts, such as search results, dashboards, and board views, are never logged.

Artifact logging replaces M1's per-site automatic logging; the two do not run side by side. M1's capture machinery stays: the revisioned, host-sent policy lease (protocol v6), the buffer, event intents, and the append-only daily log. The exact-origin Chrome grant and the host's native confirmation that M1 also used are gone since open access (2026-10-26, §3.2 and §14). What changes is what qualifies for the log. A visit is recorded only when an enabled adapter (§7.2) recognizes its URL.

**Recognition.** An adapter maps a URL on an enabled origin to an artifact type and canonical ID, for example "this URL is Jira issue `PROJ-123` in `acme.atlassian.net`." Adapters are config (§7.2) and are recognized in three tiers:

- **Tier 0: URL and tab title.** This tier needs no permissions beyond Brauser's install-time HTTP(S) access, and it provides most of the value. Work tools put their identity in the URL and most of their metadata in the tab title, which the worker already reads from tab metadata. Jira uses `/browse/PROJ-123`, or `?selectedIssue=PROJ-123` when the issue opens as a board modal. Confluence uses `/wiki/spaces/<space>/pages/<id>`. Google Docs, Sheets, and Slides use `/<kind>/d/<id>/`, with a title such as "Name - Google Docs". A GitHub pull request title carries the title, author, number, and repository. Notion, Figma, and Linear use stable path IDs. SharePoint often puts identity in a `sourcedoc` GUID in the query string. Every adapter must work at Tier 0.
- **Tier 1: declarative DOM fields (M1.5b).** Optional per adapter, for data the title does not carry, such as status, assignee, labels, or parent epic. The adapter lists named CSS selectors. On an enabled origin, the extension runs one fixed, bundled extraction function through `scripting` (§8), with the selectors passed as arguments. It returns only those named fields, reads only each element's rendered `innerText`, and never reads form values or attributes. Values are capped plain text and are untrusted like any page content. The worker runs Tier 1 while the page is live, whether or not the panel is open, and only on a URL its lease-based prefilter has recognized (§3.2). It reads the fields once the tab's title settles after the commit or, for an SPA route, after `onHistoryStateUpdated` (the title update can arrive just before `onHistoryStateUpdated`; M1 mechanics, below), and retries once a few seconds later if a field is empty, because work tools often render late. The latest values go into the hour's activity total. At delivery the host keeps only the fields the confirmed adapter declares, capped again. A field the worker could not read stays empty; the host never fetches it later.
- **Tier 2: drafted adapters.** Users can make their own adapters without writing templates. A draft starts in one of two ways: the **Create adapter** button in the side panel, available on any HTTP(S) page (on an ungranted page it starts from the context menu or the toolbar action), or a hint when the user keeps visiting unrecognized pages on an enabled origin, for example "Looks like you have recurring pages on `wiki.internal`." Either way, Brauser reviews the origin's structure as **URL shapes** and drafts an adapter from them. A drafter never receives page content, titles, or real URLs.

  A shape is a URL path and query with every identifier and every piece of title-like text replaced by a typed placeholder, for example `/wiki/spaces/{slug}/pages/{num}/{slug}` or `/jira/software/projects/{key}/boards/{num}?selectedIssue={key}`. The worker builds the shape from the original URL before storing anything:
  - A path segment stays literal only if it is at most 20 ASCII lowercase letters with at most one `-` or `_`, such as `wiki`, `browse`, `pull`, or `merge_requests`. These are the route words an adapter matches on.
  - Every other segment becomes a placeholder chosen by what it looks like. The class, not the value, is what a draft needs to choose a capture type (adapter-templates.md §7):
    - `{num}`: digits only. Suggests `int`.
    - `{key}`: an issue-style key (`[A-Z][A-Z0-9]+-\d+`). Suggests `jira_key`.
    - `{uuid}`: a UUID. Suggests `uuid`.
    - `{hex32}`, `{hex40}`, `{hex64}`: exactly 32, 40, or 64 hex digits. Suggests `hex(32)`, `hex(40)`, or `hex(64)`.
    - `{tailhex32}`: text, a `-`, and then exactly 32 hex digits, as in Notion's `Release-Plan-0123…`. Suggests `hex(32)@tail`.
    - `{id}`: anything else containing a digit that uses only `token`'s characters (letters, digits, `_`, and `-`), such as base64-style document IDs. Suggests `token`.
    - `{slug}`: the rest, such as title-derived text like `Release-blocker-SBOM`, a percent-encoded page name, or a value with other characters.

    Placeholder names are fixed, so the only digits a shape can contain are inside those names.
  - Query parameter names follow the same literal rule (a name that fails it becomes `{param}`), and every value becomes a placeholder of the same classes. The fragment is dropped.
  - The tab title is never stored and never reaches a drafter. Drafts therefore carry no title template, and the whole tab title is used (§7.2) until the user adds one by hand.

  Shapes are enough to draft an adapter, because an adapter matches on route words and ID positions, and they keep no record of what the user actually read. They also remove the main prompt-injection path: before, a planted page title could steer the agent toward a draft that would log every page on the origin. A shape leaves a page only short lowercase route words to plant, but those are still untrusted. Drafts are written in adapter templates (§7.2), which can be reviewed by reading and whose structure the host can check mechanically, so the host still treats a draft as untrusted and checks it before anything is shown:
  - The shapes go to the agent inside the same delimited untrusted-data block as page content (§4.5), and the agent denylist (§7.3) applies to the origin. The instruction to the agent includes the adapter-template grammar, and the host parses the draft with the same parser as any config; a draft that does not parse is discarded without being shown.
  - A drafter's output is limited to Tier 0 `match`, `id`, `modes`, and `refs`. Drafter output that uses `title`, `scope`, `unscoped`, `alias`, `fields`, or `path_case` is rejected; Tier 1 fields and `meta` aliases read the DOM, and scope decides whose activity is recorded. The user can add them by hand afterward, under the normal confirmation. On a shared origin, the host supplies scope: it turns the shared-origin list's tenant position into a capture, copies the scope when the origin's existing scoped adapters all agree on it, and otherwise leaves scope pending until the user chooses a scope or `unscoped` in the review. That choice, like a title entry the user adds in the review, is part of drafting, not a user edit, and a draft with pending scope cannot be enabled (adapter-templates.md §10.4).
  - A draft's origin is the one the user asked about, set by the extension from the tab. A drafter cannot choose or add an origin. The extension is untrusted, so this is acceptable only because the native confirmation names the origin.
  - Structure rules replace the old regex inspection (adapter-templates.md §10.4): every rule has at least one literal segment before its first identifier capture (a `name` capture, such as the host's tenant capture or a namespace like a GitHub repository, is not an identifier), `{*}` may not directly follow the first segment, and the ID uses at least one identifier capture. The host also drops any draft rule that overlaps an existing adapter on its origin, or that matches the origin root or any of a fixed set of probe URLs the host builds for that origin. A draft with no rule left is rejected. The probe URLs are:
    - search and filter shapes: `/search?q=x`, `/?q=x`, and paths or query keys such as `search`, `query`, `q`, `jql`, and `filter`;
    - pages that are not artifacts: `/login`, `/home`, and `/dashboard`;
    - auth and capability routes, whose URLs can carry secrets: `invite`, `reset`, `password`, `verify`, `confirm`, `share`, `magic`, `token`, `oauth`, `auth`, `signin`, `signup`, `logout`, and `unsubscribe`.

    The probe list is host-owned config.
  - The host also drops a draft rule that takes an ID from a query parameter on its search-and-filter list (`q`, `query`, `search`, `jql`, `filter`) or its pagination and view-state list (`page`, `offset`, `start`, `cursor`, `limit`, `sort`, `order`, `tab`, and `view`), and others in the probe config. This catches drafts that turn search or paging parameters into IDs.

  Before the native confirmation, the host shows a **match preview**. For every sampled shape, including the ones that are clearly not artifacts, every visited and link shape in a Create adapter draft, and every probe URL, it shows whether the draft matches and the ID it would produce. The host tests a shape by filling each placeholder with a fixed example value of its class (`{num}` as `123456`, `{key}` as `ABC-123`, `{slug}` as `example-page`, and so on). It also shows how many of the origin's already-recognized artifacts the draft would re-map. Enabling the draft then takes the native confirmation that any widening of capture needs (§7), and that confirmation repeats the count of sampled shapes the draft matches. Unrecognized visits never reach the host, so the hint's sample lives in the worker. For each enabled origin it keeps at most 20 distinct shapes, each with a visit count and the date last seen, in trusted-only `storage.local`, separate from the delivery buffer. Shapes expire 7 days after they were last seen and are purged when the origin is disabled. They go to the host only when the user asks for a draft, and they never enter the home folder. The sample is on by default, because it records only what kinds of page an origin has; the settings page shows it and can clear it.

  > **Superseded in part (2026-10-26).** Open access (protocol v6; [the open-access spec](docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md), backlog `brwr-1hj`) removes several flows this subsection and the Tier 2 text above still describe. There are no ungranted pages, so the `activeTab`, context-menu, and toolbar starts are gone; there are no grant purposes, `create-adapter` grants, or grant rollback; there is no final native confirmation naming the origin or repeating match counts, because config saves with the revision check only; and there is no agent denylist. Where the text below relies on these, follow the spec; the shaper, drafter, structure rules, probe URLs, match preview, and review still apply.

  **Create adapter.** The panel's **Create adapter** button (M1.5b) turns the site the user is on into a draft. It works as follows:
  - **Start.** The panel's **Capture Site** button starts the flow on any unconfigured HTTP(S) tab; open access has no grant purposes, `create-adapter` grants, or grant rollback. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
  - **Reviewing the site.** Brauser reviews the site only from what the browser already shows. It never fetches or crawls pages, because loading pages would act with the user's session on a site Brauser has no reason to touch. The draft's input is two sets of shapes, built with the same shaper as the sample:
    - **Visited:** the shape of the current tab's URL, and the origin's sample when the origin is already enabled. These record pages the user actually opened.
    - **Link shapes:** shapes of links on the current page. They show what else the site has.
      - One fixed, bundled function reads them. The panel runs it through `scripting` in the isolated world and in the tab's top frame (`frameIds: [0]`), only when the button is clicked. The panel has no document ID without `webNavigation`, so the panel passes the URL it saw, and the function returns nothing unless `location.href` still equals it.
      - It reads the `href` of up to 500 same-origin `<a>` elements and shapes them inside the page.
      - A literal that appears in only one link shape is replaced by `{slug}`, because route words recur and people's and customers' names usually do not.
      - It returns at most 50 distinct shapes with counts, so raw link URLs never leave the page.
      - Link shapes are held only for this draft. They are never added to the sample, and they never enter the delivery buffer or the home folder.
      - Besides the fixed Tier 1 extractor and the declared `meta` alias (§5.1 Aliases), this is the only page-reading code before M2's content extractor, and it reads only `href`.
  - **Hand-over.** The panel drafts through its own host connection; only the advanced editor gets a handoff (Capture Site for unknown sites, Handoff, below). (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
  - **The structural drafter.** The host drafts deterministically, with no agent, so Create adapter works without AI and before M2. It proposes **one adapter per kind of page**:
    1. **Filter.** It drops:
       - the root, the probe URLs, and search, filter, and paging shapes;
       - `{param}` query names;
       - every shape an existing adapter on the origin already recognizes.

       It also turns positions the shared-origin list marks as an account index, such as Google's `/u/<n>/`, into `{_}`, so they never enter an ID.
    2. **Group.** It groups the remaining shapes into kinds, keyed by the route words and placeholder classes up to the last identifier.
       - `{hex32}` and `{tailhex32}` in the same position are one class, drafted as `hex(32)@tail`, which matches both.
       - Shapes that differ only after their last identifier are one kind, drafted as one rule per distinct ending with the same ID. Examples are a pull request and its `/files` tab.
       - A trailing run of `{slug}` segments folds into `{*}` only when at least two segments come before it (adapter-templates.md §10.4). Otherwise each becomes `{_}`.
       - A route word that differs in one position within a kind becomes an alternation, but only when a literal comes before that position. Otherwise the kind splits into one rule per word.
       - Link-only shapes never widen a visited rule. An alternative, a `{*}` fold, or an ending that only link shapes support goes into a separate rule of the same kind.
    3. **Keep.** A kind becomes a candidate when an anchor comes before its first identifier placeholder in the path. When the identifier is only in the query, any anchor in the path is enough. An anchor is a route word or, on a shared origin with a tenant position, the host-supplied tenant capture (below): the review scopes it, and a scope-restricted capture anchors a template as a literal does (adapter-templates.md §4.1, §6). The tenant never counts as the first identifier and never names a kind. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
    4. **Map.**
       - Placeholders map to capture types as in the sample: `{num}` to `int`, `{key}` to `jira_key`, `{uuid}` to `uuid`, `{hexN}` to `hex(N)`, `{tailhex32}` to `hex(32)@tail`, and `{id}` to `token(1,128)`.
       - A `{slug}` after the tenant and before the first identifier names a namespace, such as a GitHub repository. It becomes a `name` capture that the ID includes. Any other `{slug}` that is not trailing becomes `{_}`.
       - An identifier in a query value becomes a query condition when the parameter is not on the search, filter, or paging lists.
       - When a kind has a fixed-shape identifier (`uuid` or `hexN`) in the path or the query, the ID uses the fixed-shape identifiers and the kind's other `{num}` positions become `{_}`. An example is SharePoint's `/_layouts/15/Doc.aspx?sourcedoc={uuid}`.
       - An alternation that names a mode, such as `edit` or `view`, is also mapped to `modes`.
       - Other route words that differ in a kind's one varying position name namespaces of its identifiers, so equal numbers under them are distinct pages: `/foo/discussions/7` and `/bar/discussions/7` never share an ID. Words seen with the same visited rules, and the words of one alternation, become one captured alternation, `{space:(bar|foo)}`, that the ID includes, when another route word anchors the rules (a captured alternation never anchors a draft, adapter-templates.md §10.4). Otherwise, or when the rules differ by word, the kind splits into one adapter per word or word set. A word only links support is always an adapter of its own, so a planted link never changes the visited pages' IDs. Mode words are never namespaces. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
    5. **Name.** Each kind's type is `<site>.<word>`, and its ID template is `<site>.<word>:{host}` followed by the tenant, namespace, and identifier captures in path order.
       - `<site>` is the host's first label (after a leading `www`, so `www.notion.so` drafts `notion.*`), and `<word>` is the last route word before the identifier. Each is reduced to lowercase letters and digits and cut to 12 characters, so the scheme stays well under 32. A label that is empty, starts with a digit, or starts with `web` becomes `site` or `page`.
       - A number is appended when the type is already taken by the config, by a starter adapter, or by another kind in the same draft. So every drafted type is unique, and an ID never collides across kinds.
    6. **Rank.** Rules backed by a visited shape come first, ordered by visit count, then rules backed only by link shapes, ordered by link count. At most 16 rules are kept per adapter, and at most 8 adapters per draft.
  - **Shared origins.** The host's shared-origin list (adapter-templates.md §6) records, per origin, the path position and capture name of the tenant, where the URL has one. Examples are `owner` in the first segment on `github.com`, and `workspace` in the first segment on `www.notion.so` and `linear.app`.
    - The drafter makes that position a `{<name>:name|lower}` capture, even when the shaper kept it as a route word, and the ID includes it.
    - If the origin already has scoped adapters that all scope that capture name to the same values, the draft copies the scope. Otherwise scope is pending, and the review pre-fills the current tab's tenant, read from the page's own URL when a kept drafted rule records this page with it, and shows it as "Capture for `<tenant>`", already chosen: one click on Save confirms it. The list stays editable, and `unscoped` with its statement (§7.2) stays the alternative, except for a rule whose only anchor is the tenant. When the URL names no tenant at that position, nothing is pre-filled and the user chooses.
    - Where the list records no tenant position, as for `docs.google.com` and `www.figma.com`, only `unscoped` is offered.
    - This choice is part of the draft flow and does not count as a user edit; every safety check still applies.
    - The tenant is now pre-filled from the real page URL and chosen, as "Capture for `<tenant>`", for the user to confirm with Save; Save stays disabled only when no tenant could be read or the user clears it. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
  - **The agent drafter.** Once an agent is set up (after M2 host invocation), the review offers **Improve with AI**. The agent receives the same shapes and the structural draft inside the untrusted-data block, and returns a draft under the same rules. The recurring-pages hint drafts structurally and offers the same button.
    - **Improve with AI.** Structural first, agent second: the structural draft always renders first and stays reviewable and savable on its own, and one click on **Improve with AI** refines it with the default harness from settings (no prompt, no picker). **Superseded:** this bullet said the recurring-pages hint's draft came from the agent whenever one was set up; the hint now drafts structurally like every other review and offers the same button.
    - **Run kind.** A service run has a `kind`, `summary` (the default, every older record) or `site_draft`. A site-draft run reuses the run machinery (records, FIFO queue with one queued-or-running run per kind and key, here the origin; timeout and process-group kill, cancel, recovery, retention, `get_run` polling, one repair turn) and branches only on its prompt (`site_draft_prompt` in `host/src/service/prompt.rs`), its input file, its answer finisher (`SiteDraftFinisher`, which writes nothing) and its working directory, which is its own run directory under the service's support folder, never the knowledge base.
    - **Host-built input.** The extension sends `start_site_draft` with the origin and private shapes only, as `draft_site` does. The native host re-runs the structural drafter and builds the untrusted-data block itself in the run's `page.md`: the origin, the visited and link shapes with counts, and its own structural tables, with marker look-alikes escaped. The catalog's rules (blocked route words, search, paging and secret keys, the forbidden keys) are in the trusted prompt, which names `page.md` by path and goes on stdin or JSON-RPC, never a command line. The host sets `kind: "site_draft"` and refuses a `start_run` from the extension that carries a `kind`.
    - **Form-only validation in the service.** `parse_site_draft_answer` (`host/src/site_draft_answer.rs`) accepts one JSON object whose only key is `adapters`: 1 to 8 tables, each for the run's origin, at most 16 rules each, each parsing as an adapter-templates value, the whole answer at most 64 KiB. A failure gets one repair turn naming the field at fault (never the answer text); a second failure ends the run as `invalid_output`, and nothing is shown. The service does not strip or repair forbidden keys or failing rules: they pass unchanged so the host sees what the agent tried.
    - **Host judgment.** When the run succeeds, the panel sends `draft_site` again with the run's `run_id`. The host fetches the validated tables (`RunView.site_draft`) and judges them with `structural::check_agent_draft`: any table carrying `title`, `scope`, `unscoped`, `alias`, `fields` or `path_case` rejects the whole agent draft as `draft_forbidden_key`, and a table for another origin, with a mismatched ID scheme or that does not validate, as `draft_invalid_rule`, before any preview. Otherwise every rule goes through `check_draft_rules` as `Drafted` (never warn-only) against the same existing document, recorded contracts and accepted tables as the structural pass, types are made unique, a kept rule that captures any input shape the structural Keep step refuses for a blocked route word in any segment is dropped as `draft_blocked_route` whatever capture, `{_}`, alternation or `{*}` reached it, and the review fields and match preview are built as for a structural draft. A kept rule that also captures a link shape none of its visited shapes is starts unchecked as `link_only`, so an agent can never widen a visited rule over pages only the page's links suggested. The answer is `site_drafted` with `drafter: "agent"`; the panel renders only that, never the run's raw tables.
    - **`unseen`.** An agent rule that matches no visited or link shape starts unchecked, labeled "suggested by AI: no page you visited or linked here matches it", so the agent can offer Tier 0 rules the structural pass missed without silently widening capture.
    - **Save unchanged.** The panel saves an agent draft exactly as a structural one: `check_site_draft` re-judges the selection with all-`drafted` provenance (the drafter is not sent), then the revision-checked `update_config`. **Use basic draft** returns to the structural draft. A failed, canceled, unparseable or wholly rejected AI draft keeps the basic draft on screen with one line saying why. Closing the review cancels an unfinished site-draft run; reopening the review on the same origin reattaches to it.
    - **Link shapes in the run directory.** Link shapes still never enter the worker sample, the delivery buffer, `update_config` or the home folder, but a site-draft run's `page.md` now holds them under the service's support folder, with the same 30-day/200-run retention as a summary's page text, which is more sensitive.
    - **Without a harness.** Capture Site keeps working; **Improve with AI** is disabled with the setup hint Summarize & log uses. On Windows, `start_site_draft` and `draft_site` with a `run_id` answer `service_unavailable`.
  - **Review before enabling.** The draft opens in the settings page's adapter editor with the match preview, which covers the visited and link shapes and the probe URLs. It shows:
    - each proposed adapter and rule in plain words, with an example URL and a checkbox to keep or drop it;
    - each type and ID scheme, which the user can rename;
    - the current tab title, so the user can add a title entry by hand. Until the user adds one, the whole tab title is used (§7.2).

    Two kinds of rule start unchecked, and the review labels why:
    - a rule backed only by link shapes, labeled "seen only in links on this page";
    - a rule whose only identifier is a `token` capture, labeled "loose: its ID has no fixed shape". Such an ID could be a session value as easily as a document ID.
  - **Checks and trust.** Checks apply rule by rule.
    - A drafted rule the user has not edited must pass every draft check: parsing, the draft structure rules (adapter-templates.md §10.4), overlap, the probe URLs, and the query-parameter lists. A failing rule is dropped and the review says why. If no rule survives, the editor offers an empty adapter for the origin instead.
    - Unchecking a rule, renaming a type, adding a title entry, and choosing scope never relax the checks on any rule.
    - A rule the user edits becomes user-written, and its draft-rule failures then only warn, as for any user-written adapter (§7.2).
    - Enabling saves through the revision check alone (§7.2). (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))

  **Capture Site for starter sites.** The panel now
  offers **Capture Site** on previously unconfigured Jira Cloud,
  SharePoint/OneDrive and GitHub origins. The existing catalog resolves the
  first two from the current URL's origin. GitHub offers repositories, pull
  requests, and issues, all three checked by default and sharing one
  organization field; unchecking a type excludes it, and the shared scope
  applies to whichever types stay checked. The field is pre-filled from the
  current URL's owner segment (also from `/orgs/<org>/…`), lowercased and
  shown for confirmation rather than applied unseen; a page with no usable
  owner, such as `github.com/`, a reserved top-level route (`settings`,
  `notifications`, and the rest of `githubOwnerFromUrl`'s list), or `/orgs`
  with no org, gets no pre-fill and asks instead. The confirmation line
  names only the types actually checked ("Records repositories, pull
  requests and issues owned by: …"), never more than the save. No scope is
  inferred from another kind or an existing definition. The inline review
  describes the remembered work, tests the real current URL, labels
  synthetic examples as illustrative, and lists the results of all 27
  existing non-artifact probes. A probe match refuses this starter flow.
  Starters retain their development-draft status until the human live-tool
  certification; their frozen definitions are unchanged. This slice neither
  reads page content nor runs an agent.

  Saving is one v6 `update_config`, with the reviewed revision retained across
  the shared mutation lock, a fresh host-config read and a fresh active-tab
  check. A change of tab or URL cancels the review; an external revision makes
  it stale. Notes finish saving under the old identity before config changes,
  retain copy-out on failure, and reload the host-derived key afterward, with
  title-triggered loads suspended during the save. Page-installed policy also
  seeds the worker's existing top-frame tab map, so measurement starts on
  already-open recognized pages without inventing opens. Background renewal
  leaves its pending real navigation to record the open after renewal.
  A configured origin is managed in settings rather than replaced or
  supplemented. The first site
  starts capture when no local pause is pending; adding another site respects
  a global pause. A pending local pause or removal blocks setup. Policy
  verification/publication failures after commit are reported as saved with
  status unavailable, so no automatic second save occurs. Unknown origins
  use the drafted flow below (`brwr-t40`); re-keying remains unbuilt.

  **Private page-pattern review.**
  The panel's collapsed **Page patterns** review works on any public HTTP(S)
  tab. It uses a self-contained function, shared with the worker shaper, in
  the isolated top frame (`frameIds: [0]`). It checks exact URL, origin and
  top-frame identity before reading hrefs and rechecks URL/origin afterward;
  the panel checks the active tab both before and after the call and discards
  canceled or superseded results. It scans at most the first 500 anchors,
  including off-origin anchors in that budget, keeps only same-origin links,
  and returns at most 50 counted shapes. Singleton route literals are judged
  across distinct shapes, so duplicating one planted link cannot preserve
  its literal. Link shapes remain only in the live review and are cleared on
  navigation, close or collapse, with no storage-session handoff yet.

  The worker samples real top-frame navigations on enabled origins after
  excluding incognito tabs, even when no adapter matches the page. Sampling
  does not widen activity capture. The bounded table, separate from activity
  and delivery, stores only shape, capped count and last-seen timestamp; it
  keeps the 20 most recently seen shapes per origin (ties sort by shape),
  expires them after seven days, and purges on pause, policy suspension or
  removal. Startup, maintenance alarms and explicit reads also prune it;
  inactive policy cannot authorize sampling. Settings offers local show and
  clear controls, including while the host needs repair. A clear reports a
  storage failure; best-effort sampling/maintenance failures cannot interrupt
  artifact capture. Samples never go to the host or notes folder in this slice.

  Shaping refuses credentials, non-HTTP(S) URLs and inputs over 8,192 characters,
  32 path segments, eight query pairs or 2,048 shape characters. Query order
  is canonical; fragments disappear. Counts saturate at 1,000,000; total
  origins cannot exceed the 64-adapter limit. Validation strips unknown stored
  metadata and drops malformed/expired shapes. Short lowercase route words
  remain visible in the review. The review creates no adapter, draft token,
  native message or agent run. These shapes are the only page-derived input to
  unknown-site Capture Site (below); known-site Capture Site retains its
  current flow.

  **Restoring recorded site rules.**
  On an unconfigured origin, an explicit Capture Site click first reads
  `get_site_restore` / `site_restore_loaded` (additive protocol v8), sending
  only the exact origin and reviewed config revision. Usable contracts from
  `adapter_id_history` and hand-removed adopted contracts take precedence over
  a new starter or structural draft. The panel names their page kinds, route
  shapes and exact account scope, including an explicit every-account line
  for recorded `unscoped` rules. This restores recognition only: title cleanup,
  aliases from titles and reference extraction were not recorded and are not
  reconstructed. Existing notes retain their host-derived keys and bytes.
  The host deduplicates exact contracts and refuses damaged records or any
  overlap between recorded adapters, including container fall-throughs whose
  note-only evidence names several identities. It retains the existing repair
  guidance; neither Forget nor migration is implicit. The panel re-reads the
  recorded set before a normal revision-checked `update_config`, refuses a
  changed set, navigation, revision changes and pending local pauses/removals,
  and finishes note edits through the existing recovery-aware save boundary.
  Restoration preserves `capture_enabled` even when this is the only site;
  a paused installation stays paused. With no recorded contract, the ordinary
  starter/drafter flow proceeds. No page patterns or content are needed for
  restoration and the log/cache formats are unchanged.

  **Capture Site for unknown sites.**
  On any credential-free HTTP(S) origin that has no adapter and is not a
  starter origin, the panel's **Capture Site** button drafts a policy with the
  host's deterministic structural drafter; no agent runs, and Chrome shows no
  prompt and the host no dialog. Open access maps the superseded flows above
  as recorded for step 7.6 in [docs/history.md §12.2](docs/history.md#122-next-steps--history).
  - **Messages.** Two request/response pairs were added within protocol v6
    (protocol/README.md, "Version 6 additions: Capture Site drafting"), and no
    existing message changed shape. `draft_site` carries only the exact origin,
    the visited shapes (the current page's shape plus, on an enabled origin,
    the worker's sample, at most 21) and the click-time link shapes (at most
    50). The host refuses a raw URL or any shape its parser
    (`adapter-templates/src/shape.rs`) rejects. `site_drafted` is bound to the
    config revision it read and returns at most 8 adapters of at most 16 rules
    with per-rule provenance (which rules start unchecked, and why), the
    dropped shapes and rules with reasons, and a match preview of every
    visited shape, link shape and probe URL, filled with the catalog's
    illustrative values and matched without scope. The panel evaluates the
    real current URL itself; no raw URL, link `href`, or tab title reaches the
    host. Link shapes live only in the open review and, for the advanced
    editor, in its expiring handoff entry (below); they never enter the
    worker sample, the delivery buffer, `update_config` or the home folder.
  - **Catalog.** The probe paths, search and paging keys, blocked route words,
    secret query keys, mode words, fill values, starter types and
    shared-origin tenant and account-index positions are one host-owned file,
    `protocol/capture-site/catalog.json`. The host parses it; the extension's
    probe list, blocked route words, fill values and starter types are pinned
    to it by tests.
  - **Secrets and capability routes.** A query pair whose key carries a
    secret or capability (a catalog `secret_query_keys` entry such as
    `token`, `code`, `state`, `sig` or `session`, a key containing a
    `secret_query_key_parts` fragment such as `token` or `signature`, or one
    ending in `sig`; ASCII case and `-`, `_`, `.` are folded) never becomes
    an ID: the drafter drops the pair, and the shape as `secret_query` when it
    has no other identifier, and the per-rule check drops an ID capture from
    such a key as `draft_secret_capture`. A route literal is a blocked route
    word when any of its `-`/`_`/`.` parts, or the parts joined, is a listed
    word once a trailing number and a plural `s` are folded, so `sign-in`,
    `reset-password`, `accept-invite`, `invitations` and `tokens` are blocked.
    The check runs on every literal the shaper kept, the shared-origin tenant
    position included, before that position is marked: `linear.app/auth/…`
    is a sign-in route, not a workspace named `auth`. Neither the review's
    prefill nor a chosen scope value may be a blocked route word.
  - **Per-rule checks.** `check_draft_rules` (adapter-templates.md §10.4)
    judges each rule: a failing drafted rule is dropped with its reason and the
    rest of the draft kept, a failing user-written rule only warns, and an
    adapter with no rule left is removed. Overlap is checked against the
    origin's existing adapters, its recorded note-only contracts (retired
    `[[adapter_id_history]]` entries and adopted contracts removed by hand),
    and then the earlier-ranked drafted ones.
  - **Pending scope.** On a shared origin with no scoped adapter, or with scoped adapters that disagree, scope is pending rather than refused. The review pre-fills the current tab's tenant from the panel's own URL (on GitHub through the starter's owner reading, which also rules out GitHub's own routes) and checks "Capture for `<tenant>`", so Save is a one-click confirmation; the list stays editable and the review adds "Records pages only for: `<tenants>`." A value counts only when one of the adapter's kept drafted rules, scoped to that value, records this tab's page with it as the tenant, so a page slug or app route at that position (`linear.app/settings/account`, a workspace-less Notion page) is no tenant. When the URL names no tenant there, nothing is pre-filled and Save stays disabled until the user enters values or checks the every-account statement. A rule whose only anchor is the tenant cannot be saved unscoped: the panel says so, and `check_site_draft` refuses it as `draft_scope_choice`. Where the catalog has no tenant position, only `unscoped` is offered; an agreeing scope is copied and fixed. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))
  - **Panel-owned overlays.** Scope, `unscoped` and a title entry (`{title}`
    plus an ending the user removes, which the panel takes from the tab title)
    are applied after the check. `check_site_draft` re-checks only the Tier 0
    tables after unchecking and renaming, with the scope choice as a separate
    per-adapter field that `resolve_scope_choice` judges, so a chosen scope or
    a title entry never trips `draft_forbidden_key`, while a table that itself
    carries `title`, `scope` or `unscoped` with a drafted rule is still refused.
    There is no title field on the wire.
  - **Save.** Under the shared config mutation lock, the panel re-checks the
    selection with `check_site_draft` against the reviewed revision (a changed
    revision is `conflict`, never rebased), and any dropped rule or scope
    verdict with a reason saves nothing. It then saves the overlaid document
    with the usual revision-checked `update_config`, as the starter flow does.
  - **Hint.** On a configured origin, when the current page is unrecognized
    and the worker sample holds an unrecognized shape with a route word that
    was visited at least three times, the panel says "Looks like you have
    recurring pages on `<host>`" and offers **Draft rules for these pages**. The
    extension decides this locally against its lease and sends nothing. The
    sample reaches the host only after that click. The draft adds adapters
    only; existing adapters stay byte-identical, and rules they already
    recognize, even only without scope, are listed as already captured.
  - **Handoff.** **Open advanced settings** writes the drafted adapters,
    their baseline rules and the shapes, never a raw URL or title, to a
    trusted-only `storage.session` entry with a 30-minute expiry and opens the
    adapter editor, which reads it once and deletes it; the worker deletes
    expired entries at startup and on maintenance. A rule whose canonical form
    differs from its baseline is user-written; an adapter given `alias`,
    `fields` or `path_case` by hand is user-written as a whole, so its
    forbidden key only warns. Unedited drafted failures still block the save.
    When the review has no valid plan yet (scope still pending, a refused
    rename, or every rule unchecked), the handoff carries the drafted tables
    with only their checked rules and omits adapters with none, so a
    link-only or loose rule the user never checked is never handed off as
    drafted.
  - **Remap invariant.** A drafted rule never re-maps an artifact the current
    policy or a recorded contract already names. The drafter lists a shape
    the current policy or a recorded contract recognizes as already captured,
    and drops a drafted rule that may overlap either as `draft_overlap`, so a
    site the user removed and captures again gets its old routes listed as
    already captured rather than re-drafted under a new type that the save's
    frozen-ID check would refuse. `draft_site` asserts `remapped` is 0 and
    answers `internal` rather than offer such a draft.
  - **Untrusted links.** A link shape never changes how visited shapes group,
    nor the type, ID scheme or rank of a kind with visited support: when only
    links differ in a kind's varying route word, the kind is named by its
    visited word, and links only rank adapters that have no visits. A query
    name a shape repeats with two classes is dropped, since one key cannot
    hold two conditions.
  - **Limitations.** On a shared origin the shaper keeps a short lowercase
    route-word-like segment literal, so a repository or namespace named like
    `widget` drafts a rule for that one name (`/{owner:name|lower}/widget/pull/{num:int}`)
    rather than a `{space:name}` capture; other repositories need their own
    visits or the advanced editor. The rule is never wider than what was seen.

  **Drafting contract: anchors.** #126 made a required path capture a `scope` key restricts count as an anchor (adapter-templates.md §4.1, §6), for any shared origin with a tenant capture. Under that rule the drafter keeps no tenant rule of its own: the anchor before a drafted rule's first identifier is a literal or the scoped tenant. Notion workspace pages are therefore drafted, safely: both forms become one `notion.page` rule, `/{workspace:name|lower}/{hex:hex(32)@tail}`, which records only the workspaces the review scopes (pre-filled from the page), every other check still applies, and it can never be saved `unscoped`, since that would leave it with no anchor. A workspace-less `/<hex32>` page is still not drafted (adapter-templates.md §13). Behind a route word on a non-shared origin, `/p/Release-Plan-<hex32>` and `/p/<hex32>` draft to one rule with one shared `hex(32)@tail` ID. Fixed-shape captures and plain alternations still do not anchor a draft. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))

  Create adapter with the structural drafter needs no agent, only `scripting`, which arrives with Tier 1 (§12.2 step 7.3). It is built in step 7.6. Agent drafting (step 7.7) needs M2 host invocation (step 8.2).

**Engagement.** Brauser records what it measured, not what it infers. It does not label an artifact "worked on"; it records "focused 42 min". The worker measures attention only on tabs whose current document its prefilter recognized, and keeps two separate numbers:

- **Focused:** the tab is the active tab in the focused Chrome window.
- **Background:** the tab is still the active tab of the last-focused window, but focus has left Chrome, for example to take notes in a terminal beside the doc. Brauser cannot tell whether the tab is still visible, so this time is kept separate and never added to focused time.

Either measure stops when the user switches tabs (`tabs.onActivated`), navigates in that tab, or closes it (`tabs.onRemoved`), or when `idle.onStateChanged` reports idle or locked. Focus moving between Chrome and another app (`windows.onFocusChanged`) moves time between the two measures and does not stop it. None of these events needs the `tabs` permission. The worker sets the idle detection interval to 5 minutes (configurable; Chrome's minimum is 15 seconds) instead of the 60-second default, because reading a long document involves no keyboard or mouse input. Chrome throws on a value below 15 or a non-integer instead of clamping it, and has no getter, so the setting is validated as a whole number of seconds of at least 15 and the worker sets it each time it starts. On `idle`, the time up to the idle report counts, so up to one detection interval of reading without input is counted. On `locked`, counting stops at the lock. Searches and filters use the measured values with visible thresholds, such as "focused at least 5 min", so no time range is left unclassified.

**Opens.** An open is a cold open: a navigation that makes a tab show an artifact it was not showing before. That means a main-frame commit, or an SPA route change (`onHistoryStateUpdated`) to a different artifact ID, from a link, a typed URL, a bookmark, a link opened in a new tab, or back or forward from another page. The worker counts one when the tab-to-artifact map (Trails, below) changes that tab's entry to this artifact.

Tab switches are not opens and are not logged at all. Switching to a tab that already shows the artifact (`tabs.onActivated`), Chrome regaining focus, and returning from idle or lock only start and stop the focused and background measures. Discarding a tab gives it a new tab ID. Chrome reports the change at the discard with `tabs.onReplaced` and `webNavigation.onTabReplaced`, before any switch to it, and the switch's `tabs.onActivated` then arrives with the new ID ahead of the reload commit. The worker therefore moves the tab's map entry and any pending source to the new ID on those events.

A commit with transition type `reload` is never an open, even when the map has no entry for the tab. Chrome uses that type for the user's own reload, for the reload when the user switches to a tab Chrome discarded to save memory, and for tabs restored with a session. A restored or reopened tab has a new tab ID and no map entry. The worker sees a restored tab's commit only if Chrome has already loaded the extension, with its listeners, when the tab commits. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))

**Rebuilding missing entries at worker start.** So that the result does not depend on Chrome's startup order, the worker, on every start and as the first task in its serialized queue, lists tabs with `chrome.tabs.query({})`, which gives every tab's ID without `tabs`. For each tab with no map entry it reads the committed main-frame URL with `webNavigation.getFrame({tabId, frameId: 0})`. It adds an entry only for a URL on a granted origin that the lease prefilter recognizes as an artifact; `getFrame` also returns URLs on ungranted origins, so this check is the same one events get. A rebuilt entry counts no open and records no trail source, as for a `reload` commit. It only starts the tab's background measure, or the focused measure when the tab is active in the last-focused window. Existing entries are left alone, and events that arrive meanwhile queue behind the rebuild, so a later commit still overwrites the entry. The same pass also repairs the map after an extension update or reload, both of which clear `storage.session`. The cost is one `tabs.query` and one `getFrame` per unmapped tab per worker start. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))

A route change within the same artifact only adds a mode; examples are a pull request's `/files` tab and a Google Doc going from `/view` to `/edit`. The same artifact opened in two tabs is two opens.

The panel says "focused 42 min · 15 min in background · opened 3 times". An hour or day with focused time but no open, such as a tab left open from the day before, shows only the time.

**Coarse log.** The log gets one line per artifact per active hour. Without this, every change to the focused and background measures would be its own line. That would be dozens a day for one ticket, because every tab switch starts or stops a measure.

The worker keeps one pending **activity** total per artifact per local hour. The total is keyed by provisional `artifact_id`, local date, hour, and UTC offset, and is kept in trusted-only `storage.local` so it survives a browser restart. It holds:

- the open count (Opens, above);
- focused and background seconds;
- the modes seen;
- the latest canonical URL, title, and Tier 1 values;
- the distinct trail sources (`from` and transition), capped at 10.

A tab switch changes only the seconds; nothing records that the switch happened. When the hour ends, the worker freezes the total into one `activity` event with a stable event ID. Delivery sends only frozen events. The panel's live view reads the current hour's pending total from the worker.

A heavy workday then produces at most one line per artifact per hour. The cost is that the log knows only the hour in which something happened, and a measure still running when the browser exits is lost. The host appends a delivery batch as one write, one sync, and one intent record covering every event ID in it, not one durable intent file per event.

Each daily-log line Brauser writes carries an owned machine marker after its human-readable text: `kind=activity`, `kind=why`, or `kind=summary` (overflow markers use `kind=overflow`) with a hex-encoded canonical wire payload. The index and `brauser reindex` rebuild modes, trails, refs, Why text, and summaries only from these markers, never from the prose rendering. The primary, mode-evidence, and trail URLs are normalized only after authorization.

**Trails.** Each visit also carries how the user got there. `webNavigation.onCommitted` gives the transition type (`link`, `typed`, `auto_bookmark`, `form_submit`, and others). The worker keeps a map from each tab to its current document's provisional artifact in `chrome.storage.session`, built from the same `onCommitted` and `onHistoryStateUpdated` events it already handles. An entry is cleared when that tab moves to a page that is not an artifact, so a new tab opened from a Jira filter page is never credited to whatever ticket that tab showed before. Edges are captured when they happen:

- **New tab or window.** `webNavigation.onCreatedNavigationTarget` fires when a link opens a new tab or window (middle-click, `target=_blank`, `window.open`). It gives the new `tabId` and the `sourceTabId`, and the worker looks up the source tab's artifact at that moment. The worker stores it as the new tab's pending source, and the new tab's first recognized commit consumes it. Reading an opener tab's URL later would record the wrong edge whenever the opener had navigated since, which is common when the user middle-clicks several results from a list and keeps going. This event is part of `webNavigation`, which logging already requests, so `openerTabId` is not used, although the step 6.1 probe found it readable without `tabs`.
- **Same tab.** The map's previous entry for that tab is the source of a link or form navigation. Back and forward keep the history entry's original transition type (`link`, `typed`, and so on) and add the `forward_back` qualifier, so the worker checks the qualifier first: a back or forward is recorded as such, with no same-tab source, and is not taken for a link.

The host accepts only `link`, `typed`, `auto_bookmark`, `auto_subframe`,
`manual_subframe`, `generated`, `start_page`, `form_submit`, `reload`,
`keyword`, `keyword_generated`, and the worker's `forward_back` encoding as
trail transitions. A new single or batched activity with another value is
terminally `rejected` as `invalid_activity`, before log or index writes.
Historical owned markers and ingestion caches may contain older, bounded
unknown values: their activity and exact event payload remain readable for
replay identity, but unknown transitions never produce derived edges. Reindex
retains the original trail size and paired-source checks and does not rewrite
the authoritative log.

A source is recorded only when it is a recognized artifact, which means it is on a granted origin. `webNavigation` events, including `onCreatedNavigationTarget`, carry the URL of every main-frame navigation whether or not its origin is granted; only tab metadata (`tabs.onCreated`, `tabs.onUpdated`) is limited to granted origins. The worker therefore drops an event whose URL is not on a granted origin with an enabled adapter before storing or mapping anything from it. The host re-resolves both ends at delivery, which gives edges such as "opened PROJ-123 from PR #412". The host never rewrites a delivered event, so a trail whose source no longer resolves to the same artifact rejects that whole hour as `trail_identity_mismatch` before any log write, exactly like a forged one. The worker therefore keeps only trails the host will accept: when a lease narrows (a site removed, or an origin carried into the pending-removal list), it drops every trail whose source no longer resolves under the lease from live totals and from queued events not yet dispatched, and it checks again as each batch is dispatched. A dispatched event stays frozen for interrupted-write recovery, so an hour already in flight when its source site is removed can still be rejected. An adapter's `refs` add *mentions* edges, for example an issue key in a pull request title. A ref scans only the extracted title, so an artifact never refers to itself through its own title prefix, matches issue keys only in uppercase, keeps at most 20 distinct refs per event, becomes a shown edge only once its target has been recorded as an artifact (so `UTF-8` never appears as a Jira issue), and resolves to a known artifact only when exactly one enabled adapter of the target type exists and its `id` template has exactly one placeholder besides `{host}` (adapter-templates.md §10.1). Otherwise it stays an unresolved key. *Co-focused* edges, between artifacts focused on in the same hour, are derived in the index and never written to the log.

**Aliases (M1.5b).** Artifact IDs change. A Jira issue moved to another project gets a new key (PROJ-123 becomes OPS-45), and the old key redirects. GitHub repositories are renamed and transferred. Confluence Data Center reaches one page through `/pages/viewpage.action?pageId=`, `/display/SPACE/Title`, and `/x/` short links. Without aliases, an artifact's history would silently split in two. Brauser keeps each artifact under one canonical ID plus a set of aliases:

- **Detection.** An alias is recorded only from evidence that the two IDs name the same thing:
  - **Server redirect.** The worker pairs `webNavigation.onBeforeNavigate`'s requested URL with the committed URL in the same tab and frame. If the commit's transition qualifiers include `server_redirect` and both URLs are recognized as different IDs of the same adapter type, that is an alias. Both URLs are on granted origins; a redirect through an ungranted origin is never seen.
  - **Declared identity signal.** An adapter can name a second source for the ID. One is the tab title, for example `[OPS-45]` in the title while the URL still says `PROJ-123`. The other is one named `<meta>` element's `content`, for example Confluence Data Center's page-ID meta tag on a `/display/` URL. This is the only case in which the fixed extractor (Tier 1) reads an attribute, and it reads only the `content` of `meta` elements the adapter names. Both signals come from text the page controls, so they are recorded as **suggested** aliases: they change record keys only after the user confirms them on the artifact card, or after a server redirect between the same two IDs corroborates them (adapter-templates.md §9.3).

  An SPA route change from one recognized ID to another is never treated as an alias, because it looks the same as clicking through issues in a board modal.
- **Recording.** The worker adds `{from, to, evidence}` to the hour's activity total, and the host writes it as an append-only `alias` event, after re-checking that both IDs are ones the confirmed adapter produces. A URL form that carries no stable ID, such as `/display/SPACE/Title` without the meta signal, is recorded under a provisional ID of that form and joins the canonical artifact once an alias names it.
- **Resolution.** The index groups confirmed and corroborated aliases transitively; suggested aliases are listed on the artifact card but not grouped. The ID most recently seen as a redirect target becomes canonical, so the new Jira key wins, and a later move back is handled the same way. Activity, edges, notes, and refs recorded under any alias count toward the canonical artifact, and a `refs` match on an old key resolves through the alias table.
- **Files.** The artifact file lives at the canonical ID's path. When an alias changes the canonical ID, the file at the old path is replaced, under the checked rename (§4.4), with a short Brauser-owned stub (`brauser.kind: artifact`, `alias_of: <canonical id>`) that links to the new file, so links in the user's own notes still lead somewhere. The frontmatter of the canonical file lists `aliases`.
- **Correction.** A wrong alias would merge two histories, so the artifact card shows "Also known as PROJ-123" with **Not the same**, which appends an `unalias` event. The log stays append-only, and the index honors the latest `alias` or `unalias` event for each pair.

**Re-learning groundwork.** The offline developer
command `brauser adapters aliases-preview` rehearses confirmed, suggested and
unalias operations against supplied old/proposed adapter documents and URL
evidence. Its replay groups confirmed edges, chooses the latest active
confirmation's target in append order and treats unalias as removal of that
pair's edge. Suggestions never group identities. The command reads no live
configuration or vault and applies nothing; it does not enable Re-learn or
relax the ID freeze. The [staged implementation plan](docs/superpowers/plans/2026-10-28-capture-relearning.md)
documents its JSON interface, provenance checks, replay rules and the remaining
impact-planning, durable-migration and panel work under `brwr-6x2`.

**Containers.** An adapter can declare that its artifacts sit *within* a container artifact, such as a GitHub pull request within its repository (adapter-templates.md §3, §8.3). A visit recognized as artifact A within container C counts toward A exactly as it always has, and also rolls up into C: C gains the visit's focused and background seconds and its open, and C's `days_opened` is the union of its own days and its contents' days, never a sum. Because a visit is recognized by exactly one adapter (first match wins, §3), each second counts once toward A and once toward C, never twice toward C. The index records the relationship as a `within` edge from the contained artifact to its container, alongside `mentions` and the derived `co-focused` edges. While some adapter declares `within` C's type, C's title and URL come from its ID, never from whichever sub-page's title happened to be seen last, so a container with rolled-up activity but no direct visit yet still shows a usable title (for GitHub, `owner/repo`) and a link built from its captures (`https://github.com/<owner>/<repo>`). Recent and the worklog show C as one row with its rolled-up totals and A as its own row; a day's or period's total counts each visit once, skipping the contained copy of a roll-up. Existing history rolls up too: because the index is rebuilt from the append-only logs, a logged visit rolls up whenever re-evaluating its original URL under the current adapters still yields the same `artifact_id` and the matching adapter declares `within`; an event whose adapter has since changed or been removed is left alone rather than misattributed. Container files follow the adapters as well: when an adapter change takes a container's roll-up away, or the last declaration of its type, its file is rewritten with only its direct visits and its own title, and a container whose only activity was that roll-up no longer has a record, so its generated file is removed (only while it is still Brauser's file for that ID; a replaced or edited-away file is left alone). A container file whose record includes rolled-up activity carries `brauser.rollup: true` in its frontmatter. The index cache remembers where it wrote each container's file, and `brauser reindex` or a rebuild of a lost cache recovers that list from the previous cache if there is one and otherwise from the owned files on disk that carry the marker, so a container that no longer derives a record still has its file removed after a rebuild. The artifact card names an artifact's container, and a container's card lists what was worked on inside it (§5.7); nesting more than one level deep, and an artifact with more than one container, are not supported.

`github.repo` is the first container (and the first starter adapter added since M1.5a step 6.8): a catch-all over everything inside `/<owner>/<repo>/…` on `https://github.com` that no `github.pr` or `github.issue` adapter already claims, scoped to the owner like the other two. Because adapters on one origin are tried in config order and a PR or issue visit already rolls up into its repository through `within`, PR and issue adapters must come before the repo catch-all on `https://github.com`; every way of adding any of the three (the panel's Capture Site, the settings gallery, and the JSON editor) keeps that order regardless of the order the user checked them in (`orderContainedFirst`, extension/starter-adapters.ts). The PR and issue starters declare `within` only when a `github.repo` rule is kept on the origin, chosen in the same save or already configured (`linkedToPresentContainers`); choosing pull requests or issues without repositories records no repositories, and adding repositories later links them then (§7.2). `github.repo`'s catch-all rule, `/{owner:name|lower}/{repo:name|lower}/{*}`, has no literal route word; it validates because a required path capture a `scope` key restricts to a literal list counts as an anchor segment too, the listed GitHub organization standing in for a literal (adapter-templates.md §4.1, §6). This is a general rule for any shared origin with a tenant capture, such as a Notion or Linear workspace, not a GitHub exception; an `unscoped` adapter, or a capture scope does not restrict, still gets no such anchor. Enabling the repo catch-all over already-configured PR or issue adapters would otherwise trip the overlapping-route guard below (§6.2): any past edit to their `scope` leaves a retired contract whose routes the catch-all overlaps under a different type. The guard is amended to accept that overlap when the new adapter is exactly that retired contract's declared container — same type, same ID template string — because those URLs were already counting toward the container. A retired contract counts as declaring the container when it declares `within` itself, or when another contract with its same origin, type, and ID template declares it: a live adapter in the saved config, or another contract already retired or removed. While the PR or issue adapter is live its routes still win (first match); only once it is removed do its URLs fall through to the container, and the PR or issue's finer-grained identity stops being recorded. Settings adding repositories to an existing GitHub setup adds `within` to the user's PR and issue adapters in the same save (§7.2). That edit still retires the old `within`-less shape into `[[adapter_id_history]]`, like any recognition edit, but its same-identity successor declares the container, so the old shape never blocks the repository adapter, including after the PR or issue adapter is itself removed later (which retires the `within` shape too).

**Mode.** Many tools reveal a mode in the URL: Google Docs `/edit` versus `/view` or `/preview`, Confluence's edit paths, and GitHub's `/files` review tab. An adapter maps these URLs to `view`, `edit`, or `review`. The record states what the URL showed: "opened in edit mode" is recorded, but "edited" is not claimed. Google Docs, for example, serves `/edit` to many view-only readers.

**Why I'm here.** The artifact card has an optional one-line note. It is appended to the log as its own event, and the latest one is shown. It is separate from the page note (§5.3). Most people will skip it, but when they do write one it is often the most useful entry in the log. While this page's origin has a pending local removal, both Save and its handler refuse and point to settings to complete the removal; another origin's pending removal does not block this page.

**Recent and worklog.** Every tool being adapted already has its own "recently viewed" list. What Brauser adds is one list across all of them, and that has to be visible in M1.5a, not only on the artifact card of a page the user is already on:

- **Recent tab.** The panel lists artifacts grouped by local day, newest day first, and within a day sorted by focused time. It can be filtered by type (for example, only `jira.issue` or `github.pr`) and by a title substring. Each row shows the title, type, focused and background minutes, and the one-liner if there is one, and opens the artifact's last URL. It reads the index (§5.5) through a `list_artifacts` message, so it needs no FTS5.
- **Worklog.** An optional daily or weekly summary, generated by the host from the index without an agent. It uses the same measured wording as the rest of the log, for example: "Opened 9 artifacts, and focused on 2 left open from an earlier day. PROJ-123 *Release blocker — SBOM export times out*: focused 2 h, opened from PR #412. Why: blocking the release." The panel shows yesterday's, today's, or this week's worklog with Copy, for standups and status reports. When the user turns on worklog files (M1.5b), the host also writes Brauser-owned `worklog/<YYYY-MM-DD>.md` or `worklog/<YYYY>-W<ww>.md` files (`brauser.kind: worklog`). They are regenerated lazily and skipped when unchanged, like artifact files (§6.7), and are not rewritten after their period ends unless `brauser reindex` runs. Worklog files are off by default, and the settings page shows the sync caution below before they are enabled.

**Known gaps when sharing a worklog.** The panel
requests an additive protocol-v8 `worklog_report` and receives `worklog_reported`:
measured text, artifact count, and up to 32 `OverflowMarker` notices from one
caught-up index snapshot, plus `gaps_truncated`. The original `worklog` pair
is unchanged. Only complete machine markers in owned daily logs supply gaps;
attached markers use their overflow payload's range, not the anchor visit's
log date. Standalone markers work without any artifact activity.

A notice intersects an inclusive day/week when the enclosing range of its two
recorded local endpoint dates intersects that period. Each endpoint keeps its
own UTC offset (including DST or travel); UTC dates and current browser offset
never re-date it. The full reported range and count are shown even when only
part intersects: the log cannot apportion missing records among dates. Counts
mean dropped completed artifact activity-hour records, not elapsed wall-clock
hours or measured focus. Retry copies with the same marker ID appear once;
conflicting copies choose the earliest sorted log path and line. Distinct IDs
stay separate even when ranges overlap or payloads happen to agree. They are
never summed or merged, and no missing activity is assigned to artifacts,
opens, focus totals or Recent rows. Notices sort by first instant, last instant,
then marker ID; truncation is explicit in both the review and shared note.

The review always shows known notices. **Include capture gaps in copied
worklog** starts checked and adds a plain incompleteness note with those exact
ranges/counts to the text preview; clearing it removes only that note from the
preview and Copy. The choice survives period changes and delivery refreshes
while the panel stays open. Copy always copies the preview. Loading/error
states disable Copy and hide stale notices; late replies cannot replace a newer
period. An older host refuses the new request and leaves Copy unavailable until
both components are rebuilt. Reindex regenerates the same notices without
changing source logs. Cache format 8 retains event checkpoints from formats 4–7
and schedules a bounded one-time gap rescan with separate gap checkpoints;
partial passes remain unavailable and resume normally. SQLite artifact tables
are unchanged; the JSON index cache holds these system notices.

As built in M1.5a, `list_artifacts` returns at most 32 artifact-day rows per request and reports truncation; the host accepts worklog ranges of one through seven inclusive local dates and caps generated text before the native-message ceiling. Worklog files remain deferred to M1.5b.

**Sync caution.** Titles of confidential tickets and documents sit in the home folder. The folder is the user's choice. (history: [docs/history.md §5.1](docs/history.md#51-artifact-logging--history))

**Rules carried over from M1.**

- Logging is **opt-in**: a site is on exactly when the user has added an adapter for its exact origin (protocol v6; before it, the origin also needed a Chrome grant and a site entry). The host independently checks scheme, host, and the adapter's match rules before recording a visit.
- Incognito and private windows are never logged, regardless of config. The extension manifest uses `incognito: "not_allowed"`, and the event handler resolves the tab's incognito state and rejects it defensively.
- A URL no enabled adapter recognizes is dropped by the worker and never buffered. If one arrives anyway (a stale lease, an engine mismatch, or a compromised worker), the host returns the terminal outcome `suppressed` with reason `not_an_artifact` and writes nothing to the home folder.
- **Buffer pressure.** The buffer holds only artifact events, so it fills far more slowly than a buffer of every visit on an origin would. Hourly activity totals (Coarse log, above) keep it small: each artifact adds at most one buffered event per hour, however often it is opened or switched to. At 80% of either the count or byte cap, the worker sets the toolbar badge (`action.setBadgeText`, no permission needed) to prompt the user to open the panel. Overflow is never silent: the dropped count and time range are kept and shown in the panel's capture strip, and an overflow marker is appended to the log on the next delivery. If drops arrive while the final event batch is in flight, the worker follows it with a standalone overflow batch (`record_batch` with empty `events` and non-null `overflow`), without waiting for another capture. Its ID and count/range snapshot are stored before dispatch and reused after lost replies or worker restarts; later drops stay counted for a separate marker. A delayed event batch retains ownership of its attached snapshot until acknowledgement. Standalone failures back off and remain visible; ready activity batches take priority over standalone retries so a refused marker cannot block later captures. Pause still prevents dispatch. The host rechecks capture, adapter policy and the selected home folder under the config lock, validates the count and both offset-qualified times (chronological order and the normal delivery window for the last time), and derives the log day from that last time. A standalone marker has no page URL or artifact: its payload is only the count and range, and its legacy marker URL hash is the hash of the empty string. It uses the existing compact intent, owned-log append, sync and duplicate checks. Anchored overflow batches keep their existing rendering for exact recovery.
- `/log` (§5.6) stays a one-off action on any HTTP(S) page. It records a `web.page` artifact, `web:<host>/<url_id>` (§6.2), unless an enabled adapter recognizes the page, in which case it records that adapter's artifact.
- M1's site entries recorded nothing until an adapter was enabled for the origin; protocol v6 drops `sites` entirely, and a v5 config's site list is ignored and removed on the next save. Existing daily logs are left as they are, and the indexer reads their M1 visit entries as visits to `web.page` artifacts with the same `web:<host>/<url_id>` IDs, so an M1 visit and a later `/log` of the same URL share one artifact. Each M1 visit entry counts as one open with no measured time, in the artifact's `opens`, Recent, the worklog, and its day's Timeline line (for example `- 2026-10-20 · opened 1×`).

**M1 mechanics, unchanged.**

- M1 observes main-frame `webNavigation.onCommitted` and `onHistoryStateUpdated` events, using document identity and lifecycle to avoid subframes, redirects, and duplicate SPA records. It does not keep dwell or scroll timers only in service-worker memory. A committed document's title arrives after commit. An SPA may set a new route's title just after `pushState`, when the title update follows `onHistoryStateUpdated`, or just before it, when Chrome reports the title update first, still carrying the previous route's URL, and the tab's URL-change update (`tabs.onUpdated`) already has the new title. The worker therefore briefly holds every new visit and fills its title from the tab's title at the URL change or from a later title update, and does not credit a title update that precedes a route change to the tab's previous artifact; the event is frozen once handed to the panel because the host hashes it for interrupted-write recovery. Each visit carries its local UTC offset, and the daily log uses that local calendar date.
- M1 normalization drops fragments and configured tracking parameters after host authorization. Artifact adapters (§7.2) do not rewrite URLs. They map a URL to an artifact ID, and the log keeps the normalized URL alongside that ID.
- The host normalizes only HTTP(S) URLs and refuses any URL carrying credentials (userinfo).
- Capture bounds both the incoming and stored page URL to 8192 bytes. Mode
  and trail URLs also pass the stored bound after authorization and tracking
  removal; percent encoding that exceeds it is rejected as `url_too_long`
  before a new log append or intent is written. Existing v1 per-event intents
  are checked first under the capture lock: an interrupted older write whose
  normalized page, mode or trail URL exceeds the bound completes only through
  the existing exact hash/prefix and owned-log recovery checks, through single
  or batch delivery. A changed payload remains retryable for manual repair;
  it never becomes a new oversized append. Rejected candidates do not choose
  the day for accepted new entries in a mixed batch. Compact intents list
  only the entries actually appended to their log, so rejected or separately
  recovered events cannot make a completed batch appear unfinished after a
  home-folder change. A normalized URL of
  exactly 8192 bytes is accepted. Activity and Why marker payloads use the same
  lowercase, hyphenated UUID as their marker hash and immediate index update, so alternate
  valid UUID spellings cannot disappear or count twice after a rebuild. Batch
  acknowledgements still echo the request's spelling for correlation. This
  governs new writes; older noncanonical marker/intent recovery is a separate
  follow-up, and the log remains append-only.
- Deliveries are idempotent by event ID recorded in the Markdown log. The current host keeps one compact in-flight batch intent in its OS config directory to find the original daily log across date changes and the one-time layout move (§6.1); only a complete Markdown marker proves persistence. Shared locks cover the current policy check, intent, append, and file sync, so revocation and concurrent panel connections cannot race a write. An interrupted batch append can resume only from an exact byte prefix in the still-selected folder; changed or missing logs require review. The legacy `record_visit` upgrade path retains M1's per-event intent files until old extension queues have drained. For the presentation-sanitizer upgrade, an interrupted v1 entry can resume using the frozen older rendering only when its entire hash matches the durable intent and its partial bytes match exactly. This compatibility applies through single and batch delivery; it completes the original bytes rather than rewriting history, while new appends use the sanitizer. New single-event intents use format 2: they retain at most 512 KiB of prepared append bytes, their SHA-256 hash, and a separate SHA-256 identity over the normalized URL, canonical event ID, original timestamp/title and canonical marker metadata, independent of rendered prose. The complete JSON journal is capped at 1 MiB on publication and bounded reading. An interrupted v2 append resumes its original bytes only when this payload identity and the log tail both match; changing the selected home folder still refuses completion. Complete markers retain visit-ID deduplication, while summaries additionally require the same payload identity and selected folder even on a complete-marker retry. V1 hash-only intents remain readable with both frozen renderer generations (before the presentation sanitizer, and before inline math/code escaping); unknown historical renderings require manual repair. Downgrading to a host that cannot read v2 intents is unsupported. In M1, near-repeat visits within a configurable window (`near_repeat_secs`) were suppressed rather than rewriting a prior Markdown entry; M1.5a retires the window. Hourly totals already give each artifact at most one line per hour, and a `/log` entry is a deliberate act that should never be dropped, while a replayed delivery is absorbed by its event ID. Dwell time is now kept as hourly activity totals (Engagement and Coarse log, above). Scroll depth is not planned.

### 5.2 Summaries

> Superseded parts of the older M2 summary design, and when they changed: (history: [docs/history.md §5.2](docs/history.md#52-summaries--history))

**Summarize & log (as built, open access PR 3).**

1. **Trigger.** Every HTTP(S) page has a **Summarize & log** button in the panel, a `summarize` keyboard command (suggested Command+Shift+9 on macOS, Ctrl+Shift+9 elsewhere), and a "Summarize with Brauser" page and selection context-menu item. Brauser holds HTTP(S) access from install (§8), so no trigger needs `activeTab`. The command and menu item open the panel synchronously, before any await, then start the run in the worker; their result reaches the panel as a short-lived notice in `storage.session` and as a live message, so a failure sent before the panel loads is still shown. The button is enabled only while a harness is set up (`agent_status` ready), and the first summary from any of the three shows the setup warning (§7.1) once in the panel, without blocking. With Brauser's site access withdrawn in Chrome, the shortcut and menu say to re-enable it rather than that the page is not HTTP(S).
2. **Extraction.** The extension injects a pinned, bundled Readability (`@mozilla/readability` 0.6.0, its hash checked at build time and copied to `dist/vendor/readability.js` with its license) and then one self-contained extractor function, both through `chrome.scripting.executeScript` into every frame of the tab (`allFrames: true`), because document editors such as Word for the web render the document in a cross-origin iframe (`*.officeapps.live.com`) and leave the top frame without text (decision 2026-10-28, §14). A frame Chrome cannot inject, or whose result is missing or malformed, is skipped; a Readability load failure is not fatal, since the extractor falls back to body text. The extractor runs Readability on a clone of the document and walks the article into Markdown-like text: headings, list items, preformatted text, and link text without URLs; images, scripts, and styles are dropped. When Readability finds no article it falls back to the body text and marks the page not readable as an article. The summary title prefers a unique nonempty H1 in the current page's `main`, `article`, or `role="main"` region (ignoring headings under `hidden`, `aria-hidden="true"`, `nav`, or `aside`), then Readability's title, then `document.title`, and finally the trigger's tab title. Repeated equal headings count as one; distinct headings retain the metadata fallback. This keeps a list page's stale SPA metadata from naming an item summary after its body and URL have changed. Caps are in encoded bytes, as the native message measures them: the selection is cut until its JSON-encoded UTF-8 length is at most 64 KiB, and the content until content plus selection is at most 3 MiB, so a `start_run` stays under the host's 4 MiB inbound cap (§4.3) whatever the script or escaping. Lone UTF-16 surrogates in the text, selection, or title are replaced with U+FFFD before measuring, since the host's JSON parser rejects them, and every cut keeps surrogate pairs whole. Each frame is extracted and capped on its own. The content sent is the top frame's when its trimmed text is at least 200 characters or the longest (ties go to the top frame), and otherwise the longest subframe's. A top frame where Readability found no article (`readable` false, so its text is page chrome such as SharePoint's suite bar, footer, or noscript text) also yields to the longest subframe when that holds at least five times its trimmed text. Known limit: a non-article top frame beside a long consent, chat, or comment iframe can lose to that iframe, and an article-less page whose real text is in a subframe under five times the top frame's chrome summarizes the chrome. The all-frames extractor call is bounded at 10 s; if a slow or hung subframe holds it past that, the top frame alone is read (`frameIds: [0]`), as before 2026-10-28. The all-frames Readability injection has the same 10 s bound (`ALL_FRAMES_TIMEOUT_MS`): past it the extractor runs without waiting for Readability, and a frame without it falls back to body text. The selection is the chosen frame's, else the first other frame's that has one, which travels only if content plus selection still fit the 3 MiB cap. The title always comes from the top frame (then the tab title), never from a subframe. A page where no frame has text after trimming, or whose top frame result is missing (its injection failed), is "Brauser can't read this page's content." and starts no run. Word for the web may keep only the pages it has rendered in its DOM, so a long document can summarize from a leading portion only, and the editor frame's body text can include toolbar labels when Readability finds no article. The top frame's extractor also returns `location.href`; if it no longer names the run's URL (a navigation or a `pushState` since the trigger), the run is refused with "The page changed before it could be read." and nothing is sent.
3. **`start_run`.** The extension sends the tab URL, title, content, and selection, with no record key and a local `requested_at`; the host derives the record key itself (§6.2) and forwards the run to the service. The service refuses the run as `not_configured` without a harness, or when the knowledge base folder is missing or no longer matches its recorded identity (§6.1); a run that waited in the queue, or was resumed after a restart, is checked again when it starts and fails before its harness if the folder changed (a task is never resumed after a restart, and is also checked against its protected paths; §4.5). It saves the title, URL, and content as the run's `page.md`, with any selection in its own "Highlighted by the user" section, and queues the run (§12.2 step 10.2). A page with a queued or running run gets that run back.
4. **The run.** The service runs the chosen harness with the §7.1 prompt in the knowledge base folder, appending the agent's streamed text to the run's `stream.txt`.
5. **Answer validation.** The agent's final message must be the §7.1 JSON answer. The service parses and normalizes it (`host/src/answer.rs`); if it is invalid, the service sends one repair turn in the same session ("Your previous answer was not valid: … Reply with only the corrected JSON object"), and a second invalid answer fails the run as `invalid_output`, keeping the stream for Copy. The validated answer is stored as the run's `result.json`, and `RunView.answer` carries its type, category, importance, and reason.
6. **The run card.** The panel polls `get_run` about once a second while a card is visible and the run is not finished, and shows the state, elapsed time, the streamed text (as text only, `textContent`, never HTML), Cancel, and on success the "Type · Category · Importance — reason" line. A Codex run offers **Open in Codex** (`codex://threads/<id>`, opened by the service), and a Claude Code run offers **Copy resume command** (`cd '<knowledge base>' && claude --resume <session>`). Failures show the reason and Copy; a save failure also offers Retry save, which reuses the stored answer without rerunning the agent. Closing the panel does not cancel a run, and in-flight runs and up to five finished in the last day (less any dismissed) return from `list_runs` when the panel opens.
7. **Saving (as built, PR 4).** When the answer passes validation, the service stamps the run's `answered_at` (kept in `status.json`) and its `SummaryFinisher` saves under the configuration current at finish, not the one at `start_run`. Holding the config lock throughout, it re-reads config, checks the home folder's identity and the fixed layout, re-derives the record key from the run's URL (§6.2), and follows a key that changed mid-run (an adapter added or edited) rather than refusing it; `RunView.written_record_key` records the key it wrote. It writes the summary file (§6.6) through the ownership-checked replace, then appends the `summary` daily-log event (§6.2, §6.7, protocol/README.md) with the run ID as its event ID. The event time is the `answered_at` stamp in the offset of the click's `requested_at` (whole seconds; UTC when `requested_at` cannot be parsed), so the daily log is the user's local day and every save of one run builds the same file and the same entry: a retry after success leaves the file unchanged and the log line is written once. A summary is an explicit action, so it is logged while capture is paused and on a page no adapter covers. When the event is logged the save succeeds even if the artifact index cannot be opened or updated; the index catches up from the log on its next read. A failed save makes the run `failed` with `save_failed` and a plain reason that ends in what to do (choose the folder again, fix config in settings, move an unowned file at the summary name, check the computer's clock), with Retry save when a retry can work; the daily log's own refusal codes go to stderr, not the panel. Retry save reuses the kept answer; if its stamp is missing or the daily log's 30-day window would no longer accept it, the service stamps it again before saving, keeping the first stamp as `first_answered_at`: only the log event takes the new time, while the summary file's `summarized_at` and the ordering check below keep the first. A reason no retry can fix (the day's daily log is full, the run's details are not valid for it, the stored answer is not a valid summary) offers no Retry save: the finisher returns it as final, `status.json` keeps `save_final`, and `RunView.can_retry_save` is false. Whenever a save fails after the summary file was written, final or not (the daily log refused or could not take the event, or the index could not name its artifact), the run keeps its `summary_path` and written key, so the run card's **Open summary** still opens it. A save never puts an older answer over a newer one: when the record's summary file says it was summarized after this run's first stamp (a later run of the page saved first, then this one was retried, even one re-stamped for the log's window), the save fails, final, and leaves the file and the log alone. A summary event already in a daily log counts as this save's only when it is the entry this save builds in the current home folder; one logged under an older key, path, or folder (an append whose last sync failed, then a change before the retry) is the interrupted-entry or original-folder reason, never a success. When the event's artifact needs the index (a key recovered from a retired contract) and the index cannot answer, the save asks for Retry save instead of logging the summary under the web page. One limit: an append interrupted after its intent was journaled can be completed only as first built, in the home folder it was built for, so after a home-folder or adapter change the reason says the log entry cannot be completed (the summary file is still saved) instead of offering a retry that cannot work.
8. **Where summaries show (PR 4).** A summarized page becomes an artifact: a page no adapter recognizes gets a `web.page` artifact keyed by its URL record key, and a page an adapter recognizes attaches the summary to that adapter's artifact (spec §7.2). Its artifact file gains a Timeline summary line (§6.7), Recent lists the page on the day it was summarized with that day's latest summary, and the worklog says "Summarized: <title> — <type>, <importance> importance". A summary never counts as an open or an opened day. Inside a container (§5.1, §6.2), the summary stays with the artifact the page is recognized as: a pull request's summary attaches to the pull request, not its repository, and a summary is never rolled up, so the container's totals, days opened, first and last seen, Summary line, Timeline, and contents list are unchanged by it; a repository page the catch-all recognizes, such as a file view, is the repository's own record key, so its summary attaches to the repository. A contained artifact recorded only by its summary still names its container on the artifact card, as an unvisited one does. The panel shows the page's saved summary and the run card's saved summary (§5.7), and both open the file with `open_summary`.

Steps 5 and 6 of the older M2 design below still define the summary file's identity and ownership rules and the Markdown neutralizer; its steps 1–4 and 7 no longer apply. (history: [docs/history.md §5.2](docs/history.md#52-summaries--history))

5. The host writes the final summary itself, to a summary file that is a separate record from the page note (§6.1). The note holds only the user's own text; Brauser never writes a summary into it. The host writes a summary only from a harness run it started itself for that URL; no message lets the extension supply summary text. A summary may be written for any HTTP(S) URL that passes normalization and the denylist, whether or not its site is enabled for logging, and it never creates a page note or visit intent. (Since PR 4, 2026-10-27, as the open-access spec §7.2 decided: a summary also appends a `summary` daily-log event and creates or updates an artifact, a `web.page` artifact when no adapter recognizes the page and the adapter's own artifact when one does, so "never creates a log entry" no longer holds. This is an explicit, per-click record, not automatic capture, and it still creates no page note or visit intent.) Each page has one summary file, `summaries/<slug>.summary.md`, keyed by the same record key as its page note (§6.2), so a document's `/edit` and `/view` URLs share one summary; the fixed suffix means no profile filename template can make it equal a note name. Its host-written frontmatter records `brauser.kind: summary`, the canonical URL, and the record key (§6.6), and its first line links to the page and to the page note's computed path in the profile's link style. The note link resolves once the note exists. Summaries written before the one-time layout move (§6.1) keep links to the older folder names until the page is summarized again. Summarizing again replaces the whole file (§4.4). Summary files belong to Brauser, and user edits to them are replaced. A file without a matching summary identity is a conflict and is never overwritten. The ownership check reads only the leading host-written frontmatter, so agent text cannot change a file's identity.
6. Before writing, the host escapes the multi-line agent text that lands in the summary body (`description` and `assessment`, `markdown::neutralize_remote`) so that no CommonMark or GFM construct, nor an Obsidian embed, can make a viewer fetch a remote resource, and no code span, fenced block, indented code block or math can form and be handed to another engine. The escape is context-free, so it does not depend on how a viewer parses the text around it:
   - Every `<` becomes `&lt;`. An entity is only ever text in CommonMark and GFM, so no raw HTML can form anywhere: no tag such as `<img>`, `<iframe>`, `<video>`, `<audio>`, `<source>`, `<object>`, `<embed>`, `<link>`, `<style>`, `<script>`, `<svg>`, `<picture>` or `<image>` in any case, no comment, CDATA section, processing instruction or declaration, and no autolink.
   - Every `!` directly before `[` becomes `\!`, unless an odd run of backslashes already escapes it. An escaped `!` cannot start an image, so no inline, full, collapsed or shortcut reference image and no Obsidian `![[embed]]` can form.
   - Every backtick becomes `&#96;`, every `$` becomes `&#36;`, and every tab becomes `&#9;`; a form feed or vertical tab becomes `&#32;`, since some renderers treat them as blank (marked can then start a code block) and others decode their entities to U+FFFD. Every `~` in a run of three or more becomes `&#126;`, and every space from the fourth of a run on becomes `&#32;`. So no code span, fenced block (backtick or tilde, at any indent, in any container), indented code block, or inline or display math can form. Obsidian hands each of those to another engine: Mermaid fences (v11 node shapes load a remote image with `@{ img: "https://…" }`, with no `<` or `![`), `dataview` and `dataviewjs` fences, Dataview inline queries in code spans (`= …` runs a query and `$= …` runs JavaScript; by default Dataview also runs them in code blocks), math (MathJax), and plugin code blocks. Numeric entities are used, not backslashes, because an entity is context-free: an agent backslash written before a backslash escape would escape the backslash instead, leaving a live backtick, but cannot undo an entity. Outside code, every renderer decodes these entities to the plain characters, so the text looks the same; a backslash that escapes a backtick, `$` or `~` written as an entity is dropped, since the entity already shows the character. A backslash before whitespace is literal, but before a whitespace entity it would escape the `&` and show the entity's text, so a backslash that ends an odd run there is doubled and stays visible. Runs of one or two `~` are left alone, so strikethrough still renders; fences need three. Escaping every `~` was not chosen because it would lose strikethrough for no gain, and a line-start-only rule would miss fences opened inside list items and block quotes.
   - A `!` at the very end is escaped as `\!`, a trailing run of `~` or of spaces is written as entities whatever its length, and a trailing odd run of backslashes gets one more. So a neutralized fragment can be joined with another, or followed by host text, without re-forming `![`, a fence or indented code, or cancelling a following `\!`. Text written directly before a fragment must not end in an unescaped `!`, an odd run of backslashes, a `~`, a space or a tab; `neutralize_remote` and `markdown::inline` output never does. The summary writer therefore places neutralized text at the start of a line, after a host-written newline, never after a list marker or other prefix ending in a space: a review fuzz found that with a `- ` prefix marked could render a code block from output that held no tab and no run of four spaces.
   - Nothing else changes. Links, emphasis, strikethrough, lists and tables still render. Agent prose loses code formatting by design: code spans and fences show as plain text with their backticks or tildes, and a line indented four or more spaces or by a tab (a list nested that deep, or a continuation paragraph under a wide list marker) loses its structure and joins the text around it. An autolink shows as text. Those costs are accepted.

   Single-line agent text (type, category, importance reason, entity names, related notes) is backslash-escaped by `markdown::inline`, which also escapes Obsidian's `#` tags, `%%` comments, `==` highlights and `|`, and writes every backtick, `$` and `~` as the same numeric entities and each space from the fourth of a run on as `&#32;`, so no code span, math, fence or indented code block forms wherever the line is placed, at a line start or after a list marker included. In the obsidian profile, entities and related notes are wikilinks, which drop `<` and `>`, which no Obsidian file name can contain, and backticks and `$`, so a viewer without wikilinks (VS Code preview), which parses the inside as Markdown, forms no code span or math there either; entities are not used inside a wikilink, since Obsidian would not decode them in a link target. A summary's entity or related note that the wikilink would change, or that holds a backslash (which would escape the closing bracket), `%%`, `==` or `~~`, is written as escaped single-line text instead; related notes are written with `/` separators and no `.` components in both profiles. The summary's page link puts the host-normalized canonical URL in a bracketed destination; the page chooses that URL and WHATWG serialization leaves a backtick, `$`, `!`, `[` and `]` raw in the query, so the destination writes those, every `&` (or a query's `&amp;` would decode to `&` and link another query), and `%`, `=` or `~` next to the same character, as numeric entities, which CommonMark decodes, so the link target is unchanged; `npm run test:markdown-neutralizer` checks that markdown-it links the page link to exactly the canonical URL. A `~` stays, since a fence cannot open after `[[`. Daily-log page titles (legacy visits and hourly activity) and Why text also use `markdown::inline` (2026-10-28, `brwr-2za`), so new rows cannot form math or code from those fields. Existing log rows are append-only and remain unchanged. V1 hash-only journal recovery retains two frozen text renderers in `host/src/capture/legacy.rs`: the pre-presentation-sanitizer generation and the Unicode-17 presentation-sanitizer generation before inline math/code escaping. A whole-entry hash and exact torn prefix must match before either can complete an interrupted append through single or batch delivery; measurements, canonical URLs and encoded marker payloads remain byte-exact. V2 journals continue to replay their stored prepared bytes and renderer-independent payload identity (§5.1). Front matter values are JSON-quoted YAML scalars (`markdown::yaml_string`). Obsidian's properties view and VS Code's preview, which hides front matter by default, show them as text, not Markdown. `yaml_string` also writes `<` and an image `!` as JSON `\u` escapes: a YAML reader gets the same value, and a viewer without front matter support, which renders the block as body text, sees neither character. `npm run test:markdown-neutralizer` renders a shared corpus (`host/tests/fixtures/markdown-neutralize-corpus.json`) as neutralized body text and as front matter with markdown-it, micromark (GFM) and marked. It also renders each input as a single-line field (`markdown::inline`) and as a wikilink, and each output followed by other neutralized fragments (including ones starting with `~~`, spaces, or a newline and four spaces), and fails on any element or attribute outside an allowlist of inert text markup (a table cell's `style` only as `text-align:left|right|center`); neutralized body text, single-line fields and wikilinks must render with no `<code>` or `<pre>` at all; raw body text must hold no backtick, `$`, tab, run of three `~` or run of four spaces, a raw single-line field no backtick, `$`, `~` or run of four spaces, and a raw wikilink no backtick or `$`. The check also requires each renderer to decode the entities to their plain characters. Front matter rendered as body text keeps `<code>` and `<pre>` on its allowlist, since `yaml_string` does not escape backticks; Obsidian reads front matter as properties, not Markdown. (history: [docs/history.md §5.2](docs/history.md#52-summaries--history))

   The guarantee covers CommonMark and GFM inline and block constructs, raw HTML, Obsidian embeds, and, since 2026-10-27, every construct Obsidian hands to another engine through a code span, fence, indented code block or math. Obsidian's handling of `\![[embed]]` and VS Code preview are to be checked by hand (TESTING.md "Real-UI acceptance"); as of 2026-10-27 no manual check has been performed, and its date and result are recorded here once it is.
7. Replaced: saving follows as-built step 7 above. (history: [docs/history.md §5.2](docs/history.md#52-summaries--history))

### 5.3 Page notes

- A page note is a persistent scratch space attached to the page. The user can add to it at any time; when they return to the page, the panel shows it again.
- One markdown file per record key: per artifact on a recognized page, otherwise per normalized URL (§6.2). A Google Doc has one note whether it is open in `/edit` or `/view`. It is created on the first keystroke, not when a page is opened, so visiting a page never creates an empty note.
- Notes work on any HTTP(S) page, not only logged sites. Brauser holds HTTP(S) access from install (§8), so the panel reads the active tab's URL and title directly and the user can type a note at once: there is no first-note gesture, toolbar or keyboard workaround, or per-origin request (protocol v6, 2026-10-26). A note does not enable logging.
- The note's title comes from the page's title when the note is created (or its host name when the page has none); the panel asks only for the note text.
- The panel autosaves: it sends the whole note about one second after the user stops typing, and again when the active tab changes or the panel closes. There is no Save button. A small indicator shows "Saving…", "Saved", or an error.
- **Local recovery.** Closing the panel cannot
  await native saves, so every input synchronously publishes the current URL
  and text in the extension origin's browser-local storage before scheduling
  autosave. Each snapshot has an immutable random key; the new copy is written
  before the previous one is retired, and a native acknowledgement deletes only
  its own exact snapshot. Another panel's stale acknowledgement or discard
  cannot remove newer edits. On reopen, the current host note loads normally
  and recovered text appears separately. Each copy has its page address,
  reason, read-only text, and individual **Copy** and **Discard** controls
  (2026-10-28, `brwr-kws`). An empty edit is explicitly labelled as clearing
  the note; Copy preserves its exact empty text. Discard selects an immutable
  snapshot ID, including for memory-only conflict copies, and leaves the
  other displayed copies and live typing alone. Clipboard failures appear
  beside that copy; a failed local discard leaves it available for retry.
  Recovery never fills the editor or sends a save automatically,
  including when the note's revision or artifact identity changed. Conflicts,
  failed saves and empty deletions remain recoverable; conflict copies stay
  until explicit discard. Copies contain note text and URLs, stay local to
  this browser profile (outside the home folder), and are not synced by Brauser.
  Blocked storage, quota and cleanup errors show a warning without preventing
  native saves; when a new local write fails, older durable bytes and the
  newest in-memory copy-out remain available, but the newest edit cannot be
  promised after teardown. Browser data removal or extension uninstall also
  removes browser-local recovery.
- The side panel is the only supported editor. The files stay plain Markdown and can be read anywhere, but an edit made outside Brauser can be overwritten. The version check in §4.4 exists so two Brauser windows do not overwrite each other: when a save is refused, the panel loads the newer note and shows the text it could not save so the user can copy it back.
- A page note holds only the user's thoughts. Agent output never goes into a note; it goes to the page's separate summary file (§5.2) or wherever a command sends it (§5.6).
- Notes created by M1 load into the editor normally. M1 review drafts are left in place and are not merged.

### 5.4 Read later

- `/save` writes a read-later entry, one per record key (§6.2); saving an already-saved artifact from another of its URLs reports it as already saved. It may link to the page's summary file (§5.2); the summary itself is never copied into the entry.
- Items can be marked done, which moves them in the index but never deletes the file.

### 5.5 Related pages

- The host maintains a SQLite FTS5 index over titles, summaries, notes, and tags.
- When the active tab changes, the sidebar shows related pages ranked by text relevance, shared tags, and shared domain.
- v1 is keyword search. Embedding-based similarity is a later, optional stage.
- The index also holds `artifacts`, `visits`, and `edges` tables, built from the artifact log (§5.1) and arriving with M1.5a before FTS5. This lets the omnibar (§5.6) answer questions that browser history cannot, such as "the Confluence page about SBOM I read last week", "PRs I reviewed this sprint", and "everything connected to PROJ-123".
- The index lives in the OS cache directory, not in the user's home folder, and can be deleted and rebuilt at any time with `brauser reindex`.
- On each connect, the host does an incremental scan (by modification time) to pick up files edited outside Brauser. While connected, it watches the home folder for changes. There is no scheduled reindexing.

### 5.6 Omnibar

Input is either a slash command or free text.

| Command | Action |
|---|---|
| `/summarize` | Summarize the page into its summary file |
| `/note <text>` | Add text to the end of the page note |
| `/save` | Add the page to read later |
| `/related` | Show related pages |
| `/log` | Log the current page now as a `web.page` artifact, on any HTTP(S) page, even if no adapter recognizes it (§5.1) |
| free text | Built as the task bar (§5.7): sent to the agent with the page as context; the report shows in the panel; Brauser writes nothing |

**Custom commands** are defined in config:

```toml
[[commands]]
name = "actions"
description = "Extract action items from this page"
prompt = """
Extract action items from the page below. Return a markdown checklist.
"""
include = ["page.content", "selection"]
output = "sidebar"          # sidebar | new_note | clipboard
```

No command writes agent output into a page note (§5.3), and only `/summarize` writes a page's summary file (§5.2). `new_note` creates a new Brauser-owned file and never replaces an existing one. To keep agent text in a page note, the user copies it from the sidebar. Config loading rejects `output = "page_note"`.

Template variables: `{page.title}`, `{page.url}`, `{page.content}`, `{selection}`, `{input}`, `{date}`.

The free-text row is built as the task bar (2026-10-28, §4.5, §5.7). Slash commands and `[[commands]]` are still later; config does not read `[[commands]]` yet.

`/log` and notes are one-off actions the user takes on a single page and need no confirmation (§7). `/log` records the page as a `web.page` artifact: one `activity` event in the log, and an artifact file and index entry like any other artifact (§5.1). It does not enable automatic logging for the site.

### 5.7 Side panel layout

The panel is about the page in the active tab. Capture housekeeping collapses into a one-line strip at the bottom, and configuration stays on the settings page.

The enabled-capture status describes sites with adapters, including when none
are configured. An unreadable tab (including a Chrome page or withdrawn site
access) has no artifact verdict; a readable page the host does not recognize
still says "Not an artifact". The current card refreshes when the host becomes
reachable again and whenever any worker-status read observes a different
`delivered_batches` generation, including reads made by Pause, Discard and
Dismiss. An unchanged generation does not request another lookup. Refreshes
keep unsaved context for the same artifact and use the existing coalescing
refresher so only the current page's response can render.

**Capture exclusions.** A separate plain-language
line explains why automatic capture is off for the current readable page:
configuration repair, no home folder, pending site removal, a host or local
pause, unavailable/expired browser policy, a workspace/account outside scope,
or no matching page rule. It uses the worker's revision-cached compiler and
ordered URL evaluator against the current host snapshot, with local narrowing
state taking precedence. Settings handles repair, pause, scope and rule
changes; a site without rules links to its existing Capture Site review.
Recognized enabled pages need no exclusion line. Navigation and policy changes
recompute it; unreadable/private tabs clear it. Page/link patterns never enter
this decision, and the line neither enables capture nor changes the host's
artifact/history verdict.

**Incoming navigation (protocol v8).** The
artifact card shows up to five recorded incoming source/transition pairs,
newest local day first, with ties ordered by source ID then transition.
Each row names the known source artifact with an explicit HTTP(S) link
opening a new tab (`noopener noreferrer`), describes the browser transition
as navigation, and gives its last recorded day. Repeated observations of a
pair stay one row, and a source can appear under more than one transition.
The host derives this only from existing validated trail edges and recorded
artifacts. Back/forward (including historical paired sources), unknown legacy
transitions, missing source artifacts, self edges, mentions, co-focus and
containment never supply a row. An empty list says "No known incoming source
recorded", without claiming that no navigation happened. Titles render as
text, and following another page clears the previous page's list. No new
capture, permission or agent request is involved; protocol v8 adds the
required `ArtifactView.incoming_trails` response field.

```
┌────────────────────────────────────────────┐
│ Rauser Browser Assistant                ⚙  │  gear opens settings
├────────────────────────────────────────────┤
│ Designing Data-Intensive Apps — Ch. 5      │  current page; follows the
│ oreilly.com/library/view/…/ch05            │  active tab
│ ● Artifact logged   ☆ Save                 │  status chips; ☆ = /save
├────────────────────────────────────────────┤
│ ┌────────────────────────────────────────┐ │  task bar (§5.6 free text),
│ │ Tell the agent what to do with this… ⏎ │ │  under the page on both tabs;
│ └────────────────────────────────────────┘ │  Enter runs, Shift+Enter adds
│                          [Run]  [Tasks ↗]  │  a line; the page it will use
│ Uses this page: Designing Data-Intensive…  │  Tasks ↗ opens tasks.html
│                                             │  (§5.7, §5.8), reading "Tasks
│                                             │  · 1 running ↗" while queued
│                                             │  or running; no card renders
│                                             │  here (#task-run-area is a
│                                             │  notice line only)
├────────────────────────────────────────────┤
│ ARTIFACT  jira.issue PROJ-123   In review  │  only on a recognized page
│ Focused 2 h over 5 days · last Tue         │  (§5.1); Tier 1 fields
│ ← opened from PR #412 · 3 linked           │  when configured
│ Why: blocking the release             ✎    │  optional one-liner
├────────────────────────────────────────────┤
│ [ This page ]   Recent   Read later (3)    │
├────────────────────────────────────────────┤
│ [Summarize & log]                          │  §5.2; on any HTTP(S) page
│ ┌────────────────────────────────────────┐ │  run cards (#run-area), newest
│ │ Designing Data-Intensive Apps — Ch. 5  │ │  first; each polls its run
│ │ Running · 42s                          │ │  state · elapsed time
│ │ Leader-based replication sends every   │ │  streamed text, as text only
│ │ write through one node…                │ │
│ │ [Cancel]  [Open in Codex]              │ │  actions fit the run's state
│ └────────────────────────────────────────┘ │
│ MY NOTE                          Saved ✓   │  §5.3, autosaves
│ ┌────────────────────────────────────────┐ │
│ │ This is how our orders DB fails over.  │ │
│ └────────────────────────────────────────┘ │
│ RELATED                                    │  §5.5
│ • Kafka: The Definitive Guide — ch. 6      │
│ • Orders DB failover (your note)           │
├────────────────────────────────────────────┤
│ ● Capture on · 0 pending               ▸   │  expands to Send now,
└────────────────────────────────────────────┘  Discard…, Dismiss, Pause
```

The Recent tab (M1.5a, §5.1) lists artifacts by day, with a type filter, a title filter, and a Worklog view with Copy. Opening Recent before the native host connects shows "Waiting for the host…" for both lists and worklog, with Copy disabled; as soon as the host configuration arrives, the visible view loads automatically without a second tab click or duplicate requests; it does not wait for the worker's capture policy to install, and still loads if that installation fails. If no home folder is chosen, both areas instead point to setup in settings and Copy stays disabled. The Read later tab lists saved pages with a search box and a checkbox to mark each done; done items move to a collapsed "Done" group and their files are kept.

**Summarize & log (as built, open access PR 3, §5.2).** On the This page view, between the artifact card and My note, the **Summarize & log** button sits above the run area (`#run-area`). The button shows on every HTTP(S) tab and is enabled only while a summary harness is ready (`agent_status` ready); otherwise its tooltip reads "Set up a summary harness in settings first." While a click is being handled the run area shows "Reading this page…" for that tab, and a refusal replaces it with one warning line: "Brauser can't read this page's content.", "The page changed before it could be read. Summarize it again." (the tab's address moved before extraction), the service setup instructions, or "Could not start the summary: …". These lines belong to the tab they are about; following another tab removes them, and a reply that arrives after the switch is dropped. The first summary from any path (the button, the `summarize` shortcut, or the context menu) adds the one-time setup warning (§7.1) at the top of the run area with **Got it**; it does not hold the run back. The shortcut and the menu open the panel and report through a short-lived notice that the panel shows as the same line, or as the run's card.

Each run card shows the page title, the state with its elapsed time, the streamed text (text only), and on success the "Type · Category · Importance — reason" line; a refresh problem shows as a warning line in the card. Cards follow the page, newest first: only cards whose run is for the followed page's URL (ignoring any `#fragment`) show, and a page that is not a web page shows none. A card for another page stays loaded and keeps polling, hidden, and shows again when the panel follows its page (2026-10-28: cards used to stay while the panel followed other tabs, which left other pages' results beside this page's Summarize & log).

| Run state | Card | Actions |
|---|---|---|
| Queued | "Queued" | Cancel |
| Running | "Running · 42s", with streamed text | Cancel; Open in Codex (Codex, once it has a thread) or Copy resume command (Claude Code, once it has a session) |
| Summarized | "Summarized · 1m 05s" and the answer line | Open summary (when the run saved a file); Open in Codex or Copy resume command; Retry save when the save can be retried; Dismiss |
| Failed | "Failed: <reason>" | Copy (the stream, or the reason); Open in Codex or Copy resume command when available; Retry save when available; Dismiss |
| Canceled | "Canceled" | Dismiss; Open in Codex or Copy resume command when available |

**Saved summaries (as built, PR 4).** On the This page view, a **Summary** line sits between the artifact card and **Summarize & log** and shows the page's saved summary as "<type> · <category> · <importance> importance" with **Open summary**, whether or not an adapter recognizes the page; it comes from `get_artifact`'s `summary` (§6.2), is hidden for a page with none, and goes as soon as the panel follows another tab, so a line never shows for the wrong page. A recognized artifact whose history holds only summaries reads "No activity measured yet" instead of zero focus over zero days. A finished card whose run wrote a file adds **Open summary** (a failed save that wrote the file before the daily log refused or failed its event included, whether or not it offers Retry save), and a succeeded one, once one `get_artifact` call for the run's page answers, the same saved-summary line; a card that sees its run succeed also refreshes the page's own Summary line, since a summary event does not move the worker's delivery count; a failed call is retried once a poll interval later, and if that fails too only the line is left out. A summarized Recent row adds "Summarized · <type>, <importance> importance" and its own **Open summary**, named for its row ("Open summary: <title>") and placed beside the row's link, not inside it. The page and Recent **Open summary** buttons are disabled while the host is unreachable; the run card's is disabled only while one of the card's actions is running. Each sends `open_summary` with the page URL and the shown summary's own `summary_path` (the run's, for a card), so it opens the file that line, row, or run names even after the URL comes to resolve to another record (an adapter added or removed, or `strip_params` changed); the host opens only a file that is its own record's summary at that record's summary name. Each reports a failure as "Could not open the summary: …" where it was clicked; the agent's fields are set with `textContent` only.

When the panel opens with a summary harness set up, it restores every queued or running run and up to five finished in the last day that were not dismissed (`list_runs`). With no harness set up it does not call `list_runs`, so opening the panel never starts the on-demand service. If the service cannot be reached then, the run area shows the setup instructions with **Open settings**.

**Task bar (as built, 2026-10-28, §4.5, §5.6; task cards moved to the Tasks page, 2026-10-28, §5.7, §5.8).** A `#task-bar` section sits directly under the page area and above the This page/Recent tabs, so it shows on both tabs whenever the panel follows a tab. It holds a two-row text field (at most 4000 UTF-16 units), a compact **Run** button, **Tasks ↗** (reading "Tasks · N running ↗", N counting queued and running tasks, while any are active), a context line, a hint line, a visually hidden live line, and the task run area (`#task-run-area`), which now shows only a transient run line or a list-failure notice — no task card renders in the panel. Enter submits and Shift+Enter adds a newline; Enter during IME composition does nothing, and an instruction that is empty once cleaned sends nothing. Before sending, the extension makes the text well formed, turns control characters other than newline and tab into spaces and trims it; that cleaned text is what the agent gets and what Reuse shows. The context line reads "Uses this page: <title>", or "Uses this page and your selection: <title>" while the followed page has a selection (a one-boolean probe of the page, run only while an agent is ready, when the panel follows a page, when the field gets focus, on Run pointerdown and when the panel window gets focus); the selection is then sent with the page. The bar is shared by all tabs: a typed draft is kept when the panel follows another tab, the context line follows, and Enter takes the instruction and the then-current page together. The bar is only ever filled by the user's typing or Reuse, never from the page, the selection or agent text. While a start is in flight the button reads "Starting…" and the field is read-only. On success the bar clears and Tasks ↗ starts counting the new task; on failure the text stays and one warning line shows in the task run area for the starting tab only, mapped by error code: "Brauser could not read this page for a task.", "The page changed before it could be read. Run the task again.", "This page is too large to send with a task.", the service setup instructions, the host's `conflict` text, "Finish setup in Brauser settings to run tasks (home folder, knowledge base folder and agent)." or "Brauser's settings need repair before a task can run." (each with **Open settings**), "Rebuild the Brauser host to use the task bar." (an older host's "invalid or unsupported request JSON"), or "Could not start the task: …". When `start_task`'s answer is lost, the line instead reads "The task may have started — check Tasks before running it again.", the draft stays, and the panel lists tasks at once so a task the service queued anyway is picked up by Tasks ↗'s count. Lost means a port disconnect, the extension's timeout, `invalid_response`, the host's "The Brauser service stopped before answering…" (its `EXCHANGE_TIMEOUT` or a closed connection), or the host's "The Brauser service sent an unexpected reply."; definite refusals keep their text (decision 41). The summary and task run areas each own their own warning line. The one-time setup warning (§7.1) still shows before the first agent run of either kind, in the area that started it, under the `summarize_warning_seen_v2` flag.

When the bar cannot run a task, the field stays editable, Run and Enter do nothing, and the hint gives the first reason in this order: "Waiting for the native host…"; "Choose a home folder in settings to run tasks."; "Brauser's settings need repair before a task can run."; "Tasks are not supported on Windows yet."; "Set up an agent (Codex or Claude Code) in settings to run tasks."; "Re-enable site access in Chrome so the agent can read this page."; "Tasks work on http(s) pages.". **Open settings** shows when settings is the fix.

**Tasks page (as built, 2026-10-28, §5.8).** `tasks.html` opens in a normal browser tab. **Tasks ↗** focuses an already-open Tasks tab rather than opening a second one (found with `chrome.runtime.getContexts`, so the `tabs` permission is never needed) and otherwise opens a new one; the panel passes the newest task it started, so Run followed by Tasks ↗ lands there; focusing an already-open tab on a different task also writes `chrome.storage.session`'s `tasks_select_v1`, which that tab selects on (refreshing once first if the run is not yet in its own list), since a hash-only navigation does not reliably raise the page's own `hashchange` there. The page lists every task the service still retains (retention is unchanged, §4.5: every run younger than 30 days plus the newest 200 of any age, whichever is more), not just the panel's old five-newest restore set. On the left, queued and running tasks are pinned under "Running", then the rest are grouped by local calendar day ("Today", "Yesterday", then dated), newest first within each group; each row shows the instruction (clamped to two lines), a state pill ("Queued", "Running · 42s", "Done · 1m 12s", "Failed", "Canceled") and the start time, with a search box (matches the instruction and the page title) and a status filter (All, Running, Done, Failed, Canceled). History is paged: the page walks `list_runs` with `kind: "task"` one page at a time, cursored from each page's last *finished* run — an active run is never a safe cursor — and merges pages so a run that arrives both active (an earlier page) and finished (a later page) shows once, as finished (`mergeRuns` keeps the fresher view). A "Show dismissed" toggle (off by default) reveals tasks Dismiss has hidden; Dismiss still only hides a finished task from the list, never deletes the record, and still shares the panel's `dismissed_runs_v1` set, whose cap rises from 200 to 1000 (about 40 KB in `chrome.storage.local`) now that the page, not just the panel's five-card restore, reads it.

On the right, the selected task shows the full instruction as the headline (from `get_run`, never the 280-character `list_runs` cut), the subline "on <page title> · Codex|Claude Code · started <time> · <state> · <elapsed>", the action buttons, the **Report**, then a collapsible **Agent output** section holding the streamed text — all as text only. The output section is hidden until the agent has streamed something, starts expanded while the task runs and collapses once it finishes (a task restored already finished starts collapsed); once the user expands or collapses it, polling re-renders leave it as they set it. The streamed text uses the page's body type, not monospace (decision 43, which first folded a finished task's activity behind "Show agent activity" on the panel's task cards; the Tasks page detail view carries it, under its own "Agent output" wording). Actions match the panel's old task-card set: Cancel; **Copy report** once succeeded, or **Copy** (the error, or the stream) once failed, as before — Copy appears only on a failed task; **Reuse**, which puts the full instruction back in the panel's task bar and shows "Instruction sent to the task bar." (it goes to the panel in the Tasks tab's own window if one is open there, or else to the next panel opened anywhere, and only once; a Reuse that arrives while a start is already in flight waits in storage and fills the bar once that start settles, rather than being dropped); Open in Codex or Copy resume command when available; **Open page**, new to the page, which opens the run's own `url` (bounded to http(s) by the host, never a URL found in agent text); and Dismiss once finished. A task has no Open summary, answer line or Retry save. A task that failed or was canceled after it started still shows "Changes the agent already made stay; check before running it again."

The page does not call `list_runs` unless a harness is ready or a run may already exist (`runsMayExist`), so opening it never wakes the on-demand service needlessly; with neither, it shows "Set up an agent (Codex or Claude Code) in settings to run tasks." with **Open settings**. An older host that rejects the `kind` field shows "Rebuild the Brauser host to see task history." instead (the panel's summary restore says "Rebuild the Brauser host to restore agent runs."). The page does not poll while `document.hidden`, and resumes with one immediate refresh when shown again; shown again (or sent a select signal) after the setup text or a failed first list, it opens again rather than staying stuck. Each refresh re-reads the first page and gives every unfinished task the page left out one `get_run`, so it still shows its finish (one the service no longer holds leaves the list). A refresh that works clears only a list error, never a Reuse or **Open page** line. A Tasks tab that closes between Tasks ↗'s lookup and its focus gets a new tab instead. An unknown `#run=` hash (pruned, from another profile, or just malformed) selects the newest task instead of throwing; with no tasks at all it reads "No tasks yet. Start one from the side panel." A task view without `instruction` (a corrupt record's failed view, `TASK_REQUEST_MISSING`) passes the shape check and shows no instruction headline and no Reuse, rather than rejecting the whole list. The panel's `#task-announce` still announces a task's finish ("Task done: …", or failed, or canceled), since the panel still polls active tasks for the running count Tasks ↗ shows.

Each state changes one area rather than the layout:

| State | Panel |
|---|---|
| Not set up | The setup warning replaces the tabs; the page controls are disabled and the task bar's hint says "Choose a home folder in settings to run tasks." with **Open settings**; the strip reads "Off". |
| Capture paused | The setup warning stays hidden, since a paused setup is complete; the strip reads "○ Capture off" and the status line reads "Capture is paused. Resume it in settings." The host records nothing while paused, including "Why I'm here", so the artifact card's Save is disabled with a hint pointing to **Resume capture** (while a local pause still waits for the host, to **Pause capture**, which finishes it, only when that control is available; if the host is unreachable the hint explains that and asks to reopen the panel); a typed draft stays in the field. Settings shows **Resume capture** only while paused; from protocol v6 it saves capture back on with no Chrome grant request or native confirmation. |
| Native host unavailable | A banner under the header with the fix; everything below is disabled. |
| Origin not enabled | Notes, `/log`, and summaries still work. The site-capture area shows **Capture Site** with "Remember work on this site without configuring rules by hand." On a Jira Cloud, SharePoint/OneDrive or GitHub origin it opens the starter review; on any other HTTP(S) origin it drafts rules with the structural drafter and shows the plain-language review inline (§5.1), with no Chrome prompt or native dialog. Without a home folder, or while config needs repair, the button is disabled with a pointer to settings. **As built (2026-10-28, `brwr-f45`):** a structural review, including the recurring-pages hint's, shows **Improve with AI**. Without a ready harness it is disabled, with the same setup hint Summarize & log shows. Once clicked, the review shows the run's progress (state, the agent's streamed text as text only, and Cancel), then re-renders as "Improved with AI (<harness>), checked by Brauser" with **Use basic draft**, or keeps the basic draft and says in one line why the AI draft was not used. |
| Site access withdrawn in Chrome | One banner, "Brauser needs site access — re-enable in Chrome", with a button that opens Brauser's extension settings; shown only on HTTP(S) tabs. The note editor returns once access does. |
| Enabled origin, page not an artifact | No artifact card; the chip reads `○ Not an artifact`. When the current page is unrecognized and the worker's sample holds an unrecognized shape with a route word visited at least three times, the site-capture area says "Looks like you have recurring pages on `<host>`." and offers **Draft rules for these pages**, which drafts from the sample and adds adapters only. Otherwise the area appears only on a configured starter origin, saying its rules are managed in settings. |
| No agent set up | **Summarize & log** is disabled with the tooltip "Set up a summary harness in settings first."; the task bar stays visible, runs nothing, and its hint says "Set up an agent (Codex or Claude Code) in settings to run tasks." with **Open settings**; no run cards are restored, and the panel does not call `list_runs`. Later milestones add the chip `✦ AI not set up` and slash commands that need no agent. |
| Brauser service unavailable | The service setup instructions and the host's reason show as the line of the summary or task that failed (in its own run area), or, when cards are restored, as one notice with **Open settings** in the task run area, which shows on both tabs. Notes and capture are unaffected. |
| Not an HTTP(S) page | The page area names the tab and says Brauser can't use it; only Read later is active. |

Milestones fill the layout in: the page area, My note, and the capture strip first; the artifact card, the Recent tab, and the worklog view in M1.5a, and worklog files in M1.5b; Summarize & log with its run area and run cards in open access PR 3 (protocol v6); the summary file and log event in PR 4; the task bar, the Omnibar's free-text row, with its task cards on 2026-10-28; slash commands, custom commands and the answer card later; Related and Read later in M3; the full command list in M4.

## 6. Data Format

### 6.1 Folder layout

The user chooses one **home folder** at setup with the native folder picker (`storage.root`; the picker grant's purpose is still named `notes_folder` on the wire). It can be empty, or a subfolder of an existing knowledge base. Brauser creates fixed folders inside it as it needs them; there are no per-folder settings. The home holds only human-facing content:

```
<home folder>/                   chosen by the user, no default
  notes/<slug>.md                page note: the user's text only
  logs/2026/09/2026-09-27.md     daily artifact log: append-only source of truth
  artifacts/<type>/<site>/<slug>.md  one Brauser-owned file per artifact, regenerated from the log (M1.5a)
  summaries/<slug>.summary.md    one Brauser-owned summary per page (M2)
  later/<slug>.md                read-later items (§5.4; reserved, nothing writes them yet)
  worklog/2026-09-28.md          optional Brauser-owned daily or weekly worklog (M1.5b, planned, off by default)
```

The protocol's `StorageConfig` keeps its `log_dir`, `pages_dir`, `later_dir`, `artifacts_dir`, and `summaries_dir` fields, so protocol version 6 is unchanged: the host reports the fixed names and refuses an `update_config` or `set_up_harness` that names anything else. The panel's worklog is a copyable view, so no `worklog/` folder exists yet.

**Moving an older layout.** A config.toml from before the fixed layout names its own folders (typically `log_dir = "log"` and `pages_dir = "pages"`). The host moves it once, automatically: when a host process starts (every native-messaging connection starts one), when the panel or settings page opens (`hello`, `get_config`), and when settings saves the same home folder. Under the config lock and the capture lock, which every vault writer in every host process holds, it renames each older folder that exists to its fixed name inside the same home (`pages/` to `notes/`, `log/` to `logs/`, and a custom `artifacts_dir` or `summaries_dir` likewise), then writes the fixed names to config.toml, revision-checked. It never replaces or merges: the rename is no-clobber (`RENAME_NOREPLACE`/`RENAME_EXCL`), and if a fixed name already exists beside its older folder, nothing is renamed, config.toml is untouched, and the panel and settings show a `config_issue` naming the folder to move; the next open retries. Unusual layouts are refused with a repair message rather than guessed at: a nested older folder (`brauser/log`), a symlink or a file in place of a folder, or a name that differs from the fixed one only in case. Absolute, escaping, or overlapping locations were already invalid configurations. A crash after a rename and before the config write is recovered on the next attempt, because a renamed folder leaves no older folder to move. Before `log/` moves, the host records the rename in `visit-ids/log-relocations.json`, so visit and batch intents, which name their log by its path at write time, still find their logs; the artifact index notices the new `log_dir` and rescans. Until the move succeeds, the older layout is never used for vault I/O: capture, notes, and AI setup refuse with the issue, and buffered events wait in the extension. A folder the user moves out of the way is left alone, as is the old home when the user chooses a different one.

Artifact paths are built by the host from slugs of the adapter type, the site host, and the artifact ID; the captured ID is never used as a path directly. When slugging an ID would lose information, for example a case-sensitive Google Docs ID on a case-insensitive filesystem, the host appends a short hash of the full `artifact_id` so two artifacts never share a file.

An older config.toml is still validated as before while it waits to be moved: no content location may equal, contain, or sit inside another, compared case-insensitively, and an M1 config without `summaries_dir` or `artifacts_dir` loads with them absent and derived as before, then moves like any other older layout.

The **knowledge base folder** (`storage.knowledge_base_dir`, protocol v6) is a separate setting: the folder a summary agent reads to judge how a page fits what the user already knows. It defaults to the home folder (`null`), is chosen with the same native picker under its own purpose-bound token, and has its filesystem identity recorded like the notes root. Brauser never writes there. It may be any folder, inside or outside the home folder.

Brauser's own files live outside the home folder, in standard OS locations, so it never adds hidden folders to someone's knowledge base:

| File | Location |
|---|---|
| `config.toml` | OS config directory, e.g. `~/Library/Application Support/Brauser` on macOS |
| `visit-ids/` | OS config directory; M1's one synced intent file per persisted visit, replaced in M1.5a by one intent record per delivery batch and a compact index (§12.2 step 6.5); `log-relocations.json` records a one-time log-folder rename |
| `index.db` | OS cache directory |
| `index-state.json` | OS cache directory; disposable ingestion cache used to replace `index.db` atomically without reparsing every prior log after each delivery |
| `index-catch-up.json` | OS cache directory; disposable progress of a log catch-up that needs more than one pass's read bound (§6.7), never served and removed when the next complete cache is published |

If Brauser finds an existing folder with files it didn't create, it leaves them alone. It only indexes files under its configured content locations.

### 6.2 Internal model

The host works on two record types. A `PageNote` has `url`, `canonical_url`, a record key (`artifact_id` or `url_id`, below), `title`, `created`, `updated`, `tags`, `links`, and `body`. A `Summary` (as built in PR 4, §6.6) has `canonical_url`, a record key, `title`, `summarized_at`, the answer's `document_type`, `category`, `importance` with its reason, `entities` (technologies, products, people, companies), `description`, `assessment`, and `related_notes`, the `harness`, and a `thread` (Codex) or `claude_session` (Claude Code) to resume the run, plus the page link and `note_link`; it is Brauser-owned and replaced whole (§5.2). Each summary also appends a `summary` log event (§5.1) carrying the event ID (the run ID), the URL, title, `occurred_at`, type, category, importance, `summary_path`, `record_key`, `artifact_type`, `harness`, and the host-derived `artifact_id`; the index exposes an artifact's latest one as a `SummaryView` (`summarized_at`, `document_type`, `category`, `importance`, `summary_path`, `harness`; protocol/README.md). An `Artifact` has `artifact_id`, `type`, `site`, `title`, `url` (the last canonical URL seen), `fields` (Tier 1), `first_seen`, `last_seen`, `focused_secs`, `background_secs`, `opens` (cold opens, §5.1), `days_opened` (days with at least one open), `last_mode`, `why`, `aliases`, and `edges`. It is derived from log events (`activity`, `why`, `alias`, `unalias`, and since PR 4 `summary`) and regenerated, never edited in place. A summary is not measured activity: it never adds to `opens`, `days_opened`, or focused or background time, though it can move `first_seen` and `last_seen`. The index joins notes, summaries, read-later entries, and artifacts by record key.

`artifact_id` has the form `<system>:<host>/<native id>`, for example `jira:acme.atlassian.net/PROJ-123`, `confluence:acme.atlassian.net/123456`, `gdoc:docs.google.com/<id>`, or `github.pr:github.com/owner/repo/412`. The native ID is the tool's own identifier, with case normalized only where the tool treats it as case-insensitive (Jira keys are uppercased). Other systems that already know these artifacts can then join on the ID without Brauser fetching anything. The scheme is fixed by each adapter's `id` template and is part of the adapter's documented contract. Profiles only control serialization, including how `note_link` is rendered.

`web.page` artifacts use `artifact_id = web:<host>/<url_id>`, where `url_id` is the page's M1 URL hash (§6.5). They therefore join that URL's note, summary, and read-later entry by record key. `/log` on a page an enabled adapter recognizes records that adapter's artifact instead. The `web` scheme is reserved for these IDs: no adapter's `id` template may use it, and no adapter may use the type `web.page` (adapter-templates.md §8.1).

Because `artifact_id` is the record key below, an `id` template is durable. The adapter template `format` number versions the language, and a later format never changes how a template in an older format builds an ID. Changing the `id` template of an adapter that has recorded activity is a **migration**: the host shows how many artifacts are affected and, on confirmation, writes one `alias` event from each old ID to its new one, so no history, note, or summary is orphaned (adapter-templates.md §8.2). Aliases arrive in M1.5b, so until then the host refuses an `id` change on an adapter with recorded activity. Removing an adapter or changing its recognition rules does not erase this constraint: `config.toml` keeps a host-owned, extension-invisible history of every superseded recognition contract and ID. The same ID may be restored, and a genuinely disjoint route may introduce a new ID, but an overlapping route cannot return under a different type or ID until migrations exist. Only restoring the exact historical contract removes that redundant history entry; a disjoint adapter that happens to reuse the same ID does not.

The frozen-ID transition check compares counted template sets within each origin and type, so removing one of two same-type adapters narrows policy without mistaking the surviving sibling for an ID edit, while a real replacement remains refused.

**Retired history budget.** Growing
`[[adapter_id_history]]` is limited to 32 KiB of pretty-serialized TOML inside
the existing 64 KiB config cap, reserving room for active adapters and settings.
A legacy history already over budget may stay unchanged or shrink, so pausing
capture and other unrelated edits still save while the total file fits.
An adapter change prunes exact duplicate history entries and entries whose
exact contract is restored as an active adapter; no distinct contract is
discarded to make room. The normal ID freeze still applies during restoration:
restore the old contracts before removing newer adapters, rather than replacing
one ID with another in a single update. A save or Capture Site preflight that
grows history past its budget refuses with an actionable message: turn Capture
off without removing adapters, or restore exact old rules and IDs in settings
to reclaim history budget. At the total-file cap, a refusal reports history's
byte cost and asks to reduce optional adapter metadata or summary vocabulary;
restoring an adapter moves its contract out of history but does not remove it
from the total file. Both size refusals reach settings as `invalid_config`
before config replacement or layout I/O. History stays host-only, and the
adopted-contracts record and note ownership rules remain unchanged.

Before a starter Capture Site review enables Save or promises that saving enables
its pages, it sends the proposed adapter document and reviewed config revision
to the host's read-only `check_capture_site` (2026-10-28, `brwr-t8m`). Under
the config lock the host refreshes its state and uses the same transition checks
as a save, including retired and hand-removed contracts, without exporting that
history. A refusal names the recorded type and ID template and points to
settings; restoring that identity or using genuinely disjoint routes remains
possible. Edits invalidate the check, and replies from an older selection,
closed review or config revision cannot enable Save. This check carries no page
URL or title, grants no token, and writes no policy; `update_config` still
independently checks the current history and revision at commit. Unknown-site
drafts retain their existing `check_site_draft` flow.

A container's ID (§5.1) is a record key exactly like any other artifact's: a repository visited directly keys its own note, summary, and read-later entry the normal way, and an adapter's `within` is part of this recognition contract (adapter-templates.md §3, §8.3). Adding, changing, or removing `within` on an adapter with recorded activity is therefore a migration-guarded change like any other, checked against retired history by the same rule above, amended so a declared container may claim the routes a retired or removed contained adapter left behind (§5.1). Because the index derives container roll-ups from the current adapters, an established `within` is frozen like an ID: a save that changes or drops the `within` a live, retired, or hand-removed contract with the same origin, type, and ID already declared is refused, naming the declaration to keep, until migrations exist. Adding `within` to an adapter that never declared one stays allowed, so an existing setup can link its rules to a container. Removing an adapter exposes its routes to the unchanged adapters after it, so the same rule also checks every unchanged adapter against the contracts a save removes: a `github.pr` or `github.issue` rule that settings could not link to the repository (its ID and scope do not capture the owner and repo) can still sit ahead of the `github.repo` catch-all, but removing it would hand its pages to repository IDs, so that removal is refused until the rule declares the container or migrations exist; a rule that declares the catch-all as its `within` can be removed freely. Adapters sharing one origin, type, and ID build the same artifacts, so a save is refused unless they all declare the same `within`, or none. A `within` edited out of config.toml by hand cannot be refused when the file is read, but the adopted-contracts record still holds it, so the next settings save that changes adapters names it and is refused until it is restored. In `index.db`, a container's `artifacts` row holds its roll-up (its own visits plus everything within it), while the `visits` table holds one row per logged visit under the artifact that recognized it, so a container's `visits` rows are only its direct visits. Summing `artifacts.focused_secs` (or `background_secs`, `opens`) across a container and its contents therefore counts each contained visit twice; a reader taking totals must sum `visits` instead.

**Record keys.** Every per-page record (page note, summary file, and read-later entry) is keyed by `artifact_id` on a recognized page and by `url_id` otherwise. Without this, one document gets several notes and several summaries: Google Docs `/edit` and `/view` normalize to different URLs, and a Jira issue opened in a board modal and at `/browse/` does too. The rules:

- **The host derives the key; nothing else supplies it.** The extension sends only the URL. The host runs that URL through the confirmed Tier 0 adapter rules in its config, resolves aliases (§5.1) to the canonical ID, and uses the result. Tier 1 fields and page content never affect a key, so a page cannot choose which note or summary it gets. The adapter rules apply for keying whether or not the worker is logging that origin right now, so turning logging off does not split a note.
- **Both keys are recorded.** A record keyed by artifact records `artifact_id` and the `canonical_url` it was created on. A URL-keyed record records `canonical_url` and `url_id`, as in M1. Filenames carry a stable suffix derived from the key, so a title slug never decides identity. The ownership check matches the recorded key (§6.4).
- **Aliases follow the artifact.** A record stays at the filename it was created under. When the index resolves its `artifact_id` to a newer canonical ID, lookups for either ID find it. If both IDs already had a note before the alias was learned, the panel shows the canonical note and lists the other as "note from before the move"; nothing is merged automatically. An `unalias` separates them again. When a note is reached through an alias, the panel says so.
- **Older URL-keyed records stay findable.** Notes from M1, and notes made before an adapter existed, are left where they are. The host maps candidate URL-keyed records through the note contracts below, so the panel lists older notes for the same artifact with read-only **View**. When the artifact has no note yet, it also offers **Use as artifact note**: the host writes one under the artifact key with a no-clobber create, then removes the URL-keyed file after checking its identity. The migrated note retains the older note's recorded creation URL, even when migration is clicked from another route; later saves preserve that URL too. If the artifact note exists, including after an autosave creates it or a stale save discovers another writer's note, older notes offer only **View**. If an adapter is later removed, an artifact-keyed record is still listed on the URLs it recorded.

As built in M1.5a, note loads derive the key in the host, consult indexed artifact URLs, and perform a bounded scan of the flat notes directory so a pre-adapter note on an alternate route is still listed. Responses carry at most 24 older-note identities and trim that metadata when needed to keep the current note within one native message. `save_note` echoes the loaded key as `expected_key`; a changed adapter identity produces `key_changed` and copy-out text instead of writing under the new key.

When an external policy notification refreshes the panel to a different host config revision, it reloads the current page's note through the editor's serialized save/load queue before allowing the next edit. Pending or in-flight edits keep their original expected key and use the same conflict copy-out and durable recovery; the new note never receives them automatically. A refresh with the same host revision only retries a failed load.

**Note keys outlive capture.** When the capture policy does not
recognize a URL, its note key comes from the note-only contracts still
recorded: the superseded recognition contracts in `[[adapter_id_history]]`,
contracts removed from config.toml by hand (below), and, while an adapter
document is inert (an `adapter_issue`), that current document. They name record keys only and never authorize capture. Removing an
adapter or its site, narrowing its rules, or an inert document therefore keeps
one note per artifact on every route its contract matched, including routes
with no recorded activity and documents first noted afterwards. Exactly one
named artifact gives its key; several give the URL key. A recorded contract
this host cannot compile (an unsupported document format or a damaged history
entry) makes identity on its origin unavailable: loads and saves there refuse
rather than choose a key. An origin any current
or recorded contract covers is decided by those contracts alone: every
recorded artifact URL was matched by one of them, so a URL none of them
matches is not that artifact, even when its canonical form equals one an
artifact recorded after `strip_params` removed the identifying query value (a
Jira board and the issue modal opened on it). The older-note list, which also
authorizes viewing and **Use as artifact note**, keeps only candidate URLs
that map to the same artifact: by the current policy when it matches (a
current match naming another artifact is final), otherwise by these
contracts. A URL on an origin no contract covers never qualifies, not even
the artifact's own indexed routes or its note's creation URL.

**Policy removal.** When neither config.toml nor the host's adopted-contract state (below) records any contract for an origin (for example, a config.toml over the size limit was replaced on a fresh install, or an adapter was removed before either record existed), the host uses the URL key on that origin and does not recover an artifact key from recorded evidence. Every recorded URL (activity, mode URLs, explicit `why` context, an artifact note's creation URL) is a stripped canonical form, so without a contract the host cannot tell an artifact's page from another page with the same form: a Jira board and every issue modal on it share one once `selectedIssue` is stripped. Such a page therefore needs no history lookup or note scan, so a large home folder or an unavailable history cache cannot block notes on it. The artifact note itself is never changed or deleted; it stays reachable from any route a current or recorded contract still covers and in the home folder, and a client holding its key gets `key_changed` rather than an autosave. Removing an adapter or site through settings records its contract in config.toml, and adopting one records it in the host's state, so this applies only once both records are gone. A repair of a config.toml the host could not use (an unconfirmed folder, invalid settings, or a newer or malformed file that is still TOML) carries forward every `[[adapter_id_history]]` entry this host can validate on its own; the backup keeps the original bytes, and a file over the size limit is not parsed, so its history is not recovered. Neither cache deletion nor reindex changes these decisions. Optional older-note discovery on a recognized page remains best effort, so a broken optional note does not hide the primary note. (history: [docs/history.md §6.2](docs/history.md#62-internal-model--history))

**Explicit preserved-note viewing.** The panel's
**Preserved notes at this address** disclosure runs a separate read-only search
on demand. It lists up to 24 verified artifact notes whose stored creation
canonical URL exactly equals the current page normalized with today's
`strip_params`, including when several historical artifacts share that
address, no contract remains, or index.db is unavailable. The existing flat
notes scan bounds apply (4,096 files and 16 MiB); results are explicitly a
limited search, not a complete history. It verifies ownership at the key's
actual path and skips malformed, unowned, oversized, symlinked and other-kind
files. Each row names the title and artifact ID and offers only **View read
only**. The host rechecks ownership and the stored address on every open.

Stored-address equality authorizes only this explicit read, never identity
recovery or migration: the primary editor keeps its host-derived key,
including an occupied URL note, and capture remains unchanged. The viewer is
separate, text-only and copyable; collapse or navigation clears it and late
replies cannot restore it. This search does not consult the history index or
note contracts and cannot make an ordinary uncovered-page load or autosave
scan the notes folder. If today's stripping no longer produces the stored
creation address, this disclosure cannot find the note on that route; a route
that still normalizes to that address or the home folder remains available.

**Hand-removed contracts.** Only a removal
through settings records `[[adapter_id_history]]`, so the host also keeps its
own record of every adapter contract it has adopted, in the same recognition
form, in a host-owned state file (`adopted-contracts.json` beside config.toml
in the OS config directory, outside the home folder like the rest of the
host's state). Each
refresh under the config lock, which every note request makes before deriving
a key, adds the current revision's contracts the file lacks, so a contract is
recorded before it can key a note. A recorded contract config.toml no longer
holds, as a current adapter or a history entry, was removed or reshaped by
hand; note identity reads it like `[[adapter_id_history]]` (record keys only,
never capture), and so does the frozen-ID transition check (`brwr-hrr`): a
settings save cannot re-add an overlapping route under a different type or
ID, whether as a new adapter or by widening an existing one's match, while an
exact restoration or a disjoint route stays valid. Deleting `jira.board` in an
editor beside a `jira.issue` rule whose modal route reads the stripped
`selectedIssue` therefore keeps the board on its artifact note, and its next
autosave updates that note. Contracts are only added: re-adding one makes it
current again, and one a later settings save moves to history is read from
there, never twice. Several hosts share the file; each rereads it under the
lock, adds what it lacks, and replaces it atomically (temporary file, fsync,
rename), so none drops another's entries. It holds at most 1,024 contracts
and 1 MiB; a write past either bound, or any other write failure, is skipped
and retried on later refreshes, and until it succeeds a hand
removal of an unrecorded contract leaves its pages on URL keys as before.
Settings shows that as a non-blocking warning (`config_result.adopted_state.warnings`,
from the reporting host's own last attempt under the lock; `get_config`
refreshes under the lock so the attempt is current). A full record
(`adopted_state.full`) also offers **Forget recorded adapters** below, since
a fresh record has room; a write or lock failure does not. A
file the host cannot use (unreadable, not a regular file, over a bound,
malformed, or from a newer host) is never rewritten, and every URL the current
policy does not recognize has unavailable identity: loads and saves refuse,
because any origin's deciding contract may be in it. Settings names the file,
why it cannot be used, and what is paused (`adopted_state.issue`), and the
note refusal names its path and points at the repair. For the same
reason a settings save that adds or reshapes an adapter is refused while the
file is unusable, since the addition cannot be checked against what it held;
removals and other edits still save. A damaged entry with a
readable HTTP(S) origin makes only that origin unavailable, and refuses
additions only there, as a damaged
history entry does; one with no such origin counts as an unusable file.
The repair is settings' **Forget recorded adapters** (`brwr-hrr`), confirmed
in the page with no native dialog: `forget_adopted_contracts` carries the
`adopted_state.revision` the user was shown, and under the config lock the
host refuses a changed file as `conflict`, otherwise renames it aside to
`adopted-contracts.json.<UTC timestamp>.bak` (never replacing an earlier
backup) and records the current contracts afresh. Forgetting drops the
contracts the file held, like a config repair, so notes from adapters
removed by hand may need **Use as artifact note** again; fixing or moving
the file by hand does the same.

**Stripped parameters an adapter reads.** The
collisions above start when `strip_params` (§7.3) removes a query parameter
an adapter rule reads, so the host refuses that at the source. A rule reads
a parameter when it names it as a key of its `query` table (a capture,
literal, or presence condition) or in its `absent` list (adapter-templates.md
§5.3); path, title, alias, and ref templates never read the query. An entry
strips a parameter by the same rule URL normalization uses: ASCII
case-insensitively, by prefix for a trailing `*`, so `selected*` and
`SELECTEDISSUE` both strip `selectedIssue`. The host and the settings page
each implement that rule, and both are tested against the shared cases in
`protocol/strip-params/vectors.json`. Readers are the current adapter
document (valid or inert, read as written), `[[adapter_id_history]]`, and
the adopted contracts, since each of them still names note keys; an adapter
a save removes counts as well, because it moves to history. A settings save
(`update_config`, which Capture Site also uses) that adds a conflict, either a
new or changed entry stripping a parameter some reader reads or a new or
changed rule reading a parameter an entry strips, is refused as
`invalid_config` with a message naming the adapter, its origin, the
parameter, and the entry; the host checks before the update and again
under the config lock. While `adopted-contracts.json` is unreadable, a hand-removed reader cannot be checked, so any new or changed entry is refused (naming the file) until it is fixed or moved aside, as adapter additions are; removing or reordering entries still saves. A conflict is one reading rule (with its adapter's
origin, type, and `id` template) and the entry that strips its parameter, so
a save that keeps an existing one (reordering entries, pausing capture,
editing the adapter's other fields, or removing or restoring that adapter)
is not refused, while a new or changed rule reading the same parameter is.
A config.toml that already has one therefore keeps loading and saving, and no install is locked out.
Settings derives the warning from the `strip_params` and adapter document
`get_config` already returns (no protocol change), names each entry with the
parameters and adapters involved, and offers a **Stop stripping**
button for the entry, which removes only that entry through the normal
revision-checked save. A conflict whose only reader is a retired or adopted
contract is refused on add but is not shown, because those contracts never
reach the extension; a known gap, accepted to avoid a protocol change, so
such a config.toml keeps its colliding stripped URLs until the entry is
removed by hand.

**Names beside stored URLs.** New daily-log markers
carry a host-derived `stripped=<hex JSON>` field beside the existing marker
payload. Its `url` is the sorted, distinct list of decoded parameter names
actually removed, preserving their case; `modes` and `trails` hold positional
lists for each activity mode and optional trail source. This evidence retains
neither removed values nor original URLs. The canonical URL still preserves
untouched raw query segments. A known empty list means no query parameter was removed;
a missing field on an older record means unknown. The log header and event
payload shapes stay compatible, and old rows are never rewritten.

Index cache format 8 keeps that evidence beside each event and writes the
SQLite `url_provenance(event_id, artifact_id, role, url, stripped_params)` table
(`role` is `url`, `mode:<index>`, or `trail:<index>`; the last column is a JSON
array or SQL NULL for legacy unknown). Trail sources never become URLs for
the destination artifact's note discovery. V4–V7 caches and V5–V7 saved
catch-up passes retain their event checkpoints while scheduling the bounded gap rescan described in §5.1; records lacking
evidence stay unknown. Reindex reads both old and new markers; malformed or
misaligned evidence is skipped rather than guessed, and an unusable cache
is rebuilt. The existing cache read and publication bounds remain in force.

New page notes and saved summaries write `brauser.stripped_params` beside
their canonical URL. These exact decoded names use the shared YAML string
escaping, so a page-controlled name cannot expose HTML or image syntax in a
Markdown viewer that renders front matter as text. A page note's creation URL
and its evidence survive later saves from alternate routes and explicit
re-keying; a legacy note keeps its missing field instead of acquiring today's
evidence. Older-note
discovery, View and Use as artifact note refuse a candidate whose recorded
removed names intersect query or absent keys read by a current, retired or
adopted contract on that origin. Unrelated tracking names still allow
contract-verified discovery. Missing legacy evidence keeps the existing
contract-only checks; it never supplies proof of lossless normalization.
Explicit preserved-note viewing remains read-only and uses address equality
only to view, and artifact autosave keys still come exclusively from contracts.

New single-event intents use format 3: the stable payload hash also binds
names-only evidence. V2 hashes remain frozen and resume their exact prepared
bytes; V1 hashes can select the pre-provenance current renderer or either
frozen historical presentation renderer, only with a matching full hash and
exact torn tail. Batch journals already retain prepared bytes and keep their
format. A v3 retry with changed removed names refuses without touching the
log. Downgrading with a pending v3 intent is unsupported; older hosts cannot
recover it.

Historical activity and Why markers with uppercase or unhyphenated UUID
payload IDs (before `brwr-q7e`) are read using their canonical UUID for both
marker-hash validation and cache identity (`brwr-smu`, 2026-10-28). A different
UUID still fails the marker hash; authoritative logs are never rewritten.
For a pending v1 intent, the original queued UUID spelling is also tried in
the marker payload across all three pre-provenance presentation generations.
Only the stored whole-entry hash and exact existing tail can select those
bytes, through single delivery or activity batch delivery. The original queued
spelling must be retained; recovery does not guess arbitrary UUID spellings.
V2/V3 prepared-byte and payload-hash checks remain unchanged.

- **Keys are checked at write.** `load_note` returns the key it resolved, and `save_note` sends it back. If the host's current config maps the URL to a different key (an adapter or alias changed in between), the host refuses the save as `note_conflict` with reason `key_changed`, and the panel reloads and keeps the unsaved text in the copy-out box. A summary's key is recomputed under the config lock before writing; since PR 4 a changed key is followed, not refused, and the run records the key it wrote (§5.2 step 7).

### 6.3 Profiles

The **neutral** profile (default) uses YAML frontmatter, relative markdown links, and frontmatter tag lists. It renders correctly on GitHub, in VS Code, and in Obsidian.

The **obsidian** preset switches to `[[wikilinks]]` and Obsidian-compatible daily-note paths.

Custom profiles can extend either one.

M1 implements the neutral serializer only. The host rejects other profile selections until their distinct paths and link styles are implemented and verified.

```toml
[storage]
root = "/path/chosen/at/setup"       # required, no default
profile = "neutral"                  # M1; obsidian and custom profiles are planned
# The content folders are fixed (§6.1). The host writes these values and
# moves an older file's own names to them once.
log_dir = "logs"
pages_dir = "notes"
later_dir = "later"
artifacts_dir = "artifacts"
summaries_dir = "summaries"

[profiles.obsidian]
links = "wikilink"
tags = "frontmatter"
daily_log = "logs/{YYYY}/{MM}/{YYYY-MM-DD}.md"
filename = "{title-slug}-{url-id}"       # stable URL-derived suffix is mandatory

[profiles.custom]
extends = "neutral"
frontmatter = "toml"
```

**Reading is lenient, writing follows the profile.** The indexer parses both link styles and any frontmatter format. Switching profiles never rewrites existing files; `brauser migrate --profile <name>` converts them and defaults to a dry run.

### 6.4 Managed blocks

In page notes, Brauser owns only:

- Its own frontmatter keys, namespaced under `brauser:`
- Content between `<!-- brauser:start -->` and `<!-- brauser:end -->`

Page notes no longer use managed blocks: the side panel is their only editor, so the host writes the whole file (§4.4, §5.3), and summaries live in their own files (§5.2). Managed blocks remain the rule for any later feature that writes into a file the user also edits elsewhere. Such replacement is deferred until a cross-platform atomic displaced-byte backup and conflict procedure is proven with an external-editor race test.

Page-note and summary filenames include a stable identifier derived from the record key (§6.2), even when a profile uses a title slug. Before updating a file, the host verifies that the recorded key (`artifact_id`, or `canonical_url` and `url_id`) and Brauser ownership match the requested page. An absent or different identity is a conflict, never an opportunity to adopt or overwrite an unrelated file. Records carry `brauser.kind`: `summary` for summary files; a page note without a `kind` key, as all M1 notes are, is a `page`. The page-note path rejects a file whose kind is `summary` or `artifact`. The summary path accepts only a file with exactly one `kind: summary` line, and the artifact path accepts only a file with exactly one `kind: artifact` line.

### 6.5 Example page note (neutral profile)

```markdown
---
title: Designing Data-Intensive Applications — Chapter 5
url: https://example.com/ddia/ch5
tags: [replication, databases]
brauser:
  canonical_url: https://example.com/ddia/ch5
  url_id: 65fc5f8734006f2e6ac748dc5daadc458ac24dc3fc98c2233160c601c42389a7
  created: 2026-09-27T10:14:00Z
  updated: 2026-09-27T10:15:12Z
  visits: 3
---

Compare this with how our event pipeline handles failover.
Check whether the orders DB uses sync or async replicas.
```

A note on a recognized page (M1.5a) records `artifact_id` as its key instead of `url_id`; `canonical_url` is the URL it was created on:

```yaml
brauser:
  artifact_id: gdoc:docs.google.com/1AbC
  canonical_url: https://docs.google.com/document/d/1AbC/edit
```

### 6.6 Example summary file (neutral profile, as built in PR 4)

```markdown
---
brauser:
  kind: summary
  canonical_url: "https://example.com/ddia/ch5"
  url_id: "65fc5f8734006f2e6ac748dc5daadc458ac24dc3fc98c2233160c601c42389a7"
canonical_url: "https://example.com/ddia/ch5"
title: "Designing Data-Intensive Applications — Chapter 5"
summarized_at: "2026-10-26T14:05:00-07:00"
document_type: "book chapter"
category: "research paper"
importance: high
entities:
  technologies: ["PostgreSQL", "Kafka"]
  products: []
  people: ["Martin Kleppmann"]
  companies: []
harness: codex
thread: "codex://threads/0199a1b2-…"
---
[Page](<https://example.com/ddia/ch5>) · [Page note](../notes/example-com-ddia-ch5-65fc5f8734006f2e6ac748dc5daadc458ac24dc3fc98c2233160c601c42389a7.md)

**Type:** book chapter · **Category:** research paper · **Importance:** high — explains the failover our orders DB relies on

**Entities:** Technologies: PostgreSQL, Kafka · People: Martin Kleppmann

## Summary
Leader-based replication sends every write through one node…

## Assessment
Confirms the failover notes already in the knowledge base and adds the replication-lag cases they lack.

Related: notes/orders-db-failover.md
```

**Identity.** The `brauser:` block in the leading frontmatter is the only part the ownership check reads (`summary::owned_summary`): it must hold `kind: summary` and this record's key, `url_id` for a URL-keyed record or `artifact_id` for an artifact-keyed one, and not the other key; a block naming the other key too is not owned. The canonical URL is written to the block but not checked. Identity values are written exactly, as JSON-quoted scalars, so a YAML reader gets them back unchanged. The file is named after the page note's stem, `summaries/<note stem>.summary.md` (`artifact-<slug>-<hash>.summary.md` for an artifact key), so an artifact's summary keeps one name across its URLs.

**Layout.** The other front matter values are the run's: the title (the canonical URL for an untitled page), `summarized_at` in the click's offset (§5.2 step 7), the answer's fields, `harness` (`codex` or `claude`), and `thread: codex://threads/<id>` for a Codex run or `claude_session: <id>` for a Claude Code run, when the run reported one. Agent and page values there are JSON-quoted with control and invisible format characters turned into spaces and no Markdown entities, since Obsidian's properties and Dataview read YAML literally (§5.2 step 6). The body starts with the page link, the canonical URL in a bracketed destination, and the page note link; then the labels line with the importance reason, an **Entities** line listing only the groups that have names (omitted when none do), the description and assessment under their own headings, each starting a line after a host newline, and a **Related** line only when the answer named related notes, written with `/` separators. Single-line agent text is escaped with `markdown::inline`, and the description and assessment with `markdown::neutralize_remote` (§5.2 step 6). The daily log's `summary` line links to the file by a path relative to the log's own folder (`../../../summaries/...`), percent-encoded per segment; the event's `summary_path` field holds the home-relative path.

Under the neutral profile the note link is a relative Markdown link from `summaries/` to `notes/`, with path segments percent-encoded, and entities and related notes are escaped text; under the obsidian preset the note link is a folder-qualified wikilink, `[[notes/<slug>|Page note]]`, and entities and related notes are wikilinks (related notes without `.md`) unless the name needs escaping (§5.2 step 6). Obsidian resolves a bare `[[<slug>]]` to whichever same-named file it finds first, and the home folder may sit inside a larger vault holding a copy of the note; the `notes/` prefix matches only the home's note and still resolves when the home is a vault subfolder.

### 6.7 Example artifact file (neutral profile, M1.5b)

```markdown
---
title: Release blocker — SBOM export times out
url: https://acme.atlassian.net/browse/PROJ-123
brauser:
  kind: artifact
  artifact_id: jira:acme.atlassian.net/PROJ-123
  type: jira.issue
  fields:
    status: In review
  first_seen: 2026-09-24
  last_seen: 2026-09-29
  focused_min: 55
  background_min: 15
  opens: 4
  days_opened: 2
  last_mode: view
  aliases: [jira:acme.atlassian.net/OPS-45]
---

Why: blocking the release

Summary: [ticket, high importance](<../../../summaries/artifact-jira-acme-atlassian-net-proj-123-….summary.md>)

## Timeline

- 2026-09-29 16:20 · Summarized: Release blocker — SBOM export times out — ticket, high importance
- 2026-09-29 · focused 42 min · 15 min in background · opened 3× · from [PR #412](../../github.pr/github-com/owner-repo-412.md)
- 2026-09-25 · focused 12 min
- 2026-09-24 · focused 1 min · opened 1× · typed

## Linked

- mentions ← [PR #412 Fix SBOM export pagination](../../github.pr/github-com/owner-repo-412.md)
- opened → [SBOM export design](../../gdoc/docs-google-com/1abc-3f9a1c.md)
```

Artifact files belong to Brauser and are rebuildable in the same way as the index: `brauser reindex` regenerates them from the log, and user edits to them are replaced (§4.4).

Artifact files are written to be quiet in synced and versioned folders:

- **Lazy regeneration.** Delivery updates the index and marks changed artifacts dirty there; it does not touch their files. The host regenerates dirty files on a best-effort pass when the panel's connection closes, on the first connection of each local day, and when the user chooses "Update artifact files now" in settings. The dirty set is mirrored in `index.db` and its atomically replaced ingestion cache, so a host that exits before its pass loses nothing. The panel's artifact card reads the index, so a file that is not yet regenerated never makes the panel stale. The host treats a connection as the panel's once it sends a panel-only request (note, artifact card, context, Recent, or worklog); closing any other connection, such as the worker's background sync or the settings page, runs a pass only when none has completed yet that local day for the same home folder, with the day taken in the offset of the user's latest activity when the last pass ran. A pass publishes its cleared dirty set at least half a second apart (backing off to four times the cost of the last publication for large indexes) and before reporting a write error, so a host Chrome ends soon after its port closes keeps the files it already wrote and the next pass resumes after them. Host startup sweeps stale artifact and index-cache temporaries a killed pass left behind. (history: [docs/history.md §6.7](docs/history.md#67-example-artifact-file-neutral-profile-m15b--history))
- **Log ingestion recovery.** The format-2 disposable cache records each log's relative path, byte length, and modification time. Every delivery update, history query, note-identity lookup, and dirty-file regeneration first checks those checkpoints under the configuration/write lock and reparses changed files in full. This recovers an append that became durable before cache publication, including across restart, without replaying the event or asking for reindex. Unchanged logs are not reread; changed-file parsing retains the preceding prose needed by legacy markers and revisits an incomplete trailing marker on the next append. Cache-format upgrades rebuild from the logs, except that a format-4 or format-5 cache migrates in place, keeping its checkpoints: a format-4 cache drops cached activity the bounded log parser would now skip and marks M1-visit artifacts dirty so their files gain the open each visit now counts; a format-5 cache, which can hold no summary event (they had no writer before format 6), takes the new format number and marks every artifact dirty, since the same release changed how every artifact file escapes its front matter and Why line, so the next regeneration rewrites each file whose rendering changed and leaves the rest unchanged; its checkpoints are kept, so after a downgrade to a format-5 build (which skips a format-6 summary marker as malformed) and an upgrade back, `brauser reindex` recovers summaries in logs that have not changed since. Saved catch-up progress of format 5 migrates the same way and resumes; progress of any other older format is discarded and the pass restarts. SQLite is published before the checkpoint cache, so a failed publication leaves the checkpoint eligible for retry. Durable capture acknowledgement never depends on that publication. Ingestion is bounded to 100,000 files, eight directory levels, 32 MiB per file, 64 MiB of changed-file reads per pass, and the existing 64 MiB serialized state cap. A catch-up that needs more than one pass, such as a rebuild after the cache is deleted, a log is removed or shrinks, or a rescan of a history over 64 MiB, stops before the next changed log in sorted order, saves its progress in `index-catch-up.json` (never served), and reports unavailable history; the next pass resumes after the logs already read, so a rebuild split across passes ingests the logs in the same order as one pass; a Markdown file without the log header is checkpointed like a log so it is not reread. `brauser reindex` runs those bounded passes back to back in one invocation. Every rebuild, including the one after a removed or shrunk log, keeps the conflict winner the published cache chose for a reused event ID, at that body's own earliest log position, when that body is still in the logs, and falls back to the earliest log position only without a usable cache; that fallback holds even when a log sorting before ones already read arrives between passes. A logged event is indexed only within the field bounds capture enforces before writing (for activity, including the 2,048-byte title limit); an edited or crafted marker outside them is skipped like any other malformed line. `index.db` is written without overflow pages, so a row whose record payload exceeds SQLite's local table-leaf limit (page size − 35 bytes) is left out of that snapshot with a logged warning instead of failing publication; the log and the ingestion cache keep the event. A read, bound, conflicting-event, or publication failure returns unavailable history for artifact, Recent, and worklog queries instead of a success containing stale totals; no partial checkpoint is published. The planned real SQLite replacement (§14) still owns incremental transactions and scaling beyond these provisional cache limits.
- **Stable content.** Frontmatter uses dates, not timestamps, and whole minutes. Timeline and worklog prose show positive activity shorter than a minute as `<1 min`, omit zero durations, and keep whole minutes for longer durations; numeric frontmatter counters remain integers. A file whose regenerated bytes are unchanged is not rewritten (§4.4). An extra visit in the same day changes one Timeline line and a few counters, not the whole file.
- **Bounded size.** The Timeline keeps one line per day for the last 14 days with activity. Older days roll into one line per week, and weeks older than 26 weeks roll into one line per month. Linked lists the 50 strongest edges and ends with "N more" pointing at the omnibar. The full detail stays in the log and the index, so a ticket opened every day for a year still has a file of a few dozen lines. Titles, Tier 1 fields, and the one-liner are written with YAML escaping, and their Markdown is neutralized as in §5.2 step 6, so a hostile ticket title cannot make a viewer fetch a remote resource. Links follow the profile's link style.
- **The Summary line (PR 4).** An artifact with a summary shows its latest one under Why as `Summary: [<type>, <importance> importance](<link>)`, a relative link to the summary file with each segment percent-encoded; it is regenerated like the rest of the file, so the next pass after a new summary moves it.
- **Summaries in the Timeline.** A summary of the page, including one attached to an adapter artifact the page is recognized as (§7.2), adds a `- 2026-10-20 09:15 · Summarized: <title> — <type>, <importance> importance` line, with the time in the event's own offset and no seconds, worded like the worklog. Summary lines and day rollups are merged newest first by local date: a rollup sorts by its latest date, a summary by the date in its own offset (newest instant first within a date), and a summary sorts before the rollup of its own day, M1 visit days included, so after travel across offsets the lines follow local dates rather than instants. The 20 newest summaries (by instant) are listed and older ones become one `- N earlier summaries` line at the end. A summary is not measured activity, so it never changes focus, opens, or days opened; it can move first and last seen.

## 7. Configuration

Config lives in the OS config directory (§6.1) and is owned by the host. The extension reads and edits it only through host messages, which validate every change and require the revision returned by `get_config`. A change to `storage.root` requires a short-lived, single-use token from a native folder picker, and so does a new `storage.knowledge_base_dir`; each token names the setting it was chosen for, so one cannot authorize the other. `update_config` must reject an arbitrary path supplied by the extension. The picker token proves which folder Brauser may write or read; it is not a privacy choice. Every other change, including adding, editing, or removing adapters, turning capture on or off, and editing `strip_params`, is saved with the revision check alone (protocol v6, 2026-10-26). Canceling a picker leaves config unchanged. A malformed or newer on-disk config must produce a recoverable setup error rather than preventing the host from responding at all. (history: [docs/history.md §7](docs/history.md#7-configuration--history))

The config starts nearly empty. Setup writes only what the user chooses; everything else is either off or an inert, commented-out example. The host refuses to perform an action whose required config is missing and tells the extension what to configure, rather than guessing. Configuration is edited on the extension's settings page; the side panel links to it.

### 7.1 Agent harnesses

An agent is optional. Without one, logging, notes, read later, and related pages all still work; only AI commands are disabled.

> The M2 step 1 harness lockdown this section once described, and its removal: (history: [docs/history.md §7.1](docs/history.md#71-agent-harnesses--history))

**Setup: detect and probe, in the service.** Settings sends `discover_harnesses` and `set_up_harness`; the host forwards both to the service, so setup sees exactly the `PATH` and environment a run gets (launchd's plist environment plus `agent.env`, §14). Detection looks for `codex` and `claude` on the service's absolute `PATH` entries plus `$HOME/.local/bin`, Codex first, and reports each as found (with its version), not found, or found with a problem. The resolved binary and its directory, the `PATH` directory it was found in, and the directory of every symlink hop between them must be owned by the user or root and not group- or world-writable (group write is refused even for a user-private group, as on Ubuntu with umask 002; the error names the path and says to run `chmod go-w` on it), and `--version` must parse (`codex-cli N.N.N`, `N.N.N (Claude Code)`); any well-formed version is accepted. Every run resolves its program the same way, on its own environment, and applies the same owner and permission check before starting it, so a run starts only what setup could have found. `set_up_harness` names only a `HarnessId`, a timeout, and optionally `summaries_dir`. The service detects that harness again and probes it with no lock held: one fixed prompt ("Reply with the single word OK.", never page content) through the real runner in the knowledge base folder, with a 90-second cap, passing only when the answer contains "ok". It then takes the config lock, checks the revision, and commits `[summarizer]` (`harness`, `binary`, `version`, `timeout_secs`, `probed_at`; wire `ConfigSnapshot.agent`) with `summaries_dir` (the request's, else the existing one, else `summaries`). A failed probe changes nothing and names the harness and the reason. `timeout_secs` is 60–3600 and defaults to 600; settings shows it in minutes, and its hint says a task-bar task runs for at least 30 minutes whatever it is set to. Summaries and site drafts run under it; a task runs under the larger of it and 30 minutes (`TASK_MIN_TIMEOUT_SECS`, 1800), never more than 3600 (2026-10-28, from the first live task, a research task that took 7m36s of the 10-minute limit). `update_config` may only echo `agent` unchanged or clear it. A v5 file's `[agent]` and `[harness_record]` tables load as not set up and are dropped on the next save.

**Codex runs.** `codex app-server --listen stdio://`, speaking newline-delimited JSON-RPC 2.0, started in the knowledge base folder with the run environment. The service sends `initialize`, then `thread/start` with `cwd`, `approvalPolicy: "never"`, and `sandbox: "danger-full-access"`, then `thread/name/set` to "Brauser: <page title>" (for a task, "Brauser: Task: " plus the first 80 characters of the instruction, whitespace collapsed; at most 100 characters; a failure there is only a diagnostic), and records the thread ID. Each turn is a `turn/start` with the same `cwd` and approval policy, `sandboxPolicy: {type: "dangerFullAccess"}`, the prompt as text input, and the answer's JSON Schema as `outputSchema`; a Capture Site agent draft sends no `outputSchema`, because Codex applies it as strict structured output and open adapter tables cannot be described that way, so the draft relies on the service's validation and repair turn, and a task sends none either, since its answer is a plain-text report. `item/agentMessage/delta` text streams into the run, the turn's completed `agentMessage` item is its answer, and `turn/completed` ends it; a `failed` turn, or the app-server exiting first, fails the run. A repair turn reuses the thread. Codex uses the user's own `HOME` and `CODEX_HOME`, so its normal login and its user-level instructions and skills apply (§14).

**Claude Code runs.** One `claude` process per turn, in the knowledge base folder with the run environment: `claude -p --output-format stream-json --verbose --include-partial-messages --dangerously-skip-permissions`, plus `--session-id <uuid>` (chosen by Brauser) on the first turn and `--resume <id>` (the `system`/`init` event's session ID) on a repair turn. The prompt goes on stdin, which is then closed. Text deltas stream into the run; the assistant message is used only when no delta arrived. A turn succeeds only on exit 0 with a final `result` event with `is_error: false`, whose `result` is the answer; otherwise the run fails with the result text or the last stderr line. `--no-session-persistence` is never passed, so the session can be resumed from a terminal.

**The prompt.** The §6.4 prompt of [the open-access spec](docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md) (`host/src/service/prompt.rs`) names the run's `page.md` by absolute path and never carries page text. It says the file is untrusted data copied from the web that may contain instructions aimed at the agent, to be ignored; that the working directory is the knowledge base, whose related notes the agent should read to judge importance; and that the final message must be only one JSON object: `document_type`, `category`, `entities` (`technologies`, `products`, `people`, `companies`), `description`, `importance` (`level` high, medium, or low, and `reason`), `assessment`, and `related_notes`. Document types and categories come from the editable `summary_vocabulary` in config (by default report, announcement, ticket, specification, email, article; and technical specification, product plan, administrative, project plan, marketing material, research paper, news): the agent picks one and proposes a new value only if none fits.

**The answer.** The service trims the final text, strips one surrounding code fence, and requires one JSON object (unknown keys are ignored). Type and category are 1–64 characters with no control characters; each entity list keeps at most 32 distinct non-blank items of up to 128 characters; `description` is 1–2000 characters, the importance reason 1–300, and `assessment` 1–4000; the level is matched case-insensitively. `related_notes` keeps at most 32 entries and silently drops any that is absolute, contains `..`, is missing, is not a regular file, or resolves outside the knowledge base. Every error names its field. An invalid answer, including an empty one, gets one repair turn; a second invalid answer fails the run as `invalid_output` and keeps the stream, and a repair turn that fails in the harness fails the run as `harness_failed`.

**Environment and auth.** A run gets the service's environment overlaid with `agent.env`, read at the start of every run (§14). Codex needs nothing there; a Claude Code on Bedrock user puts `CLAUDE_CODE_USE_BEDROCK`, `AWS_PROFILE`, and `AWS_REGION` there. Settings lists the file's names, never its values (`get_agent_env`), and **Reveal agent.env in Finder** creates it at mode 0600 if missing and reveals it (`open_path`).

**Settings.** The "Summary harness" section shows the warning "Summaries and tasks run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Use them on pages you trust." (shown once more before the first agent run of either kind, a summary or a task, under the same seen flag), **Detect harnesses** with one choice per found harness and its version, the run time limit, **Test and use**, the harness state, **Remove harness**, the summaries folder, the knowledge base folder, the `agent.env` names, and the document type and category lists. When the service is not reachable, the state shows setup instructions keyed off `service_unavailable`.

Prompt injection in page content can distort a summary and, with full access, can make the agent read or change the user's files or run commands as the user. That is the accepted cost of full-access runs (§14): summaries and tasks run only on the page the user asks for, a task follows page steps only when the user's request asks for them (§4.5), settings warns about it, the answer is validated, agent text is shown only as text, and neither the panel nor a stored summary loads remote resources (§3.1, §5.2).

### 7.2 Artifact adapters

An adapter recognizes artifacts. It does not clean up URLs for a visit log. Its job is to say "this URL is Jira issue `PROJ-123` in `acme.atlassian.net`," not "this is a page on an allowed site" (§5.1).

Adapters are written in **adapter templates**, a small declarative language specified in [adapter-templates.md](adapter-templates.md) (format version 1). Path templates match a URL's path one segment at a time, query conditions match named parameters, title templates (one, or an ordered list for tools whose titles vary, such as an unread-count prefix) extract the title and fields from the tab title, and `id` templates build the canonical `artifact_id`. Captures use a fixed set of host-owned types (`int`, `hex`, `token`, `name`, `jira_key`, `uuid`, and literal alternations). Raw regular expressions are not supported.

```toml
[adapters]
format = 1

[[artifacts]]
type   = "jira.issue"
origin = "https://acme.atlassian.net"
match  = [
  { path = "/browse/{key:jira_key}" },
  { path = "/jira/{*}", query = { selectedIssue = "{key:jira_key}" } },
]
id     = "jira:{host}/{key}"
title  = "[{key:jira_key}] {title} - Jira"
refs   = [{ type = "jira.issue", find = "jira_key" }]   # other artifacts this one's title mentions
fields = { status = "<status selector>" }   # Tier 1 (M1.5b), optional; real selectors are recorded per starter adapter

[[artifacts]]
type     = "gdoc"
origin   = "https://docs.google.com"
unscoped = "acknowledged"   # shared origin; see Scope below
match    = [{ path = "/document/d/{id:token(20,64)}/{mode?:(edit|view|preview)}" }]
modes    = { edit = "edit", view = "view", preview = "view" }
id       = "gdoc:{host}/{id}"
title    = "{title} - Google Docs"
```

- **Evaluation.** Every operation runs in time linear in its input, with no backtracking, by construction (adapter-templates.md §4.3, §9, §10.2). The host (Rust) and the worker (TypeScript) each implement the same small grammar directly; neither compiles templates to regular expressions. Both prepare the URL identically (WHATWG parsing, 2048-character cap, per-segment percent-decoding, and refusal of `%2F`, `.`, `..`, and control characters inside a segment) and cap titles at 512 scalar values. Matching runs against the original URL after host authorization, so a path template is also the adapter's path rule. The first matching rule of an adapter wins, and adapters on one origin are tried in config order. Config validation reports the first error with its position and caps adapters at 64 and rules at 16 per adapter.
- **The worker's copy.** The same confirmed templates go to the worker in the policy lease (§3.2). A disagreement between the engines can only cost a visit the prefilter wrongly dropped, never add one, because the host re-checks. Shared test vectors, differential property tests between the two implementations, fuzzing, and bounds tests run in CI (adapter-templates.md §12).
- **Captures.** Named captures fill the `id` template. `{host}` is the authorized origin's host. No capture type admits `/`, `:`, `%`, whitespace, or control characters, so a capture never becomes a path (§6.1) and an `artifact_id` splits back into its parts unambiguously. The title template runs on the tab title, and when it does not match, the whole tab title is used.
- **Scope.** Some origins host many unrelated tenants: every GitHub organization, every Google account in a Chrome profile, every Notion workspace. The host owns a list of **shared origins** (`github.com`, `docs.google.com`, `www.notion.so`, `www.figma.com`, and `linear.app` in format version 1). An adapter on a shared origin must either restrict captures to listed values with `scope` (for example `scope = { owner = ["acme-corp"] }`), or declare `unscoped = "acknowledged"` where the URL cannot tell tenants apart, as with Google Docs. The native confirmation for an unscoped adapter says plainly that it records every matching artifact from every account in the Chrome profile, and recommends a separate Chrome profile for work. A URL outside the scope is not an artifact.
- **`path_case`.** Literal path segments compare case-sensitively by default; `path_case = "insensitive"` folds ASCII case for tools such as SharePoint. Capture types define their own case handling.
- **Aliases.** An adapter may declare `alias = { from = "tab_title", capture = "key" }` or `alias = { from = "meta", name = "<meta name>", value = "{id:int}" }`, its second source of identity (§5.1). The value must produce an ID through the same `id` template. Server-redirect aliases need no declaration.
- **Tier 1 `fields`** (M1.5b) are optional named CSS selectors, used as in §5.1, and travel to the worker in the lease with the match rules. The fixed extractor caps each value (for example at 256 characters) and the total. When a selector matches nothing, the field is empty and nothing fails.
- **Overlap.** Because the language has no free-form patterns, the host compares rules segment by segment and warns when two adapters on one origin could match the same URL, naming the rule that wins. The same check drops each Tier 2 draft rule that overlaps an existing adapter, and it feeds the match preview's re-map count.
- **Starter adapters.** Brauser ships adapters for common tools (Jira, Confluence, Google Docs/Sheets/Slides, GitHub pull requests, issues, and repositories, Notion, Figma, Linear, SharePoint) as disabled **starter adapters** with a placeholder origin such as `https://<your-site>.atlassian.net`. Before enabling one, the user resolves it to an exact scheme, host, and port and can edit it for self-hosted tools. Tool UIs change, so each starter adapter records the date its URL and title formats were last checked against the live tool. Each ships with a documentation page, `docs/adapters/<type>.md`, giving sample URLs and titles, what each rule and title entry covers, the frozen `id` template, scope guidance, and known limits ([adapter-templates.md](adapter-templates.md) §11.1). A starter adapter's `id` template is frozen once it ships (§6.2). A Tier 1 selector that stops matching degrades to Tier 0; it never fails the visit.
- **Starter rule updates.** Starter adapters improve after a user has saved one, and a saved adapter never changes on its own. Brauser keeps each starter's earlier shipped rule sets (`STARTER_HISTORY`, extension/starter-adapters.ts). When a configured adapter has the origin, type, and `id` template string of a current starter (a GitHub one built with the adapter's own organizations), holds every rule of one of those earlier versions, and lacks some current rule, its settings card offers **Update to current rules** beside Edit and preview. It opens the usual edit of that adapter with the earlier version's retired rules removed and the current rules appended, under a line naming each rule added and removed ("Replaces 1 rule with 1 rule from the current OneDrive for work documents starter:"), and previews the first added rule. Rules the user added, scope, `within`, title, and every other key stay; `modes` gains only the entries a new rule's `mode` capture lacks. An adapter matching no earlier version, because its user wrote it or narrowed it on purpose (say, removing Jira's board-modal rule), is never offered a re-widening. Nothing changes until the user saves, through the usual revision-checked save, and no offer is made when the result would pass the 16-rule cap or fail to compile (`starterRuleUpdate`). The frozen `id` template is untouched, so recorded history is unaffected; if the host still refuses the save as reusing a retired adapter's routes, settings says to edit it by hand.
- **Match preview.** Enabling or widening any adapter, not only a Tier 2 draft, shows the match preview from §5.1 against the origin's URL-shape sample and probe URLs. Until the sample exists (§12.2 step 7.6), and on an origin with no sample yet, the preview covers the probe URLs only, plus link shapes during Create adapter (§5.1). For a user-written adapter, the Tier 2 rejection rules only warn, because user-written config is trusted (§4.2).
- **Saving.** From protocol v6 (2026-10-26) saving an adapter turns its site on: settings shows the adapter's match rules, scope (or the unscoped statement), Tier 1 fields, and the match preview, and the save needs only the host's revision check. There is no Chrome grant request and no native confirmation, and removing an origin's last adapter turns the site off. Changing the `id` template of an adapter that has recorded activity is a migration, not an edit (§6.2). (history: [docs/history.md §7.2](docs/history.md#72-artifact-adapters--history))

**Adapter config.** `ConfigSnapshot` now carries `{ format, artifacts }`, serialized as `[adapters] format = 1` and `[[artifacts]]` in `config.toml`. The host validates the document through the Rust adapter-template engine, requires every adapter origin in the site allowlist, and returns bounded rule-pair overlap warnings in `get_config`; the settings page makes both warnings and adapter-policy errors visible. A missing adapter table in an older config becomes an empty format-1 document. An unsupported or malformed adapter format is kept in the snapshot and left unchanged on disk, but suspends automatic capture through a separate `adapter_issue`, without blocking page notes or AI settings. Unknown future keys inside `[adapters]` follow the same adapter-only failure boundary: the host keeps them outside the wire snapshot, preserves them across unrelated settings updates, and removes them only through the explicit whole-document repair; they never turn the complete config into a backup-and-rewrite repair. An open adapter edit is keyed by its origin, type, and ID rather than its array position; after another settings page publishes a revision, harmless reordering follows the same adapter, while a changed, removed, or ambiguous target disables Save until the user closes the stale editor and reviews the current version. While an adapter issue is present, a revisioned update may echo only that exact inert document so folder, AI-privacy, harness, pause, and Chrome-revocation narrowing remain usable; any adapter change must replace it with a fully valid document. Settings offers a single remove-invalid-rules action, and re-reads `adapter_issue` after every save before it can publish a worker lease. An unscoped shared-origin adapter adds the every-account-in-this-Chrome-profile statement and recommends a separate work profile. Until the M1.5b alias-event migration exists, the host conservatively refuses any direct `id` template change to an existing valid adapter. The extension accepts the host's preserved adapter document's wire shape even with format 0 or more than 64 artifact objects, so the host's `adapter_issue`, revision and repair controls remain visible. Non-object artifact entries are still invalid replies. Adapter validation keeps the supported format and 64-adapter limit for new policy; reading a repair reply does not enable it. (history: [docs/history.md §7.2](docs/history.md#72-artifact-adapters--history))

**Lease and recognition.** Every policy lease now contains the exact confirmed format-1 adapter document. The worker validates and compiles it with the TypeScript engine once when a lease is installed, or once when that revision is first restored after a worker start, then reuses the revision-keyed compiled document without serializing or recompiling the policy on each navigation. Every non-expired install message is still compiled even when its revision is already cached; an expired message is rejected before it can replace the active revision's compiled matcher. The worker requires a live Chrome grant and evaluates each original navigation URL before reading tab metadata or placing page-derived content in `storage.local`; rule misses and out-of-scope URLs disappear at that boundary. A cached pre-adapter lease is recognized as upgrade state and renewed once from the host; a missing or intentionally suspended lease remains inert. Delivery remains defense in depth: under the config lock, the host refreshes policy and independently evaluates the original URL and title with the Rust engine before opening the home folder. A stale or compromised worker's unrecognized or out-of-scope event receives terminal `suppressed` / `not_an_artifact`. (history: [docs/history.md §7.2](docs/history.md#72-artifact-adapters--history))

**Settings review.** The settings page lists each configured adapter's ordered rules, ID template, scope or every-account acknowledgment, title behavior, and Tier 1 fields, alongside the folder-sync caution. Overlap warnings use the adapters' type names and identify the winning rule instead of exposing protocol indices. A review editor can add, edit, remove, and reorder adapters without editing `config.toml`; it accepts one JSON adapter object, validates both that object and the resulting format-1 document with the worker's grammar, requires an already-enabled exact origin, and sends the result through the existing revision-bound native confirmation. For an example URL and optional tab title it shows the rule, canonical ID, title, and mode that would win. It also evaluates the fixed root, search, filter, login, dashboard, auth, sharing, and capability probes from §5.1 and highlights any match. These probe matches warn but do not block a user-written adapter, as required by §7.2. (history: [docs/history.md §7.2](docs/history.md#72-artifact-adapters--history))
- **M1 compatibility.** M1's `[[sites]]` entries (an exact origin and an optional path prefix) recorded nothing on their own (§5.1). Protocol v6 drops `sites` and path-prefix narrowing: a v5 config's site list loads, is ignored, and is removed on the next save, and capture origins are the adapters' own origins.

**Protocol v6 changes.** The as-built notes above describe v5's site allowlist, native confirmations, and Chrome-grant reconciliation; v6 removes all three. The host derives capture origins from the valid adapter document it has cached for the current revision (none while `adapter_issue` is set) and still re-checks each delivered event with the Rust engine. Settings lists the sites that have adapters, each with a Remove action that removes its adapters.

### 7.3 Privacy

```toml
strip_params = ["utm_*", "fbclid", "gclid"]
```

`strip_params` is the only privacy setting left. URL normalization drops fragments and the listed tracking parameters after the host has authorized the original URL (§5.1 M1 mechanics). An entry may not strip a query parameter an adapter rule reads; the host refuses a save that adds one, and settings offers to remove one already in config.toml (§6.2, "Stripped parameters an adapter reads"). Incognito windows are never logged: the manifest keeps `"incognito": "not_allowed"`, with no setting to change it.

Protocol v6 (2026-10-26, §14) removed the rest. `log_incognito` (which could only be `false`), the agent denylist, and `agent_denylist_confirmed` are gone from the wire and from new config writes. A v5 `config.toml` that still has them loads, ignores them, and drops them on its next save. The first harness setup no longer reviews a denylist, and no AI command waits for one to be confirmed.

## 8. Extension Permissions

| Permission | Why |
|---|---|
| `sidePanel` | Always-on sidebar |
| `nativeMessaging` | Talk to the host |
| `storage` | UI preferences, the capped delivery buffer, and, with artifact logging, the pending hourly activity totals, the URL-shape sample, the session tab-to-artifact map, and Create adapter's short-lived draft inputs in `storage.session` (§3.2, §5.1) |
| `webNavigation` | Required from protocol v6 (earlier builds requested it with the first adapter). Provides main-frame commits, SPA route changes, transition types, `onCreatedNavigationTarget` for trails, and `onTabReplaced` (§5.1). Its events carry every main-frame URL, so the worker filters them against the enabled adapters (§5.1 Trails); Chrome lists it as "Read your browsing history" |
| `scripting` | Required from protocol v6. Summarize & log (open access PR 3) uses it to inject the pinned, bundled Readability file and then the self-contained page extractor into every frame of the tab (`allFrames: true`, since 2026-10-28), only when the user asks for a summary (§5.2). The link-shape and Tier 1 functions stay top-frame only. M1.5b will also use it for the fixed Tier 1 field extractor and Create adapter's fixed link-shape function (§5.1). Adds no install warning of its own |
| `contextMenus` | Required. Registers one item, "Summarize with Brauser", on pages and selections; it opens the panel and starts a summary of that tab (§5.2). Create adapter's entry may join it later (§5.1) |
| `commands.summarize` | Not a permission but a manifest entry: the `summarize` keyboard command (suggested Command+Shift+9 on macOS, Ctrl+Shift+9 elsewhere, "Summarize this page with Brauser"), which opens the panel and starts a summary of the active tab (§5.2). No install warning |
| `alarms` | Required, added with background sync (§3.2); a one-shot alarm about 30 seconds after a capture and a 5-minute periodic alarm wake the worker to deliver buffered events and renew its policy lease. No install warning |
| `idle` | M1.5a; stops counting attention when the user has been idle for the detection interval (5 min by default) or the screen locks (§5.1). No install warning |
| `host_permissions` | `http://*/*` and `https://*/*`, granted at install (protocol v6). Chrome shows one install warning, "Read and change all your data on all websites", which is accepted (§14). This lets the panel read any HTTP(S) tab's URL and title and lets the worker observe navigation; automatic capture is still limited to adapter origins, and adapter paths remain host-enforced |

Since protocol v6 there is no `activeTab`, no `note` command (open access PR 3 adds only `summarize`), no `optional_host_permissions`, no runtime permission request, and no grant reconciliation. If the user withdraws Brauser's site access in Chrome, the panel shows one "Brauser needs site access — re-enable in Chrome" banner (§3.1) and nothing narrows the host config. `tabs` is still not requested. `minimum_chrome_version` stays at 116. (history: [docs/history.md §8](docs/history.md#8-extension-permissions--history))

M1.5a adds `idle` and uses `tabs.onActivated`, `tabs.onRemoved`, `tabs.onReplaced`, and `windows.onFocusChanged`, none of which needs `tabs`. Trails use `webNavigation.onCreatedNavigationTarget`, not `openerTabId` (§5.1). The worker-start map rebuild uses `tabs.query` for tab IDs and `webNavigation.getFrame` for main-frame URLs, and drops any URL the enabled adapters do not recognize (§5.1 Opens). M1.5a also sets the toolbar badge (`action.setBadgeText`, no permission) when the buffer nears its cap.

## 9. Repository Layout

```
/extension        TypeScript MV3 extension
/host             Rust native host and `brauser` CLI, one crate
/adapter-templates Rust crate for the adapter-template language (M1.5a; adapter-templates.md)
/protocol         Shared message schema (JSON Schema → generated TS and Rust types)
/scripts          Development scripts: dev native-host registration, the macOS smoke runner, and the log reader (§15)
/installers       macOS user installer; optional Homebrew formula
/fixtures         Sample vault and captured pages for tests
/docs             DESIGN.md, adapter-templates.md, SECURITY.md, CONTRIBUTING.md, config reference, one page per starter adapter (docs/adapters/)
```

The root `Cargo.toml` workspace members are `protocol`, `host`, and `adapter-templates` (the `macos-alert` crate was removed with protocol v6). The message schema is defined once and code-generated for both sides, so the extension and host cannot drift.

The layout above is the planned release layout. The repository currently contains `extension/`, `host/`, `protocol/`, `adapter-templates/`, `scripts/`, and CI, with DESIGN.md and adapter-templates.md at the root for now. The installers and fixtures arrive in later milestones, and `/docs` so far holds the starter-adapter pages and design specs.

## 10. Quality

- **Host:** unit tests, property tests on URL normalization and slug generation, and fuzzing (`cargo-fuzz`) on the message parser, frontmatter parser, and path handling.
- **Scripted host isolation:** debug `scripted-dialogs` lifecycle fixtures use an absolute `BRAUSER_TEST_CONFIG_PATH` under a temporary directory on every platform, with their cache beside it. Ordinary builds ignore the variable; a release build cannot include the feature. The macOS smoke wrapper retains its temporary-HOME defaults (TESTING.md).
- **Extension:** unit tests, plus Playwright end-to-end tests running the real extension against a real host and a fixture vault.
- **Round-trip tests:** every profile writes, reads back, and re-writes page notes and summary files with no diff, and user content outside managed blocks survives unchanged.
- **CI on every PR:** `clippy -D warnings`, `rustfmt`, `cargo-deny`, ESLint, TypeScript strict, dependency review, and the full test suite on macOS and Windows (`macos-15` and `windows-latest`, with `macos-15-intel` (x64) as a fallback only when GitHub cancels a starved `macos-15` job; the headless smoke run and the adapter-template fuzz-target compile check are macOS-only).

## 11. Distribution

- **Download experience:** provide one public Brauser download page with two clearly labeled steps: install the browser extension from the Chrome Web Store, then download and run the signed native-host installer for the user's OS. The page explains why both parts are needed, links to supported-browser instructions, and offers a short troubleshooting path. The native host is a companion installer; do not ask users to build from source, use a terminal, or install Homebrew to get started.
- **Extension:** publish the stable release through the Chrome Web Store. Keep the published extension ID stable because the native-host manifest authorizes that exact ID. Chrome Web Store installation is the supported path on macOS; do not instruct users to install a local CRX.
- **Supported browser:** Google Chrome Stable only, on macOS. Test the current and previous two stable major versions and publish the tested range on the download page. Edge, Brave, Arc, other Chromium browsers, Firefox, and Safari are unsupported in the first release. Chrome's Side Panel API requires Chrome 114 or later, and `sidePanel.open`, added by the side-panel rework (§8), requires 116; the release floor must also satisfy the current three-version support window.
- **macOS package:** offer a signed and notarized per-user `.pkg` for macOS 15 Sequoia and later, with a universal Intel and Apple silicon host binary and a Chrome user-level native-messaging manifest. Sign the app with the publisher's Developer ID Application identity and the installer package with its Developer ID Installer identity. Homebrew may be offered as an additional channel for technical users, not as the primary consumer install path.
- **Native-host registration:** register only Google Chrome, using its documented user-level manifest location on macOS. The installer must use the stable Chrome Web Store extension ID in `allowed_origins`; it must never use wildcard origins. `brauser register` may repair the Chrome registration after installation, with a clear confirmation and status report. Uninstallation removes only Brauser's host registration and installed binaries.
- **First-run flow:** after the extension is installed, opening the side panel checks for the host and gives a direct OS-specific download link if it is missing. After host installation, the panel verifies host/protocol compatibility, warns that setup is incomplete, and links to the settings page, which guides the user through choosing a home folder, profile, optional agent, optional sites, and privacy exclusions. Explain permissions at the point they are requested. Show a final review of what will be captured and where it will be written; capture stays off until the user finishes setup. Include a no-agent setup path that reaches a usable capture experience.
- **Upgrade and removal:** browser-store updates apply to the extension; host updates use the signed OS installer and preserve the configured home folder. Show a clear incompatibility error if extension and host protocol versions do not match. Uninstall leaves the user's notes and config untouched by default, and the documentation explains how to remove them manually.
- **Release page and support:** use the public GitHub repository's Releases page as the canonical download page. Each release links to the Chrome Web Store listing, the macOS `.pkg`, checksums, install/upgrade/rollback/uninstall instructions, troubleshooting, and the GitHub Issues support channel. Do not publish a release until the macOS installer and the extension have passed the install and uninstall checks.
- **Release artifacts:** publish signed installers, checksums, SLSA build provenance, and an SBOM for every release. Reproducible builds are a stretch goal.

## 12. Build Order

| Milestone | Scope |
|---|---|
| **M0 — Foundation** | Protocol schema, host skeleton, config loading, vault confinement, atomic writes, CI |
| **M1 — Capture** | Side panel and settings page, site logging, URL normalization, daily log, page notes (create-only with review drafts as first built; the side-panel rework before M2 made them editable and autosaving, §12.2 step 4) |
| **M1.5a — Artifacts** | The adapter-template engine in host and worker ([adapter-templates.md](adapter-templates.md)); Tier 0 adapters replacing per-site logging, with three starter adapters for tools the user opens daily (Jira, SharePoint/OneDrive, and GitHub); the worker prefilter from the policy lease; hourly activity totals, trails and refs, mode, and the one-liner; one intent per delivery batch and compact intents; record keys; `artifacts`/`visits`/`edges` index tables and lazily regenerated artifact files; the artifact card, the Recent tab, and the worklog view. No agent required |
| **M1.5b — Artifacts, continued** | Aliases for moved issues and renamed repositories, Tier 1 fields (which add `scripting`), worklog files, the remaining starter adapters, and Create adapter with the structural drafter, after M1.5a has been in daily use for a few weeks. Agent drafting after M2 host invocation (§12.2 step 8.2). No agent required |
| **M2 — Agent** | Harness setup, content extraction, streaming with cancellation, `/summarize`. (Superseded in part, 2026-10-26: the native confirmation for harness setup and denylist enforcement were removed by [the open-access spec](docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md); the agent service and summary flow follow that spec.) |
| **M3 — Related** | FTS5 index (added to `brauser reindex` from M1.5a step 6.6), related pages, read later |
| **M4 — Omnibar** | Built-in commands, free text, custom commands |
| **M5 — Release** | Installer, store listing, signing and provenance, docs |

M1 is a usable capture slice for development with no agent required. It is not a public download until the extension and signed installers complete M5.

M0's as-built record (2026-09-27): (history: [docs/history.md §12](docs/history.md#12-build-order--history))

The M1-era requirement that the host bind the notes root to a native folder-picker selection before exposing vault writes is now part of §4.4 and §7. (history: [docs/history.md §12](docs/history.md#12-build-order--history))

### 12.1 M1 implementation and acceptance plan

1. **Prove setup authority before capture.** The settings page opens a persistent native connection and asks the host to show an OS folder picker. On macOS this uses a directory-only `NSOpenPanel`. Chrome supplies `--parent-window=0` when a service worker starts the host. Parenting to a nonzero Chrome window handle requires separately audited code. Each synchronous picker or confirmation runs on the main thread of a short-lived child host process, which pauses the native connection until the dialog closes and then exits. A long-lived host that showed an AppKit modal and then blocked on stdin left macOS with a persistent busy cursor during the first smoke run. The child only reports the user's choice; the parent validates the result and mints every token. The host canonicalizes the result and returns a short-lived, single-use selection token bound to the folder's filesystem identity. A config update changing or repairing `storage.root` must present that token and the current config revision; a raw path alone is rejected. Cancellation and stale tokens leave config unchanged. The host gives the panel a repair path for malformed, newer, or identity-less config files without performing capture. Exercise the picker and thread model on macOS before M1 acceptance.
2. Consent and permission state: replaced by open access (protocol v6; §7, §8). (history: [docs/history.md §12.1](docs/history.md#121-m1-implementation-and-acceptance-plan--history))
3. **Deliver visits across service-worker and panel lifecycles.** Capture only main-frame, active-document navigation and SPA URL changes for granted sites. The service worker stores a count- and byte-capped queue with stable event IDs and original URLs in `chrome.storage.local`, restricted to trusted extension contexts. It never relies on an in-memory dwell timer. The service worker delivers pending records itself, starting a host for each background sync (§3.2; merged in PR #23, 2026-09-28); the host authorizes the original URL against its allowlist and path rule before normalization. It returns a terminal per-event outcome: `persisted`, `suppressed`, or permanent-policy `rejected` with a reason. Transient vault/host errors (including an unavailable folder or failed durability step) return a retryable outcome; the event stays queued with bounded backoff and a visible error. The extension removes only terminally acknowledged records and reports rejections and overflow in the panel. Complete Markdown event markers are the persistence proof; the OS config intent locates the original log on replay.
4. Recoverable Markdown: completed with M1 acceptance.
5. Verification of the no-agent path: completed with M1 acceptance. (history: [docs/history.md §12.1](docs/history.md#121-m1-implementation-and-acceptance-plan--history))

The plan follows Chrome's current [optional-permission rules](https://developer.chrome.com/docs/extensions/reference/api/permissions), [incognito behavior](https://developer.chrome.com/docs/extensions/reference/manifest/incognito), [service-worker lifecycle](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle), [storage access levels](https://developer.chrome.com/docs/extensions/reference/api/storage/StorageArea), and [native-messaging window handle](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) contract. Folder picker details follow [Apple's `NSOpenPanel`](https://developer.apple.com/documentation/appkit/nsopenpanel).

M1 validation status and the dated M1 smoke and acceptance records: (history: [docs/history.md §12.1](docs/history.md#121-m1-implementation-and-acceptance-plan--history))

### 12.2 Next steps

1. **M1 acceptance on macOS: done (2026-09-28).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
2. **Catch page-load regressions in CI (optional, recommended).** Unit tests import the pages but do not load the built extension. A headless Chrome step that loads `extension/dist/` and opens the panel and settings page would have caught the stale-manifest class of failure before a manual run.
3. **Settle M2 design before code: done (2026-09-28).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
4. **Rework the side panel and make notes editable (before M2): done (2026-09-28).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
5. **M2 step 1, harness setup: done (2026-09-28), merged in PR #19.** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
6. **Build M1.5a (artifacts)** next, after M2 step 1 and before the rest of M2 (step 8), in this order, independent of M2 except where noted. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   1. Smoke-runner probes first: `webNavigation.onCreatedNavigationTarget` for a middle-click, a `target=_blank` link, and `window.open`, including a source tab that navigates on before the new tab commits; transition types for a link, a typed URL, and a board-modal `?selectedIssue=` change (`onHistoryStateUpdated`); `chrome.storage.session` and a pending `storage.local` activity total surviving a forced worker stop; and `idle.setDetectionInterval` at 5 minutes; and that switching to a discarded tab and restoring a session commit with transition type `reload`, so neither counts as an open (§5.1).
      - Probe results (2026-09-28): (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   2. Adapter-template engine, built to [adapter-templates.md](adapter-templates.md) as a pure library with no protocol, config, or UI change, so it can be reviewed and merged on its own. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   3. **Adapter config, lease, and settings review: done (2026-10-16).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   4. **Activity events: done (2026-10-16).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   5. **Delivery and durability: done (2026-10-16).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   6. **Index tables and artifact files: done (2026-10-16).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   7. **Panel, worklog, and record keys: done (2026-10-17).** (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   8. **First three starter drafts: gallery built; live certification pending (2026-10-16); GitHub repositories and containers added (2026-10-27).** Settings recognizes enabled Jira Cloud, SharePoint/OneDrive, and GitHub origins and resolves the format-1 recorded fixtures to the exact origin. The GitHub gallery offers repositories alongside pull requests and issues, sharing one organization field that it pre-fills with the union of any GitHub adapters already configured (never rewriting their own scope) until the user edits it; the field is still required, and the starter path still never offers `unscoped`. Choosing repositories for an origin that already has PR or issue rules also offers to add `within` to them in the same save, so their existing history starts rolling up without a separate edit, without tripping the overlapping-route guard (§5.1, §6.2, §7.2). Each choice opens as editable JSON in the existing complete-document validation, match-preview, safety-probe, and native-confirmation flow, and an already-configured type/origin is not offered twice. The bundled documentation records example routes, ID contracts, scope behavior, and known limits. These are deliberately labeled development drafts rather than release-certified starter adapters: tonight's isolated build could validate the recorded fixtures in both engines but could not perform the required live-tool UI checks. Complete those checks and record their date in `docs/adapters/` before changing that label or treating step 6.8 as done.
7. **Build M1.5b (artifacts, continued)** once M1.5a has been in daily use for a few weeks. **Resequenced 2026-10-16 (§14):** Create adapter (step 7.6) ships as **Capture Site** right after M1.5a step 6.7, together with the alias/re-key migration from step 7.2 that a re-learned site needs. Agent drafting (step 7.7) follows M2 host invocation (step 8.2) and comes before the rest of M2. The rest of M1.5b keeps the daily-use gate. Moved issues and renamed repositories are rare, so aliases wait until there is a real log to judge them against. M1.5b can run alongside the rest of M2 (step 8). When GitHub repository containers (#126) took version 7, those messages moved onto version 7 with it (2026-10-27), so one version describes the whole Capture Site wire surface. Additive host-only messages, which no older peer sends and which an older host refuses as `invalid_request`, may stay within the current version; a change to an existing message's shape takes the next one. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   1. Alias probes: that `onBeforeNavigate` plus a committed `server_redirect` qualifier pairs the requested and final URLs for a moved Jira issue and a renamed GitHub repository. If Jira's move redirect turns out to be client-side, only the title signal is used for it.
   2. Aliases: `alias` and `unalias` events, append-only and idempotent by event ID, and notes reached through an alias listed on the artifact. Tests cover a chain (A→B→C), a move back, an `unalias`, a stub left at the old path, a ref to an old key, a board-modal switch between two issues that must not create an alias, a compromised worker's alias between IDs no adapter produces, and notes on both sides of an alias.
   3. Tier 1 fields in the worker while the page is live. This step adds `scripting` to the manifest (§8); M2 content extraction (step 8.4) reuses it. The policy lease carries the confirmed Tier 1 selectors, and the host drops any undeclared Tier 1 field a compromised worker delivers. The smoke run checks that fields are captured with the panel closed, including a late-rendering SPA field that needs the retry.
   4. Worklog files: the optional Brauser-owned daily or weekly files (§5.1), off by default, behind the sync caution.
   5. The remaining starter adapters for the common tools, each checked against the live tool on the date it records and each with its documentation page (spec §11.1).
   6. **Built (2026-10-27, `brwr-t40` on `feat/capture-site-unknown`; plan `docs/superpowers/plans/2026-10-27-capture-site-unknown-sites.md`).** Starter live-tool certification stays human work, and agent drafting is step 7.7. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))

      The original step and its test plan: (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   7. **Built (2026-10-28, `brwr-f45`, §5.1).** The hint drafts structurally and offers the same **Improve with AI** button. Named tests: the crate's `adapter-templates/tests/agent_draft.rs` (each `agent_planted_*` case mirroring a step 7.6 `planted_*` case, `agent_planted_blocked_route_word_is_dropped`, `agent_draft_overlapping_existing_adapter_loses_only_overlapping_rules`, `agent_can_add_a_tier0_rule_the_structural_pass_missed_and_it_passes_the_same_checks`, `agent_draft_with_tier1_fields_is_rejected_before_the_preview`, `agent_draft_with_a_title_template_is_rejected_before_the_preview`, `agent_draft_with_a_scope_is_rejected_before_the_preview`, `unparseable_agent_draft_is_discarded_without_being_shown`), each driven by an answer in `host/tests/fixtures/site-draft-answers/`; the same cases end to end through the host (`host_agent_*`, `host_unparseable_agent_draft_is_discarded_without_being_shown`, `agent_preview_lists_every_non_artifact_shape_a_borderline_draft_matches` in `host/src/capture_site.rs`); the service's `invalid_site_draft_answer_gets_one_repair_turn_then_fails_as_invalid_output`, `site_draft_run_cwd_is_its_run_directory`, `site_draft_run_writes_nothing_outside_its_run_directory` and `site_draft_runs_never_reach_the_summary_finisher`; the host's `start_run_from_the_extension_refuses_a_kind` and `start_site_draft_builds_the_input_from_the_hosts_own_structural_draft`; the panel's `the review renders only the host's site_drafted, never the run's raw answer`, `unseen AI rules start unchecked with their label` and `restored summary cards skip site-draft runs`; and the headless smoke step "Capture Site improves a draft with AI". Draft tests give the fake harness the step 7.6 planted inputs and check that its drafts are held to the same per-rule checks. They also cover a draft carrying Tier 1 fields, a title template, or a `scope`, and a draft that does not parse; the host must reject each before the preview, and the preview must list every non-artifact shape a borderline draft matches. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
8. **Build the rest of M2** after M1.5a, in this order, each step reviewed and merged separately. The sub-steps keep M2's step numbers, so step 8.2 is M2 step 2.
   2. Host invocation: replaced by step 10 (open access) before it was built. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   3. Streaming protocol: replaced by step 10; the panel polls `get_run`.
   4. Content extraction: built in step 10.3 (open access PR 3).
   5. `/summarize`: built as Summarize & log in steps 10.3 and 10.4.
9. **M3–M5** follow as in the table. The compact intent index moved into M1.5a (step 6.5).
10. **Open access and the Brauser service (2026-10-26)**, built before the rest of step 8, per [the open-access spec](docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md) and its plan (`docs/superpowers/plans/2026-10-26-open-access-and-agent-service.md`), in four PRs. It replaces the harness parts of steps 8.2–8.5: the allowlisted environment, argv templates, host-owned work directory, and §7.1 canary of step 8.2; all of step 8.3's host-side streaming protocol (chunk, `rejected`, `cancel`, and `summary_result` messages, and the concurrent native message loop it needed), since the panel polls `get_run` instead; the `activeTab` checks of step 8.4; and the denylist checks of step 8.5. Record keys (step 6.7) and the summary file's identity and ownership rules (§5.2, §4.4) carry over unchanged.
   1. **PR 1, open access and protocol v6: built on `feat/open-access-pr1`.** See §14.
   2. **PR 2, the service skeleton: built on `feat/agent-service-pr2`.** **[human]** checks still to run: register the dev build and close Chrome; run `start_run` from the panel console; confirm `launchctl print gui/$UID/com.rauser.brauser.service` shows the service and that it exits after 10 idle minutes; confirm `agent.env` values reach a run; confirm a rebuild plus a request restarts the service. The `agent.env` check needs a run that actually starts, which PR 3's real runners now provide. (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   3. **PR 3, real runners, extraction, and the run card: merged (#98).** **[human]** checks still to run: a real Codex and a real Claude Code summary from the dev build, and **Open in Codex** opening the thread in Codex Desktop (TESTING.md "Real-UI acceptance"). (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
   4. **PR 4, the summary file, the `summary` log event, and index integration: built on `feat/summary-outputs-pr4`.** **[human]** checks still to run: the neutralized constructs in Obsidian and VS Code preview (TESTING.md "Real-UI acceptance"). (history: [docs/history.md §12.2](docs/history.md#122-next-steps--history))
11. **The task bar (2026-10-28): built on `feat/task-bar`** (plan `docs/superpowers/plans/2026-10-28-task-bar.md`; §4.5, §5.6, §5.7). The free-text row of the Omnibar runs a task through the service, with its own run cards. **[human]** checks still to run: a real Codex and a real Claude Code task, Cancel mid-run, the hostile-page and spoofed-heading canaries, and a look at the logs (TESTING.md "Real-UI acceptance"). Next: completion notifications while the panel is closed (the `notifications` permission and a worker alarm), then a keyboard command and context-menu gesture for the bar, instruction recall, and slash commands with `[[commands]]`.

## 13. Platforms and License

- **Browsers:** Google Chrome Stable only; see the release support window in §11. Edge and all other browsers are out of scope for the first release.
- **Operating systems:** macOS 15 Sequoia and later on Intel or Apple silicon, and Windows (reinstated 2026-10-16, §14; supported Windows versions are set when Windows CI and the Windows installer are planned). Linux is out of scope as a product platform (the Brauser service still builds and tests on Linux for development, §12.2 step 10.2). Sections that still say "macOS only" predate this decision and are updated as the Windows work lands.
- **License:** Apache-2.0, chosen for its explicit patent grant.

## 14. Open Questions

No open M0 or M1 design questions. The release hosting, support, platform, browser, and signing policies are defined in §11; M1 security requirements are recorded in §4 and §5.1.

The decision records below are in force. Superseded decisions, and decisions now stated in full in the sections they govern, are in history. (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

The 2026-10-27 Capture Site drafting decisions (`brwr-t40`), now stated in §5.1: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

Product direction decision (2026-10-28, John):

- **Brauser is AI-first; there is no host-less "Lite" tier.** Brauser exists to use AI to improve the user's browsing at work, and its value has to be an experience a plain browser history cannot give. A tier that drops the native host and the agent service for a store-only install (notes, capture, Recent, and worklogs through the File System Access API and an in-extension SQLite index) was considered and rejected: without AI it is a glorified history and does not set Brauser apart. The native host and the local agent harness stay part of every install. Release work makes the two-part install (§11) easy and does not remove it. Judge new features by the AI-driven value they add, not by capture or recall alone.

Summarize frames decision (2026-10-28, John: relax privacy rules):

- **Summarize reads every frame (John: relax privacy rules).** Summarize & log injects Readability and the extractor into all of a tab's frames, so a document whose text lives in an embedded frame (Word for the web's `*.officeapps.live.com` editor) summarizes instead of failing as unreadable. The top frame's content wins when it holds at least 200 trimmed characters or the most text, unless Readability found no article there and a subframe holds five times its text; otherwise the longest subframe's is sent. A 10 s bound on the all-frames call falls back to the top frame alone; the Readability injection has the same bound, past which the extractor runs without it and falls back to body text. The title and the `page_changed` address check stay with the top frame, and failed frames are skipped. The link-shape and Tier 1 extractors stay top-frame only (§5.2).

Capture Site Improve with AI decisions (2026-10-28, `brwr-f45`; plan `docs/superpowers/plans/2026-10-28-capture-site-improve-with-ai.md`):

- **Structural first, agent second.** The structural draft always renders first and **Improve with AI** refines it on demand; the agent never replaces the structural pass, and **Use basic draft** returns to it. The recurring-pages hint drafts structurally too and offers the same button, replacing §5.1's earlier rule that the hint drafted with the agent when one was set up.
- **One click, then the same confirm.** No prompt, extra choice or harness picker: the default harness from settings runs, and its result goes through the same review, tenant pre-fill and one-click Save.
- **Same checks, no new trust.** An agent rule is `Drafted`, never `UserWritten`, so every failing check drops it. The service validates only the answer's form (one repair turn); the host alone judges safety, and a forbidden key or a foreign origin rejects the whole agent draft before any preview.
- **Unseen AI rules start unchecked** (`unchecked_reason: "unseen"`).
- **A rejected or failed agent draft keeps the basic draft**, with one plain line saying why.
- **Closing the review cancels the run**; a review reopened on the same origin reattaches to an unfinished one.
- **Additive within version 7.** `start_site_draft`, `RunKind`, `StartRunRequest.kind`, `RunView.kind`, `RunView.site_draft`, `DraftSiteRequest.run_id`, `SiteDrafter` `agent` and `UncheckedReason` `unseen` are additive (protocol/README.md).

Task bar decisions (2026-10-28, decided autonomously by the orchestrating agent under John's standing approval, each open to revisiting; plan `docs/superpowers/plans/2026-10-28-task-bar.md`, whose Decisions section holds the full list):

- **A dedicated host-built request.** The extension sends `start_task` (page fields, instruction, nullable harness, `requested_at`) and never a `kind` or record key; the host bounds the instruction, resolves the key and forwards a `start_run` with `kind: "task"`. A `start_run` from the extension that carries an `instruction` is refused. Rejected: inferring the kind from an instruction on `start_run`.
- **Additive within version 8.** `start_task`, `RunKind` `task`, `StartRunRequest.instruction` (host to service only) and `RunView.instruction`/`task_report` are additive (protocol/README.md "Agent tasks"). An older host answers `invalid_request` and the panel asks for a rebuild; an older extension rejects a task view, so the host and extension deploy together. An older service cannot read a task's `status.json`, so roll back only with no task active.
- **The user's request is the directive; the page is untrusted context** (§4.5): fenced request, page by path only, page steps only when the request asks, protected Brauser locations named in the prompt and failing closed when unknown, prompt-level only.
- **One task slot, four active, no joining**, so a long task never starves summaries.
- **Free-text report, nothing written**, with a task-specific repair turn and no Codex `outputSchema`.
- **Placement and gestures** (§5.7): the bar sits under the page area on both tabs, Enter submits, there are no Chrome notifications in this cut, and the bar is never pre-filled from page or agent text.
- **Diagnostics never carry the instruction or the report** (§15).

The 2026-10-27 home folder decision (one home folder, fixed layout, moved once), now stated in §6.1: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

Open access decisions (2026-10-26, John; [the open-access spec](docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md) §3). These supersede the earlier decisions below wherever they conflict, including the M2 extraction-permission, denylist-setup, and side-panel "notes on any page" decisions:

- **Standing access.** The manifest requests `http://*/*` and `https://*/*` as install-time `host_permissions`, with `webNavigation` and `scripting` required and `activeTab` dropped. Chrome's single install warning is accepted. The panel reads any HTTP(S) tab's URL and title, a note can be typed on any page at once, and there is no first-note or "can't see this tab" state (§5.3, §8).
- **The privacy and selective-capture machinery is removed.** Per-site Chrome grants as a capture gate, the native confirmation for config changes (`confirm_config`, consent tokens, the every-account disclosure in a dialog), the AI denylist and its confirmation, `log_incognito`, Chrome grant reconciliation, and the sync caution are gone. Config saves with the revision check only. The native folder picker and its single-use token stay, because they prove which folder Brauser may write, not a privacy choice (§7, §7.3).
- **A site is on when it has an adapter.** `sites` leaves the protocol in v6; the host derives capture origins from the adapters and still re-checks every delivered event. **Path-prefix narrowing is dropped**: an adapter's own match rules are its path rule. Origins an earlier build had removed or seen revoked stay out of capture through a pending-removal list carried across the upgrade (§3.2, §7.2).
- **Harness setup is detect and probe.** No native confirmation and no single-use harness token; the chosen harness is detected again and committed only when its probe passes (§7.1). Since PR 3 the service does both, and the harness lockdown is gone (below).
- **A knowledge base folder.** A new setting, chosen with the native picker under its own purpose-bound token and defaulting to the home folder, that a summary agent reads to judge importance (§6.1).
- **Summarize & log moves to a per-user Brauser service** with full-access Codex or Claude Code runs on the user's normal login, polled from the panel; this is PRs 2–4 of the spec, and §5.2 and §7.1 are rewritten as they land. The open question below about where a Chrome-spawned host gets auth values is answered there by the service's environment and an `agent.env` file (spec §5.1), recorded as built below.
- **Protocol v6.** These removals, `set_up_harness`, and `knowledge_base_dir` ship together as protocol version 6; v5 config files load unchanged and drop the retired keys on their next save (protocol/README.md).

Brauser service decisions (2026-10-26, as built in PR 2, §12.2 step 10.2):

- **Auth values for agent runs: closed.** A Chrome-spawned host no longer runs agents; the per-user service does, with launchd's environment from its plist (`HOME`, `USER`, `LANG`, and a `PATH` with `/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin`). At the start of every run the service reads an optional `agent.env` (KEY=VALUE lines) in Brauser's config directory and overlays it. The file must be a regular file of mode 0600 that is not a symlink, or the run fails before it starts; no error ever quotes a value from it, and settings will show only its names, never values. Codex needs nothing there; a Claude Code on Bedrock user puts `CLAUDE_CODE_USE_BEDROCK`, `AWS_PROFILE`, and `AWS_REGION` there. This closes the open M2 question below and backlog `brwr-9yc`.
- **Build identity.** A LaunchAgent service left running from before a rebuild speaks the same protocol version as the new host but runs old code. Before forwarding, the host sends `service_hello` and compares the returned `build_id` (the build label the page headers show, `<version>+<commit>`, plus the executable's device, inode, and modification time) with its own. Both come from one stamp, `BUILD_LABEL`, so the headers and this check never name builds two ways. On a mismatch it restarts the service once (`launchctl kickstart -k`); a second mismatch is `service_unavailable`. The restart is decided under an exclusive lock on `restart.lock` in the support directory, and the host sends `service_hello` again once it holds the lock, so when several hosts find the same stale service only the first restarts it and the rest use the fresh service rather than killing it and its runs. A host restarts the service only while its own startup build id still matches the executable on disk: a host that has outlived a rebuild (a long-lived panel port) is the stale party, so it answers `service_unavailable` asking the user to reload the extension and never kills the current service. A `service_hello` cut off by a service exiting at its idle deadline is retried once on a fresh connection. The host and the service both refuse a `start_run` past the schema's url, title, content, or selection bounds, and stored titles and run errors are cut to 2048 bytes, so a full `list_runs` always fits one outbound frame.

Summary runner decisions (2026-10-26, as built in PR 3, §12.2 step 10.3):

- **Full access replaces the lockdown.** Runs use Codex's `danger-full-access` sandbox with `approvalPolicy: "never"` and Claude Code's `--dangerously-skip-permissions`, in the knowledge base folder, on the user's normal login (spec §6). The argument templates, `--help` and `codex features list` checks, reviewed Codex feature tables, the emptied environment with its allowlist, host-owned work directories, the Codex event allowlist, and the §7.1 canary of step 8.2 are deleted, not kept beside it. What remains is that page text never goes on a command line, the prompt marks the page untrusted, the answer is validated, and agent text is shown as text only (§4.5, §7.1).
- **Verified flags.** Checked on 2026-10-27 against the installed binaries by reading their strings, not by running them (TESTING.md records the details). codex-cli 0.144.4: `codex app-server --listen stdio://`, with no `--session-source` flag; `thread/start` takes `sandbox: "danger-full-access"` (kebab-case) while `turn/start` takes `sandboxPolicy: {type: "dangerFullAccess"}` (camelCase), both with `approvalPolicy: "never"`; notifications `item/agentMessage/delta`, `item/completed`, and `turn/completed`. Claude Code 2.1.311: `-p --output-format stream-json --verbose --include-partial-messages --dangerously-skip-permissions` with `--session-id <uuid>` on the first turn and `--resume <id>` after it; `stream-json` needs `--verbose` in print mode, `--include-partial-messages` needs `--print` and `stream-json`, and `--session-id` must be a UUID. A **[human]** run of each real binary still has to confirm them.
- **`outputSchema`.** The installed app-server's `turn/start` params include `outputSchema`, documented as an optional JSON Schema constraining the turn's final assistant message, so Codex runs pass the answer schema there; the Claude Code runner passes no schema and relies on the prompt. Both still go through the same validation and repair turn, because the service does not rely on a harness enforcing the schema, and only the service can check `related_notes` against the knowledge base. Generating the app-server's schema with `codex app-server generate-json-schema` to confirm this is a **[human]** step.

Summary output decisions (2026-10-26, as built in PR 4, §12.2 step 10.4; spec §5.2, §7):

- **A summarized page is an artifact.** A summary of a page no adapter recognizes creates or updates a `web.page` artifact (`web:<host>/<url_id>`) keyed by the page's URL record key; on a page an adapter recognizes, the summary attaches to that adapter's artifact instead (a key recovered from a removed adapter takes its indexed type, and an artifact of unknown type falls back to the page's `web.page`). Either way it gets an artifact file, Timeline lines, a Recent row, and worklog wording, but never counts as an open or an opened day (§6.2, §6.7).
- **Config at write time; a changed key is followed.** The service saves under the configuration current when the run finishes and writes under the key the URL resolves to then, recording it as `written_record_key`, instead of discarding a run whose key changed (§5.2 step 7).
- **Summaries log while capture is paused.** Logging a summary is the user's explicit, per-click action, so neither a pause nor the page's site being uncovered stops its `summary` event; automatic capture stays off.
- **`requested_at` carries the user's offset.** As with every captured event, the extension supplies the user's local offset: the panel's `requested_at` (the click's local time with its offset) sets the offset of `summarized_at` and of the log event, and the summary lands in the daily log of the user's own day. The time itself is the answer's `answered_at` stamp, so a retry builds the same entry.
- **`open_summary` uses the default app.** The native host (not the service) opens an owned summary file (the named `summary_path`'s, or the page record's) with PR 3's desktop opener, `/usr/bin/open -- <path>`, so it opens in whatever app the user has for Markdown; anything else at the summary name is refused (protocol/README.md). The config lock is held through the open, and just before it the path must still name, without a symlink, the inode whose bytes passed the ownership check; the remaining window between that check and `open`'s own path lookup is accepted, as `replace_owned_file`'s replace window is, since `open` takes no file handle.
- **A context-free escape replaces the code-span neutralizer (2026-10-27).** Agent body text is escaped with entities and backslashes that hold whatever the surrounding parse, instead of wrapping each construct in a code span, and backticks, `$`, tabs and fence-length runs are entities so nothing reaches Mermaid, Dataview or math; front matter stays plain JSON-quoted YAML with no entities, since Obsidian and Dataview read it literally (§5.2 step 6).

Product decisions (2026-10-16, John, from the architecture review):

- **Windows is back in scope.** This supersedes the 2026-10-15 macOS-only decision below. Keep the existing Windows code paths (do not remove them as "vestigial"). Restore Windows CI first, then fix the code that has assumed macOS since 2026-10-15. Interactive Windows checks (picker, dialogs, registry registration, installer) need a person on Windows. Edge on Windows is worth evaluating at the same time, since it is Chromium.
- **Users never see adapters: Capture Site.** Adapters are machinery under the UI. The primary way to start capturing a site that no starter adapter covers is a **Capture Site** button in the side panel. It drafts the policy with the host's structural drafter (no agent required), and the user's configured harness can refine the draft after M2 host invocation. The review is written in plain language: what Brauser will remember on this site, with example pages it matched, and what it will not capture, using the existing match preview and fixed safety probes. The adapter JSON editor stays as an advanced view, not a step in any normal flow. Every structural-drafter and agent-drafter safety rule in §5.1 and §12.2 steps 7.6–7.7 still applies.
- **Re-learning replaces the frozen ID.** A user who never sees an adapter cannot hand-correct a wrong machine-drafted ID, so Capture Site ships with a Re-learn action that migrates existing activity, notes, and index rows through explicit, append-only alias/re-key events. That migration replaces the conservative "ID frozen until alias migrations exist" guard and its `adapter_id_history`, rather than relaxing it.
- **`index.db` moves to a real SQLite library.** Step 6.6 shipped a hand-written SQLite file writer, plus a whole-history JSON cache rewritten on every delivery, because the overnight sandbox cannot fetch crates. Replace both with `rusqlite` (bundled) and incremental, transactional updates (`brwr-b2e`, after the dependency is added outside the sandbox, `brwr-mff`). The log stays authoritative and `brauser reindex` remains the rebuild. M3's FTS5 index requires this.

The 2026-10-15 macOS-only platform decision, superseded on 2026-10-16: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

Artifact decisions (2026-09-28):

- The 2026-09-28 artifact decisions now stated in the sections they govern (sequencing, the M1.5 split, artifacts replacing per-site logging, the log as source of truth, measured attention, recognition in the worker, cold opens, the coarse log, no near-repeat window, aliases, trails, Recent and the worklog, mode, record keys, URL shapes, and adapter templates), with the later title-prefix, first-starter, Create adapter, anchor, and tab-map decisions: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

- **Containers (2026-10-27).** An adapter may declare `within`, naming a container artifact its own artifacts sit inside (adapter-templates.md §3, §8.3). A visit rolls up into its container once — each second, open, and local day counted there exactly once, days as a union of the container's own and its contents' days — and the index records the relationship as a `within` edge alongside `mentions` and `co-focused` (§5.1, §6.2). GitHub repositories are the first container: `github.repo` is a new catch-all starter over everything inside a repository that no PR or issue adapter claims, and `github.pr`/`github.issue` declare `within` it. A container's title and URL come from its ID, never from a sub-page's title, so a repository with rolled-up activity but no direct visit yet still shows `owner/repo` and a link to its home page; this applies while some adapter declares `within` the container's type, so a repositories-only setup keeps the latest sub-page title (accepted limit, docs/adapters/github.repo.md). Enabling the repository catch-all on top of already-configured PR or issue adapters would otherwise trip the overlapping-route guard (§6.2): any past edit to their scope leaves a retired contract the catch-all's routes overlap under a different type. The guard is amended to allow that overlap when the new adapter is exactly the retired contract's declared container, because those URLs were already counting toward it. Existing PR and issue history rolls up into repositories retroactively, because the index is rebuilt from the append-only logs and derives each event's container when the event is re-evaluated, not when it was originally written. Downgrading after adding repositories is unsupported: older hosts reject `within` in the adapter config and in adapter history.
- **Brauser owns its ID scheme (2026-09-28).** Brauser is a standalone product, so starter adapters use Brauser's own `artifact_id` form (§6.2) and do not follow any other tool's keys. Other tools read Brauser's IDs and alias events from the log if they want them. A starter adapter's `id` template is still frozen once it ships (§6.2).

Open artifact questions:

- **Adapter-template open questions** are tracked in adapter-templates.md §13: hash-routed apps, Notion pages with no workspace segment under a scoped adapter, and which further origins join the shared-origin list at release.

M2 decisions (2026-09-28):

- The extraction-permission, summary-destination, harness-mode, and denylist-setup decisions, superseded by open access or now stated in §5.2 and §6.4: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))
- **Codex isolation.** Codex ships in M2 running with the user's normal `HOME` and `CODEX_HOME`. A host-owned `CODEX_HOME` would keep the user's global `AGENTS.md` and skills out, but its copied credential could, on refresh, break or corrupt the user's own Codex login. Sharing the normal home avoids that. The accepted cost is that user-level Codex instructions and skills can shape Brauser's output.

The open M2 question about where a Chrome-spawned host gets auth values, closed on 2026-10-26 by `agent.env`: (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

Side panel decisions (2026-09-28):

- The editable-notes, no-AI-output, and `/log`-on-any-page decisions, now stated in §5.3 and §5.6, and the notes-on-any-page decision, superseded by Standing access (above): (history: [docs/history.md §14](docs/history.md#14-open-questions--history))

## 15. Diagnostics

Brauser keeps local diagnostics so a failure can be followed after the fact: structured logs and spans from the native host, the service, and the CLI, and a small ring buffer in the extension. They are files on this computer. Nothing is sent anywhere, and there is no setting that sends them; a user shares them only by copying them out.

**Files.** Each process appends JSON lines to its own file in the log directory: `host.jsonl` (every native host process Chrome starts), `service.jsonl` (the Brauser service), and `cli.jsonl` (`brauser reindex`, `brauser adapters`, and other commands). A file is rotated when it reaches 5 MiB, to `.1` (newest) through `.3`; older copies are deleted. Files are created mode 0600 on Unix. Many host processes may write `host.jsonl` at once: each event is one append, and a process that finds the file already rotated by another follows it instead of rotating again (`RotatingFile` in host/src/diag.rs).

| OS | Log directory |
|---|---|
| macOS | `~/Library/Logs/Brauser/` (beside the development service's `service.log`) |
| Windows | `%LOCALAPPDATA%\Brauser\data\logs\` |
| Linux (development only, §13) | `$XDG_DATA_HOME/brauser/logs/`, default `~/.local/share/brauser/logs/` |

`BRAUSER_LOG_DIR` replaces the directory. `BRAUSER_LOG` sets the filter in `tracing-subscriber` `EnvFilter` syntax (`debug`, `brauser_host::service=debug`); the default is `info`. Test builds and `scripted-dialogs` builds write no file unless `BRAUSER_LOG_DIR` is set, so tests and the headless smoke run never touch the real directory. A directory that cannot be opened never fails the process: it logs to stderr only and says why once.

**Line shape.** `tracing-subscriber`'s JSON format, one event per line:

```json
{"timestamp":"2026-10-28T12:02:48.433990Z","level":"WARN","fields":{"message":"request failed","outcome":"service_unavailable","error":"…","elapsed_ms":40},"target":"brauser_host::native","spans":[{"name":"process","proc":"host","pid":49063,"build":"0.1.0+c8b3bd86a120"},{"name":"request","request_id":"…","kind":"start_run"}]}
```

`timestamp` is UTC. `level` is `TRACE` through `ERROR`. `fields.message` is a short, stable, lower-case event name (a grep key); the other `fields` are the event's own. `spans` lists the spans the event happened in, outermost first. The first is always the root `process` span: `proc` (`host`, `service`, or `cli`), `pid`, and `build` (`BUILD_LABEL`, the `<version>+<commit>` label the page headers show), so lines from many short-lived host processes stay apart. Spans write no open or close lines of their own; each ends with an explicit event (`request done`, `service call done`, `run finished`, `connection closed`) that carries its outcome and `elapsed_ms`. A panic is logged as an `error` event `panic` with its location, in the spans it happened in, before the default hook prints it.

**Correlation.** Every protocol request carries a `request_id` the extension makes (`crypto.randomUUID`); the host enters a span with it for the request and forwards it to the service with a run message, so one id follows a click from the extension, through the host, into the service. A run's `run_id` is a span field for everything the service does for that run (queue, harness turns, finishing). Field names are fixed:

| Field | Meaning |
|---|---|
| `request_id` | the extension's id for one protocol request |
| `kind` | the request's protocol tag (`start_run`, `visit`, `update_config`, …) |
| `run_id` | the service run |
| `harness` | the run's `HarnessId` (`codex` or `claude`) |
| `outcome` | `ok`, or the protocol `ErrorCode` as its wire string (`service_unavailable`) |
| `error` | the error's message text |
| `elapsed_ms` | wall time of the step the event ends |
| `origin` | a page's origin, the only form of a URL in a log |

**Levels.** `error`: something failed that the user will notice. `warn`: degraded but continued (what was an `eprintln!` warning). `info`: one line per request and per run lifecycle step. `debug`: detail, such as batch internals. The info volume stays modest: one line per capture or visit request, never one per event in a batch.

**stderr.** Warnings and errors are also written to stderr as plain text, as before the files existed: Chrome's log for the host, and launchd's `~/Library/Logs/Brauser/service.log` for the development service (§3.2).

**Extension.** `extension/diag.ts` keeps a capped in-memory ring buffer of recent events in each extension context (`{ts, level, ctx, event, fields}`, with `ctx` naming the context), under the same field names and privacy rules, and puts the `request_id` it sends on its own events. The options page's **Copy diagnostics** copies the buffer to the clipboard as JSON lines; nothing else takes it out of the extension.

**Reader.** `npm run logs` (`scripts/brauser-logs.mjs`) reads all three files and their rotations from the directory above (or `BRAUSER_LOG_DIR`, or `--dir`), merges them by time, and prints one compact line per event: time, `proc:pid`, level, message, and the key fields, with ids shortened. It filters by `--request` and `--run` (a prefix of the id, matched on the event or any of its spans), `--level`, `--errors` (warn and up), `--since` (`10m`, `2h`, or an ISO time), and `--proc`; `--json` prints the raw lines, `--follow` tails the live files, and `--extension <file>` merges a Copy diagnostics export as proc `ext`. It counts and reports malformed lines instead of failing, and never writes to the log directory.

**Privacy.** Diagnostics record what happened, never what the user read or wrote. Never log page text or extracted content, note text, titles, prompts, answers, task instructions, task reports, streamed agent output, a harness's stdout or stderr, file contents, or any `agent.env` name or value. A URL is logged only as its origin (`diag::url_origin`; `https://example.com`). A vault path is logged only relative to the home folder, or as a file name. Errors are logged with their message text, as the stderr warnings already were. A harness's own output stays in its run's `run.log` (§4.5), never in a diagnostics file. The task bar adds no log field of its own: in the host (`start_task`'s `request` span, its refusal texts, which never quote the instruction), the service (the forwarded `start_run`, the `run` span with `kind = "task"`, the turn and answer events, `TaskFinisher` and `log_save`; the Codex thread name travels only in `thread/name/set` params, which `rpc request` does not log) and the extension (`diag` never gets the instruction, the report or the page title), a task logs only what a summary does: ids, kind, harness, origin, outcome codes, sizes and timings.
