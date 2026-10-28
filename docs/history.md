# Brauser design history

> This file is the append-only record of how Brauser's design got to where it is: dated decisions, as-built notes, superseded designs, probe and acceptance results, review reproductions, and the build log. It is not the contract. [DESIGN.md](../DESIGN.md) is the contract: what Brauser does and must do now. When a decision or an as-built note changes a rule, append the dated record here, under the section it concerns, and update DESIGN.md only to the resulting current rule. Never edit or delete a record here except to fix a broken link.
>
> Records are grouped by the DESIGN.md section they concern and keep their original dates and wording. Records moved here from DESIGN.md and AGENTS.md on 2026-10-28 (`brwr-m68`) are in their original order within each section. Relative links in moved text were written for the repository root; they are rewritten to work from `docs/`.

## §3.2 Native host — history

(The `macos-alert` crate, the one exception, was removed with the native Yes/No confirmation in protocol v6.)

*Background capture, the M1 grant wall (the paragraph was rewritten to the current lease rule on 2026-10-28):* Before writing any record, the worker awaits `setAccessLevel({accessLevel: "TRUSTED_CONTEXTS"})`, loads a revisioned, host-confirmed site policy with a bounded expiry, and checks the current Chrome grant; absent or expired policy fails closed. The host independently enforces its confirmed site allowlist and privacy rules against the original URL before any lossy normalization. The host cannot inspect Chrome's live grants. Disabling logging or confirming a site grant revocation purges affected records. The side panel and settings page serialize config changes and Chrome grant cleanup across every open window through one shared lock, and recheck the grant before enabling capture. An explicit re-enable clears a prior revocation only when the worker has the committed host revision and Chrome currently grants that origin. A background sync sends only `get_config` and `record_batch`; config changes, consent, and native dialogs stay with the panel and settings page, and the host re-checks every delivered event against its own confirmed config either way.

**Open access (protocol v6, 2026-10-26).** Parts of the paragraph above describe the M1 grant wall, which is gone.

## §4.4 Filesystem rules — history

*Summary-file replacement, before the bullet was trimmed to current behaviour:* (As built in PR 4: the service's `SummaryFinisher` writes them through the shared ownership-checked `replace_owned_file` (host/src/vault.rs), and a failed write surfaces as the run's `save_failed` with Retry save, not a `retryable` outcome; §5.2 step 7. The rest of this bullet is the M2 design.)

*From the M2 design of the same bullet:* A sharing violation or access-denied error while another program holds the file is retried briefly, then returned as `retryable` with nothing changed.

## §4.5 Agent invocation rules — history

- The agent denylist, the single-use harness token with its native confirmation, and the PR 2-era lockdown (argument templates, `--help` and feature checks, an emptied environment with an allowlist, and host-owned work directories) were removed (protocol v6 and open access PR 3, 2026-10-26, §14).

## §5.1 Artifact logging — history

*Create adapter, as designed for M1.5b before open access:*

- **Starting.**
  - The panel cannot see an ungranted tab's URL (§5.2). On an ungranted page, Create adapter therefore starts, like the first note, from the page context menu (**Create adapter for this site**) or the toolbar action. Both grant `activeTab`, so the panel learns the tab's origin.
  - The panel then shows the button with the origin named. Clicking it requests that exact origin from Chrome while Chrome still has the user gesture. The worker keeps a record in `storage.local` of why each origin is granted (logging, notes, or Create adapter). It records this grant with the purpose `create-adapter` and a 30-minute expiry.
  - Nothing is captured until the final native confirmation. If the user cancels, closes the review, or lets it expire, the worker removes the `create-adapter` purpose. It removes the grant itself only when no other purpose holds the origin, for example a note taken there in the meantime (§5.3). The worker checks for expired purposes when it starts and on an alarm.
  - On an origin that is already granted, for logging or for notes, the button works directly and records no new grant.

*Create adapter, as designed for M1.5b before open access:*

- **Handing over.**
  - At the click, the panel keeps the shapes and the tab's ID only in `storage.session`, under a one-time draft ID with its own 30-minute expiry, and opens the settings page's adapter editor with that ID.
  - The settings page's host builds the draft, because the panel's host and the settings page's host are separate processes (§3.2).
  - The settings page reads the tab's current title from tab metadata, which it can do on the now-granted origin, and shows it only in the review. The title is never stored and never sent to the host.

*Structural drafter, step 3 (Keep):* **Amended 2026-10-27:** this step first required a route word, with the tenant not counting; it now uses the same scoped-capture anchor rule as validation (#126), and the per-rule checks decide whether the scope really restricts the tenant.

*Structural drafter, step 4 (Map), route words that name namespaces:* **Amended 2026-10-28:** the drafter first gave every rule of such a kind one ID that left the word out.

*Create adapter, shared origins:* **Superseded 2026-10-27 (John: "make adapter creation as easy as possible and do the work on our side").** This step said "Enable stays disabled until the user chooses", with the prefilled list unchosen. This reuses the GitHub starter's pre-fill (#126), whose organization field is filled from the URL and confirmed by a line in the review.

**As built (2026-10-28, `brwr-f45`; plan `docs/superpowers/plans/2026-10-28-capture-site-improve-with-ai.md`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Improve with AI”.

*Create adapter, checks and trust:*

- Enabling takes the same native confirmation as any adapter, including the counts of visited and link shapes it matches.

**As built (2026-10-27 — Capture Site, known-site slice).** The rule this record introduced is now stated in DESIGN.md §5.1, “Capture Site for starter sites”.

**As built (2026-10-27 — private page-pattern review, `brwr-b39`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Private page-pattern review”.

**As built (2026-10-28 — restore recorded site rules, `brwr-cua`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Restoring recorded site rules”.

**As built (2026-10-27 — Capture Site for unknown sites, `brwr-t40`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Capture Site for unknown sites”.

*Capture Site for unknown sites, pending scope:* (Until 2026-10-27 the pre-filled list started unchosen; see the superseded step above.)

**Drafting contract decision (`brwr-gf1`, 2026-10-27; revisited the same day).** The first decision kept the leading-literal rule as written: on `www.notion.so` the first segment is the host-supplied `workspace` tenant capture, a `name` and not a route word, so `/acme/Release-Plan-<hex32>` and `/acme/<hex32>` were refused as `draft_no_leading_literal`.

*Opens, from the step 6.1 probes:* In the step 6.1 probes, a restored tab's commit made while the extension was loaded and its worker stopped woke the worker and reached it. An extension loaded or installed during startup, however, missed the active tab's commit, which Chrome makes a few hundred milliseconds after launch, before the worker's first start; that leaves the tab unmapped, with no time counted, until its next navigation. Background tabs commit seconds later and were seen. Whether Chrome loads an installed extension before that first commit could not be tested headless (§12.2 step 6.1).

**Rebuilding missing entries at worker start (decided 2026-09-28).** The rule this record introduced is now stated in DESIGN.md §5.1, “Rebuilding missing entries at worker start”.

*Rebuilding missing entries at worker start:* It is built with activity events (§12.2 step 6.4).

**Re-learning groundwork (2026-10-28, `brwr-sq9`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Re-learning groundwork”.

**Known gaps when sharing a worklog (2026-10-28, `brwr-9r7`).** The rule this record introduced is now stated in DESIGN.md §5.1, “Known gaps when sharing a worklog”.

**Sync caution (removed 2026-10-26).** Earlier builds warned in the first adapter's confirmation when the folder was inside a known sync location; protocol v6 removed that warning with the native confirmation (§14).

## §5.2 Summaries — history

> **Superseded in part (2026-10-26).** Summaries move to a per-user Brauser service with full-access Codex or Claude Code runs, polling instead of streaming, and no denylist or `activeTab` trigger; see [the open-access spec](superpowers/specs/2026-10-26-open-access-and-agent-service-design.md) §5–§7. The Summarize & log flow below is as built in PRs 3 and 4; steps 1–3 of the older numbered list after it (the `activeTab` triggers, the denylist checks, and streamed chunks) no longer apply. Of its steps 4–7, PR 4 replaced step 4 (the write now follows the configuration current at finish, a changed key included, rather than discarding the output) and step 7 (save outcomes are the run card's Retry save, not `summary_result`); of steps 5 and 6, only the summary file's identity and ownership rules and the Markdown neutralizer still describe what was built (step 5's denylist and its "the host writes" wording no longer apply: the service's `SummaryFinisher` writes the file, step 7 above).

The older M2 design follows.

1. The extension extracts readable content (Readability-style) from a tab on demand. Any HTTP(S) page can be summarized. Without `tabs`, the side panel cannot see an ungranted tab's URL, and a side-panel click does not grant `activeTab` (§8). Summarizing therefore starts in one of two ways:
   - A trigger that grants `activeTab`: the `summarize` keyboard command, the page context menu's "Summarize with Brauser", or the toolbar action. The grant is temporary, needs no prompt, and covers only that tab until it navigates. The worker records the tab and document the grant was issued for, opens or focuses the panel synchronously in the handler (`sidePanel.open`, before any await), and the panel summarizes only that target.
   - The panel's Summarize button, when the extension already holds the page's exact origin or an `activeTab` grant for it. On any other page the button tells the user to use the shortcut, context menu, or toolbar action. It never asks for broad access, and it does not use the logging-only `webNavigation` permission to learn tab URLs.

   A trigger for summarizing does not enable logging; the host's site rules still decide what is logged.
2. The panel sends the tab's URL to the host, which answers `allowed` or `denied` from the denylist (§4.5). On `denied` nothing is extracted. Otherwise the extractor runs in the main frame only and returns `location.href`, `document.title`, and the content together. If the canonical form of the returned URL differs from the one checked, the panel discards the content and reports that the page changed. The host checks the returned URL against the denylist again, derives the summary's record key (§6.2) from it alone, and invokes the configured harness with the summary prompt.
3. Output streams back to the sidebar in chunks, staying under the host's 900 KiB outbound cap (§4.3) per message. Granularity depends on the harness (§7.1). Agent output is untrusted: the panel renders it only as text (`textContent`), with no Markdown-to-HTML conversion, so it loads nothing and never navigates by itself. Streamed text is provisional; a `rejected` run (§7.1) clears it.
4. The host holds no config or capture lock while the harness runs. It snapshots the config revision and harness entry at spawn. Before writing, it takes the config lock and a per-page summary lock, re-reads config, and rechecks the notes-folder identity, the summaries location, the denylist for both the original and the returned URL, the record key (§6.2), and that the harness entry still matches. Any failure discards the output and nothing is written. The per-page lock serializes concurrent summaries of one page; the last to finish replaces the file. Canceled, timed-out, over-cap, rejected, non-zero-exit, and panel-closed (EOF) runs write nothing.

*Summary neutralizer (§5.2 step 6):* An earlier scanner (2026-10-26) turned each construct into a code span. That required mirroring each viewer's handling of fences, list items, table cells, code spans, link titles and autolink literals, and three review rounds kept finding inputs that a real renderer still rendered live, so it was replaced on 2026-10-27.

7. The final message is `summary_result`, correlated by `request_id`, sent after the write is attempted. Its `outcome` is `created`, `replaced`, `created_with_warning`, `replaced_with_warning` (the file is in place but temp cleanup or directory sync failed, as with page notes; not a failure and not retried), `conflict` (names the occupying file and says to move or delete it), or `retryable` (a transient vault error before publication), with `relative_path` and `message`. After `conflict` or `retryable`, the host keeps the final text in memory, keyed by `request_id`, until the write succeeds, the panel discards it, or the connection closes; Retry sends only the `request_id` and does not rerun the harness. The panel keeps the text on screen with Copy, Retry save, and Discard; if the connection is lost it becomes a copy-only unsaved view. A canceled, timed-out, or failed run shows its partial text marked unsaved.

## §5.3 Page notes — history

**Local recovery (built 2026-10-27, `brwr-3nq`).** The rule this record introduced is now stated in DESIGN.md §5.3, “Local recovery”.

## §5.7 Side panel layout — history

**Capture exclusions (built 2026-10-28, `brwr-vny`).** The rule this record introduced is now stated in DESIGN.md §5.7, “Capture exclusions”.

**Incoming navigation (built 2026-10-28, `brwr-1g2`, protocol v8).** The rule this record introduced is now stated in DESIGN.md §5.7, “Incoming navigation (protocol v8)”.

*Side panel states, “Origin not enabled” row, before Capture Site was built:* Chip `○ Not logged · Set up artifacts for this site… · Create adapter`, which opens settings with the origin filled in and offers any matching starter adapter. With standing access (protocol v6) the panel's Create adapter button works on any tab (§5.1). **As built (2026-10-27, `brwr-t40`):** the site-capture area shows **Capture Site** (the row now states the result).

*Side panel states, the row for a non-artifact page on an enabled origin, before Capture Site was built:* No artifact card; chip `○ Not an artifact · Create adapter`. When unrecognized pages recur, a hint also offers to draft one from them (Tier 2, §5.1). **As built (2026-10-27, `brwr-t40`):** the chip reads `○ Not an artifact`.

## §6.1 Folder layout — history

**Moving an older layout (2026-10-27).** The rule this record introduced is now stated in DESIGN.md §6.1, “Moving an older layout”.

## §6.2 Internal model — history

**Retired history budget (2026-10-28, `brwr-2iz`).** The rule this record introduced is now stated in DESIGN.md §6.2, “Retired history budget”.

**Note keys outlive capture (2026-10-27).** The rule this record introduced is now stated in DESIGN.md §6.2, “Note keys outlive capture”.

**Policy removal acceptance (2026-10-27, revised the same day).** The rule this record introduced is now stated in DESIGN.md §6.2, “Policy removal”.

*Policy removal acceptance:* Review reproduced the earlier recovery reading the board, and another issue's modal, as the first issue and autosaving over its note, and authorizing the board's URL note to migrate into it.

**Explicit preserved-note viewing (2026-10-28, `brwr-r88`).** The rule this record introduced is now stated in DESIGN.md §6.2, “Explicit preserved-note viewing”.

**Hand-removed contracts (2026-10-27, `brwr-hrc`).** The rule this record introduced is now stated in DESIGN.md §6.2, “Hand-removed contracts”.

**Stripped parameters an adapter reads (2026-10-27, `brwr-f4s`).** The rule this record introduced is now stated in DESIGN.md §6.2, “Stripped parameters an adapter reads”.

**Names beside stored URLs (2026-10-28, `brwr-srp`).** The rule this record introduced is now stated in DESIGN.md §6.2, “Names beside stored URLs”.

## §6.7 Example artifact file (neutral profile, M1.5b) — history

*Artifact files, lazy regeneration:* Step 6.6 implements the connection-boundary pass and `brauser reindex`; the settings-facing action lands with the settings and panel work in step 6.7.

**Log ingestion recovery (built 2026-10-27).** The rule this record introduced is now stated in DESIGN.md §6.7, “Log ingestion recovery”.

## §7 Configuration — history

*Configuration changes:* Before v6, adding or widening capture, enabling it, or weakening a privacy exclusion also needed a native host confirmation and a single-use consent token; that confirmation is gone (§14).

## §7.1 Agent harnesses — history

> **Rewritten for open access PR 3 (2026-10-26).** Agents run in the Brauser service with full access (§4.5, §14). The M2 step 1 lockdown this section used to describe (per-harness argument templates with no tools and a read-only sandbox, `--help` and `codex features list` flag checks, reviewed Codex feature tables, an emptied environment with an allowlist, a host-owned work directory per run, the Codex event allowlist, the §7.1 canary, and the native setup confirmation and harness token) is gone from the code and from this section.

## §7.2 Artifact adapters — history

**Starter rule updates (2026-10-28).** The rule this record introduced is now stated in DESIGN.md §7.2, “Starter rule updates”.

*Saving an adapter:* Before v6, enabling or widening an adapter requested the origin's optional Chrome grant and a native confirmation.

**As built (M1.5a step 6.3, protocol v5 — config slice).** Adapter additions and edits use the existing revision-bound native config confirmation. The host uses that same stable identity when deriving native-consent changes: removing an earlier adapter or site does not re-confirm survivors merely because their array indexes shifted. It separately verifies that matched prior indexes remain ordered, so an actual precedence-changing reorder still requires native confirmation; duplicate identities are treated as changed rather than guessed.

**As built (M1.5a step 6.3, 2026-10-16 — lease and recognition slice).** Locally revoking an origin also narrows the leased adapter document, so the narrowed lease remains valid on the next worker wake. Recognized events still use the durable M1 visit writer only as the temporary persistence path; step 6.4 replaces them with hourly artifact activity events.

**As built (M1.5a step 6.3, 2026-10-16 — settings review slice).** URL-shape samples and re-map counts still wait for Create adapter in M1.5b step 7.6. Removing an origin's final site, including a Chrome-grant revocation reconciled at startup, removes its adapters in the same host update so a valid config cannot retain an adapter whose origin is no longer enabled. If the adapter document is already invalid or from a future format, revocation still removes the site and may turn capture off, but echoes the unsupported document without structural changes and keeps it suspended until explicit repair rather than reinterpreting it. Reconciliation rechecks the worker's revocation markers after acquiring the shared configuration lock, so a stale snapshot cannot remove an origin another settings page just re-enabled.

**As built (protocol v6, 2026-10-26).** The rule this record introduced is now stated in DESIGN.md §7.2, “Protocol v6 changes”.

## §8 Extension Permissions — history

Protocol v6 (2026-10-26) replaced the earlier permission model. Before it, the manifest declared broad *optional* HTTP(S) patterns and Chrome granted only the exact origin the user enabled, wrote a first note on, or drafted an adapter for; `webNavigation` was optional; `activeTab`, a `note` command, and the toolbar action granted temporary access to one tab; and settings and the worker reconciled revoked grants.

## §12 Build Order — history

**M0 as built (2026-09-27):** `protocol/schema.json` generates the shared Rust and TypeScript wire types. The Rust host implements framed native messages for discovery and configuration, checks the protocol version before decoding a version-specific request, and returns structured errors. Config updates require a revision from `get_config`; the host locks, rechecks, and atomically replaces a bounded config so stale panels cannot overwrite one another. The root-scoped vault API publishes new notes without replacing user files and reports cleanup or durability warnings after publication as a created file. Capture remains disabled until M1 adds site allowlisting and permission enforcement. The vault write API is internal and is not exposed as a native message in M0. CI is configured for macOS (then the only supported platform; a Windows CI job briefly also ran, dropped per the macOS-only decision, §14).

*M1 prerequisite:*

Before M1 exposes vault writes, the host must bind the notes root to a user selection made in a native OS folder picker. The current M0 `update_config` message alone does not prove user intent. M1 uses create-only page notes; an existing-file edit becomes a separate review artifact. The side panel rework (§12.2) replaces this with the versioned note replace in §4.4, which treats the side panel as the only editor; Brauser-owned summary files (§5.2) use the checked rename there.

## §12.1 M1 implementation and acceptance plan — history

2. **Make consent and permission state explicit.** The MV3 manifest declares `incognito: "not_allowed"` and broad *optional* HTTP(S) match patterns, with no site origin granted at install. A setup button requests optional `webNavigation` and the chosen exact origin directly from the live Chrome user gesture, before awaiting the host; path-specific adapter rules remain host-enforced. The host shows its own native confirmation for every privacy-expanding config change and issues a single-use token bound to the exact change and revision. If that confirmation is canceled, remove the newly granted Chrome permissions and keep capture off. Only after host confirmation can the settings page enable capture. The service worker caches the host-confirmed policy with its revision and bounded expiry, fails closed if absent or expired, and awaits trusted-only `storage.local` access before buffering. Removing a grant or disabling a site stops capture immediately and clears its pending records. The host also disables a revoked site when informed, while acknowledging that it cannot inspect Chrome's live grant state. The service worker checks the incognito flag before buffering.

4. **Write recoverable Markdown.** Normalize only HTTP(S) URLs, reject credentials, and drop fragments and configured tracking parameters without erasing meaningful query data after authorization. Site-specific transforms follow M1 as separately consented adapter rules. Under the config and capture locks, recheck the current host policy, sync an event intent outside the home folder, append accepted visits with IDs to `log/YYYY/MM/YYYY-MM-DD.md`, and sync before acknowledging persistence. Retry completes only a byte-exact interrupted prefix in the currently selected root; it never overwrites unrelated user edits. The near-repeat window suppresses redundant entries without rewriting user text. Create page notes using a mandatory URL-derived identity suffix and record the canonical URL. Verify identity and ownership before treating an existing note as already present. Never replace an existing note in M1: write any proposed edit as a no-clobber sibling review artifact keyed by the proposed content, so a replay reuses the same draft. Malformed markers are conflicts. The UI must distinguish created, already present, conflict, and created-with-warning outcomes.
5. **Verify the complete no-agent path.** CI covers normalization, permission and allowlist enforcement, incognito rejection, replay/idempotence including concurrent host processes, transient retry, traversal, and create-only page ownership/conflicts on macOS. A real Chrome smoke run with an unpacked development extension and development-only native-host registration checks first-run setup, panel close/reopen, visit replay, permission removal, picker cancellation, and page-note creation. No release claim or installer download link is made during M1.

**M1 validation status** (summarizing the dated entries below): The first macOS guided run was stopped by a persistent busy cursor after a native dialog. Native dialogs now run in a short-lived child process, and the repeat runs below, including the unattended `--auto` run, passed the full flow on macOS on nondefault and default ports. macOS CI builds and tests every change. A failed append rolls back its own bytes, and replay tolerates complete visits appended after an intent. Automated tests cover host identity/replay recovery and extension repair, pause, policy refresh, re-enable races, title capture, notice retention, and the compact M1.5a batch intent that replaced the unbounded per-event journal on the current delivery path. The old single-event protocol remains only to drain pre-upgrade queues. (Windows's interactive run, default-port check, and directory-entry flush validation were parked, then dropped with Windows on 2026-10-15; Windows was reinstated on 2026-10-16, §14, and its interactive acceptance is tracked separately, so these are not M1 blockers.)

**macOS smoke pass (2026-09-28):** `npm run smoke:macos` completed the full flow on macOS with Chrome 154: first-run warning, picker cancellation, declined consent (with the new Chrome grants rolled back), confirmed setup, allowed and blocked visit replay, panel reopen without duplication, page note creation, identical retry, one review draft and its retry, site removal with Chrome grants removed, and no capture after removal. No busy cursor persisted after any native dialog, which confirms the child-process dialog fix. The fixture's nondefault port was accepted by Chrome's permission request. Re-requesting access that Brauser had just removed did not show a second Chrome prompt, so the host's native confirmation, not Chrome's prompt, is the gate for that re-enable; the host still required it. The runner now drives the extension pages itself through the DevTools pipe and stops only for the picker, Chrome's prompt, and the host dialog.

**macOS default port and unattended run (2026-09-28):** `node scripts/smoke-macos.mjs --default-port --auto` passed the full flow with the fixture on port 80 and the site entered as `http://127.0.0.1:80`. Chrome granted exactly `http://127.0.0.1:80/*` and not the any-port pattern, and the host stored the origin without the port. A repeat run on a nondefault port also passed. `--auto` answers every prompt through macOS UI scripting, so the run needs no person once the terminal has Accessibility access. The host's folder picker does not answer Accessibility queries and is driven by keyboard. Its Yes/No alert is drawn by `UserNotificationCenter`, and Chrome's permission prompt is a sheet on the settings window, so both buttons are pressed by name. In place of the busy-cursor question, the runner checks that no dialog process or host window remains. **M1 is accepted for development on macOS.**

**Page note title (2026-09-28):** The panel's page-note form no longer has a title field. The host receives the tab's title, or the host name when the page has none; a change in that title still produces a review draft.

**Settings page (2026-09-28):** Configuration moved from the side panel to a dedicated `options_ui` page. The panel shows a gear button and a setup warning until a folder, a site, and capture are all in place. The page opened from the panel on macOS once Brauser was reloaded in `chrome://extensions`. Chrome keeps an unpacked extension's manifest and service worker from load time but serves rebuilt pages from disk, so a build that changes `manifest.json` or `worker.ts` needs that reload; the panel now says so when the settings page cannot open. `npm run smoke:macos` starts a fresh profile and is unaffected. This is not a smoke-test pass for the flow above.

## §12.2 Next steps — history

*Step 1, M1 acceptance on macOS:* The full run passed on both a nondefault and the default port (above).

- **Windows reinstated (2026-10-16): the 2026-10-15 drop below is superseded (§14).** Windows CI comes first (`brwr-bzx`); interactive Windows checks are [human] work.
- **Windows dropped (2026-10-15), superseding the earlier "Windows parked" status.** macOS is the only supported platform; see §14. Windows CI was removed. The Windows smoke runner, its interactive run, its default-port check, and the registry-based native-host registration it would have needed are no longer planned work.

*Step 3, settling the M2 design:* The answers are in §14 and in §4.4, §5.2, §6, §7.1, §7.3, and §8. The last open question, Codex isolation, was answered on 2026-09-28: Codex runs with the user's normal home (§14).

*Step 4, the side panel rework:* Built the §5.7 layout with the controls M1 already has: the page area following the active tab, the autosaving note (§5.3) with the host's versioned replace (§4.4), notes on any HTTP(S) page started from the toolbar action, a keyboard command, or the context menu (§5.3), which brings `activeTab`, `contextMenus`, and a `note` entry under `commands` (§8) forward from M2, and the capture strip collapsed into a one-line disclosure. This replaces the create-only note flow and review drafts. Protocol version 3 replaces `create_page_note`/`page_note_result` with `load_note` (returns `exists`, a `revision`, `title`, `body`) and `save_note` (takes `expected_revision`; a match replaces the note whole and returns `note_saved`, a stale revision returns `note_conflict` with the note's current content instead of writing anything). The host reuses `get_config`/`update_config`'s revision-check pattern; before replacing, it checks the existing file's recorded canonical URL and `brauser:` ownership (kind absent or `page`, never `summary`), never adopting a mismatch. The answer card, Summary section, and full omnibar remain M2/M3 and are not built; the omnibar's one-line input and the `/summarize`, `/related`, and `Read later` surfaces have no placeholder yet beyond the capture strip. The headless smoke run (`npm run smoke:macos:headless`) now exercises autosave (typing, then the debounce save; a save flushed by navigating the page away and back) and the refused-save path (a simulated second window saves first, so the panel's own save is refused, shows the current note, and keeps the unsaved text in a copy-out box; a later edit then saves cleanly against the now-current revision) in place of the removed create-only/review-draft checks. Not yet verified against real Chrome: whether the toolbar action alone (via `openPanelOnActionClick`, with no `action.onClicked` handler) grants `activeTab` the way the `note` command and context-menu item do — the same open question M2's build order already flags for `summarize` (§12.2 step 8.4).

*Step 5, M2 step 1 (harness setup):* The host offers harnesses found on `PATH` and shows a native confirmation of the binary, arguments, environment variable names, the denylist review (§7.3), and the proposed `summaries_dir` (§6.1). As with `storage.root`, the extension never supplies a raw binary path; `update_config` requires a single-use token bound to the confirmed harness entry and denylist, and that token sets `agent_denylist_confirmed`. The same setup records the harness version and identity, checks every flag against `--help` (and `codex features list`), runs the one-token probe, and refuses a harness that fails (§7.1). Add optional `summaries_dir` and the privacy keys to the protocol `StorageConfig`/config schema, validation, and overlap checks, with a protocol version bump. Tests cover the host refusing agent requests while the denylist is unconfirmed and `update_config` rejecting an extension attempt to set it. See the as-built notes in §7.1. macOS CI passes, as does the headless smoke run; the smoke run covers it end to end with fake harnesses on a pinned `PATH` (detection, including a fake Codex passing its checks, a canceled setup, a failing probe, a confirmed setup, denylist add and removal, a confirmed Codex setup whose dialog carries the §14 disclosure and whose probe runs the reviewed argv with only `HOME` and `PATH`, and a denylist edit after the harness is removed). Not yet verified: the real macOS confirmation dialog (guided or `--auto` run), and setup against real `claude` or `codex` binaries. Where a Chrome-spawned host gets auth values is still open (§14).

*Step 6, building M1.5a:* Record keys (step 6.7) must land before M2 `/summarize` (step 8.5) writes any summary. M1.5a's schema changes (the adapter config and lease in step 6.3, activity and `why` events in step 6.4, the config change in step 6.5, and `get_artifact`, `list_artifacts`, `worklog`, and record keys in step 6.7) ship together as protocol v5; any later schema change, including M2 step 8.3 streaming, takes the next version. It gives the product a clear first-release reason to exist ("Brauser remembers the tickets and docs you work on") without an agent. It ships with three starter adapters for tools the user opens daily (Jira, SharePoint/OneDrive, and GitHub), so the log is exercised on real work from the first day.

*Step 6.1, probe results:*

- **As built (2026-09-28).** `npm run probe:m15a` (`scripts/probe-m15a.mjs`, with the probes in `scripts/probes/`) runs every group in headless Chrome on a fresh temporary profile, nothing on screen, with a recorder extension that has Brauser's worker permissions (no `tabs`) and a grant for the fixture origin only; `--only <probe>` runs one group, `--json <file>` writes the report, and `--chrome <path>` runs another Chrome build. Results on Chrome 154.0.8037.57 were the same in two full runs and an isolated rerun. Where a result contradicted this document, the design text above was corrected and the probe now expects the corrected behavior, so the suite is a regression guard.
  - **New tabs.** `onCreatedNavigationTarget` fired once, with the new `tabId` and the right `sourceTabId`, for a middle-click, Meta- and Shift-click, `target=_blank` with and without `noopener`, `window.open`, and a popup. It always arrived before the new tab's `onBeforeNavigate` and commit, and before the source tab's next navigation or `pushState`, even when the source moved on in the same tick. The new tab's first commit was `link`, never `reload`, and a browser-opened tab fired no event. The event also carries a target URL on an ungranted origin (§5.1 Trails, corrected). `tabs.onCreated` carried `openerTabId` without `tabs`.
  - **Transitions.** A click commits as `link`, a typed URL as `typed`, a form as `form_submit`, a bookmark as `auto_bookmark`, and a browser reload as `reload`. Every board-modal `?selectedIssue=` `pushState` or `replaceState` fired `onHistoryStateUpdated` as `link` with no qualifiers, whether clicked, scripted after activation expired, or scripted in a document never activated. Back and forward, same- or cross-document, from the page or the browser, and from the back/forward cache or not, keep the entry's original type and add `forward_back` (§5.1 Trails, corrected). A title set before `pushState` reaches the worker ahead of `onHistoryStateUpdated` (§5.1 M1 mechanics, corrected). A page's own `location.reload()` commits as `link` with `client_redirect`. `onCommitted` reports URLs on ungranted origins; `tabs.onUpdated` does not. The recorder, with `webNavigation` and no `tabs`, shows exactly one install warning, "Read your browsing history" (§8).
  - **Worker stop.** After a forced stop, and after the worker stopped by itself about 30 seconds after its last event, `storage.session` (the tab map) and a pending `storage.local` total were unchanged and an in-memory global was gone. The navigation that woke the worker reached the new worker in full. The `storage.local` total also survived a browser relaunch.
  - **Idle.** `setDetectionInterval(300)` and `(15)` are accepted; values below 15, non-integers, and strings throw instead of being clamped, `queryState` has the same floor, and there is no getter (§5.1 Engagement, clarified). A restarted worker can set 300 again. An extension with only `idle` shows no install warning.
  - **Discard and restore.** Switching to a discarded tab, by `chrome.tabs.update` or by activating its target, and reopening a closed tab with `chrome.sessions.restore` commit as `reload` with no qualifiers, as do all four tabs of a `--restore-last-session` relaunch, including a `?selectedIssue=` URL. A discard gives the tab a new ID, reported by `tabs.onReplaced` and `webNavigation.onTabReplaced` at the discard, and the switch's `tabs.onActivated` arrives with the new ID before the reload commit (§5.1 Opens, corrected). A reopened tab also gets a new ID and fires no `onCreatedNavigationTarget`. A worker loaded after startup missed the active restored tab's commit, which Chrome requested about 300 to 470 ms before the worker was running, and saw the background tabs' later commits.
  - **Restore order (2026-09-28).** Branded Chrome 154 ignores `--load-extension`, even with `--disable-features=DisableLoadExtensionCommandLineSwitch`, so the `restore-order` probe runs on Chrome for Testing 154.0.8037.57 (`--chrome`). That build honors the switch, and the runner adds `--use-mock-keychain` so an ad hoc signed build does not stall on the login keychain. The recorder is named with `--load-extension` on every launch; the probe restores four tabs (three granted, one not) with a different active tab each time. Across three probe runs, 15 free relaunches and 9 held ones:
    - Chrome treats a `--load-extension` extension as a fresh install at every launch: `runtime.onInstalled` fires with reason `install` and `runtime.onStartup` never fires. So this is not an installed extension, which Chrome loads from prefs with the lazy listeners from earlier sessions.
    - Free relaunches: the active tab asked the fixture 320 to 1,012 ms after launch, and the worker first started 424 to 1,302 ms after launch, always later, for `onInstalled`. The active tab's commit was missed in 15 of 15; every background tab (about 2.3 to 4.1 s) and the ungranted tab were logged as `reload`.
    - Held relaunches: the fixture parked every request until the worker had run for `onInstalled` and been stopped. On release, every restored tab's `reload` commit, the active tab's included, reached a new worker started for it (9 of 9; its first event was the tab's `tabs.onUpdated`, then the commit).
    - Without `tabs`, `webNavigation.getFrame` and `getAllFrames` return a restored tab's main-frame URL, and also the ungranted tab's URL, which `chrome.tabs.get` hides.
    - So an extension that Chrome has loaded, with its listeners, before a restored tab commits gets the commit, even with its worker stopped; one loaded during startup misses the active tab. Which of these an installed extension is depends on whether Chrome loads installed extensions before it restores the session, which the probe cannot show without a real install. §5.1 (Opens) rebuilds missing map entries at worker start, so the outcome does not matter (decided 2026-09-28).
  - **Not proven headless.** No supported headless method installs the recorder persistently: `Extensions.loadUnpacked` lasts one browser session, `--load-extension` reinstalls at each launch, and a CDP drag of the folder onto `chrome://extensions` does nothing. Faking an install through Preferences, external-extension files, or policy is ruled out. So whether Chrome loads an installed extension before the first restored commit, and `runtime.onStartup`, are untested (restore order, above). Also untested: real idle and locked transitions (the state is the whole machine's, which the probe must not drive, so it only saw `active` and does not wait 5 minutes); Chrome's own memory-saver discard (the probe uses `chrome.tabs.discard`); the toolbar Back button, which skips entries added without user activation (CDP has no GoBack); omnibox and bookmark-bar navigations, simulated with CDP transition types; and whether `storage.session` is cleared by a browser restart, because the relaunch also reloads the unpacked recorder, which clears it anyway.

*Step 6.2, the adapter-template engine:* The spec was accepted on 2026-09-28. Unread-count prefixes are left to each adapter's title entries (spec §9.1), and the remaining open questions (§13) do not change the grammar, so they can wait. Two independent implementations of the same grammar, neither using a regular-expression engine: a Rust crate added to the Cargo workspace (`adapter-templates/`) for the host, and a TypeScript module in `extension/` for the worker. Each covers parsing and validation (spec §4.1, §5, §6, §7, §8, §9, §10.3 limits), URL preparation (§5.1), path matching (§4.3), query conditions and modes (§5.3, §5.4), scope (§6), capture types and `@tail` (§7), `id` building (§8), title templates (§9), and refs (§10.1). The host crate also implements the conservative overlap check (§10.3) and the Tier 2 draft structure rules (§10.4), and the worker module needs only what the prefilter uses. A `brauser adapters eval` CLI subcommand evaluates a template against a URL and title for tests. Tests follow spec §12: shared vectors in `/protocol/adapter-templates/vectors.json` run by both implementations in CI, with the spec's §11 starter examples among them; differential property tests between the CLI and a Node harness, with near-miss inputs; `cargo-fuzz` targets for the parser, URL preparation, and title matching; bounds tests at every cap within a fixed time budget in both engines; and scope tests for each shared origin. They also cover a URL with `%2F`, `.`, or `..` in a segment, a title capture that disagrees with its path capture, a captured alternation containing `:`, and a scope value no URL could produce.

*Step 6.3, adapter config, lease, and settings review:* Protocol v5 carries the `[[artifacts]]` schema and `[adapters] format = 1` through wire and disk, validates it with scope and the shared-origin list, reports overlap warnings, and puts widening changes behind revision-bound native confirmation (§7.2). Unsupported formats suspend only adapter policy without rewriting the document; origins outside the site allowlist are refused; unscoped shared origins carry the every-account statement; and existing adapter IDs are frozen until migrations exist. Removed recognition/ID contracts stay in host-owned config history, so neither removing and re-adding an adapter nor changing its type at the same time can evade that freeze; an exact-ID restoration and a non-overlapping new route remain valid. Every lease now carries the confirmed templates. The worker validates and compiles them with the step 6.2 TypeScript engine before buffering, and the host independently re-evaluates delivered original URLs and titles with the Rust engine, returning `suppressed` / `not_an_artifact` for every miss. The host keeps that validated Rust document with the adopted configuration revision and reuses it for all visit and activity evidence checks; ordinary disk refreshes retain it when the revision is unchanged, and the pairwise overlap report runs only for `get_config` or immediately after a successful commit. Tests cover unrecognized, out-of-scope, and ungranted navigations at the worker boundary; a compromised worker delivering rule and scope misses; recognized traffic; lease renewal; repeated native refresh and evaluation without recompilation or overlap scans; and grant revocation narrowing both sites and templates without poisoning the next worker wake. Settings now provides the config editor and evidence described in §7.2: readable policy cards, named overlap warnings, local full-document validation, precedence controls, an example URL/title match preview, fixed non-artifact probes, and the sync caution before the native confirmation. URL-shape samples and re-map counts remain part of M1.5b's Create adapter flow.

*Step 6.4, activity events:* Automatic capture now keeps offset-qualified local-hour totals in trusted `storage.local` and the tab/artifact, pending-source, active-window, focus, and idle context in `storage.session`. Cold opens, focused and background milliseconds, modes (each with its evidence URL), refs, and capped trails freeze at the hour boundary into stable-ID `activity` events; the same protocol version also carries append-only `why` events. The worker retains and drains older M1 visits after upgrade, but new navigation capture no longer creates visit rows. `idle` is a required no-warning permission and is set to five minutes. The host independently re-evaluates the activity artifact, each claimed mode, every trail source URL, and title refs before an idempotent measured Markdown append; neither activity nor why uses M1 near-repeat suppression. Each human-readable line's existing owned marker now adds `kind=activity|why` and a hex-encoded canonical wire payload, keeping the append-only log sufficient to rebuild modes, trails, refs, and why text in step 6.6 without trusting the prose rendering; the primary, mode-evidence, and trail URLs are normalized only after authorization, and M1 visit markers remain valid. Tests cover hour and UTC-offset boundaries (including the repeated daylight-saving hour), restored pending totals, focus/background/idle/lock accounting, reloads and restored/discarded tab replacement, same-artifact routes and duplicate tabs, forward/back and created-target trails, SPA title timing, worker-start map repair for active/loading/ungranted tabs, legacy-queue delivery, host identity/mode/trail tampering, and why-event replay. The remaining browser-order and real idle/lock limitations stay those recorded by the step 6.1 probes and `TESTING.md`.

*Step 6.5, delivery and durability:* The worker freezes up to 64 ready records from one local calendar day under a stable batch ID, persists the batch's exact overflow snapshot across retries, and applies terminal acknowledgements together. The host validates policy before vault I/O and publishes all surviving entries with one compact in-flight intent, one combined append, and one log sync; a replay completes only the exact intended suffix or recognizes complete event markers, and the bounded index replaces current delivery's one-file-per-event journal while legacy `record_visit` remains for upgrade draining. At 80% of either queue cap the toolbar badge lights, duplicate undispatched hourly activity coalesces only when the result still fits the byte cap (title refs come only from the event supplying the retained title, deduplicated, so an older title cannot leave unverifiable evidence), and dropped-hour count and range remain in the panel until an idempotent overflow marker is durably acknowledged. `near_repeat_secs` is absent from protocol v5 and new config writes, accepted and ignored when loading M1 config, and removed from consent and log scanning, so two deliberate `/log` actions a minute apart both persist. Tests cover batching and replay, overflow acknowledgement loss with concurrent drops, hard-cap coalescing, malformed compact-index bounds, and the two manual entries.

*Step 6.6, index tables and artifact files:* Each durably accepted activity updates an atomically replaced, disposable SQLite `index.db` with `artifacts`, `visits`, `edges`, and `dirty_artifacts`; a bounded host cache avoids reparsing all prior logs on every delivery while leaving owned machine markers authoritative. Duplicate retries and upgrade-era visits reconcile from those markers instead of trusting the incoming retry body, conflicting reuse of an event ID is refused, and the explicit reindex shares the host's configuration/write lock. Connection close regenerates dirty artifact Markdown through capability-scoped, exact ownership-checked replacement, preserves mtimes when bytes are unchanged, neutralizes presentation text, and rolls old daily activity into chronologically ordered weekly then monthly Timeline rows. Case-colliding IDs receive distinct hash-suffixed paths. `brauser reindex` discards the cache, reparses bounded owned logs, and recreates both database and files. Older configs choose a non-overlapping artifact directory when `artifacts` is already occupied. The settings-facing manual trigger remains with step 6.7. Tests cover a hostile title (embeds and YAML breakers), a case-colliding ID, an external edit to an artifact file being replaced, a day of heavy use producing no file write until regeneration, standard SQLite reads across multiple B-tree pages, malformed-marker and conflicting-ID isolation, offset chronology, passive carry-over activity, legacy delivery, config migration, and a year of daily opens staying within the rollup bounds.

*Step 6.7, panel, worklog, and record keys:* The panel shows the current artifact's identity, mode, measured focus/background/open totals, links, and append-only one-line context; Recent groups artifact-day rows across tools with type/title filters, and the host generates copyable today, yesterday, or current-week worklogs without an agent. `get_artifact`, `list_artifacts`, `worklog`, and the settings `refresh_artifacts` action read the disposable index under host-side bounds, with regeneration conflicts and paths visible instead of overwritten. `load_note` and `save_note` now resolve and return a host-derived URL or artifact key, stale keys fail as `key_changed`, alternate routes share one note, and guarded **Use as artifact note** migration keeps older URL-keyed notes findable. When no adapter currently recognizes a URL (its adapter or last enabled site was removed), the recorded contracts in config.toml decide its key (§6.2; the index fallback this step first used was withdrawn on 2026-10-27). A new `view_older_note` message opens an older URL-keyed note read-only, by its own key, so notes beyond the one a sibling migration already moved into the shared note stay reachable even though their own URL now resolves to it. Tests cover the Google Doc, Jira modal, mid-edit adapter change, occupied artifact note, unchanged M1 note, message bounds, recent/worklog derivation, conflict reporting, removed-adapter note recovery, context saved before the first activity, the older-note candidate cap, and read-only older-note viewing (including its own authorization check) cases named for this step.

**Protocol versions (corrected 2026-10-27):** this step once said that new M1.5b messages take the next protocol version. Capture Site first added `draft_site` and `check_site_draft` additively within version 6, following the agent-run precedent in protocol/README.md ("Also added in version 6").

*Step 7.6, as built:* The panel reviews and enables the known Jira, SharePoint/OneDrive and separately scoped GitHub starters without JSON; `brwr-b39` builds private URL shaping, bounded worker samples, click-only link shapes and a read-only panel/settings review; and `brwr-t40` adds the host-owned catalog, the Rust shape parser, the per-rule checks with pending scope, the structural drafter, `draft_site`/`check_site_draft`, the panel's plain-language review and revision-checked save, the advanced-editor handoff, the recurring-pages hint, and a headless smoke step (§5.1). Scope, `unscoped` and the title entry are panel-owned overlays applied after `check_site_draft`; the checked tables hold only Tier 0 keys and the scope choice travels as a separate field. The text below is the original step; open access (protocol v6) supersedes parts of it as follows, and nothing in the left column was built:

| Superseded requirement | Open-access equivalent built |
|---|---|
| Ungranted page starts Create adapter from the context menu or the toolbar action, using `activeTab` | No ungranted pages exist. The panel's **Capture Site** button shows on any unconfigured HTTP(S) tab; no context-menu entry, and the toolbar action stays "open panel" |
| Clicking requests the exact origin from Chrome; `create-adapter` grant purpose with a 30-minute expiry | No `chrome.permissions` call and no grant purposes. Only the advanced-editor handoff keeps a 30-minute `storage.session` entry |
| A canceled or expired flow removes the newly granted origin, keeping it when a note was taken there | Cancel, close, tab switch, navigation or a stale revision discards the draft and its link shapes and sends no `update_config`; notes never depended on the draft |
| Hand-over to the settings page's host through `storage.session` | The panel's own host connection drafts inline; the settings handoff exists only for the advanced editor |
| Settings reads the tab title on the now-granted origin and shows it only in the review | The panel reads the active tab title and shows it only in the review; it never goes to `draft_site`/`check_site_draft`, and a title entry the user types is part of the saved adapter |
| A final native confirmation names the origin and repeats the visited and link match counts | The review names the origin and shows "Matches N of M visited patterns and K of L link patterns"; the save is `check_site_draft` plus revision-checked `update_config` under the config mutation lock |
| The origin is acceptable only because the native confirmation names it | The host drafts only for the request's exact origin and never adds one; user-written config is trusted (§4.2) |
| The agent denylist applies to the origin | Removed in v6; no agent runs |
| Enabling or widening needs the native confirmation (§7.2, adapter-templates.md §6) | The review must be shown and saved explicitly, and `unscoped` needs its own checkbox with the statement |
| Flow test starting from the context menu on an ungranted tab | Starting from the panel on an unknown tab with no permission request |
| Flow test of a canceled or expired flow removing the newly granted origin | Cancel or expiry saves nothing and leaves notes untouched; an expired advanced-editor handoff is deleted |

*Step 7.6, the original step:* The Notion named case below is covered by the `brwr-gf1` decision as revisited on 2026-10-27 (§5.1, §14): behind a route word on a non-shared origin, title-prefixed and bare `hex(32)` pages share one `hex(32)@tail` ID (`title_prefixed_and_bare_hex_pages_share_one_tail_id_behind_a_route_word`), and Notion workspace pages share one behind the scoped workspace, their anchor (`notion_workspace_pages_draft_behind_the_scoped_workspace`). The shared-origin flow test below is superseded by John's 2026-10-27 decision (§14): the tenant is pre-filled from the tab's URL as "Capture for `<tenant>`" and confirmed with one click on Save (`a shared origin pre-fills this tab's tenant as Capture for <tenant> and saves with one click`); Save stays disabled only when the URL names no tenant. Original step: Create adapter with the structural drafter (§5.1), after Tier 1 (step 7.3) brings `scripting`; no agent. This step adds the URL-shape sample and the recurring-pages hint, the panel button and the "Create adapter for this site" context-menu entry, the link-shape function, the host's structural drafter, and the review in the settings page's adapter editor. Shaper tests feed real URL forms from every shipped starter adapter, plus Confluence `/display/SPACE/Title`, SharePoint `sourcedoc` GUIDs, Notion `Title-<32 hex>` pages, percent-encoded names, and long mixed-case slugs, and check that no stored shape contains a digit outside a placeholder name, an uppercase letter, a segment over 20 characters, a query value, or any title. Link-shape tests check that the function runs only in the top frame of the document the panel saw, rechecks the origin, reads only same-origin `href`s, stops at 500 links, turns a literal seen in only one link into `{slug}`, returns at most 50 shapes and no raw URL, and that nothing it returns is stored, sampled, or delivered. Drafter tests feed the shapes of every shipped starter adapter's recorded URLs and check that the draft matches those URLs with one adapter per kind of page, unique types, and no `web` scheme. Named cases: a GitHub pull request and its `/files` tab become one adapter keyed by owner, repository, and number; Notion pages with and without a title slug share one `hex(32)@tail` ID; SharePoint's `sourcedoc` GUID, not `15`, becomes the ID; and Google Docs' `/u/0/` never enters an ID. They also feed planted links steering toward `/{*}`, a root match, a search page, an auth or capability route, an ID from a search or paging parameter, an ID-less rule, or a new alternative on a visited rule, and check that each is dropped, or kept in a separate unchecked rule, before the review. A `token`-only rule starts unchecked, and a draft overlapping an existing adapter loses only the overlapping rules. Flow tests cover starting from the context menu on an ungranted tab, a canceled or expired flow removing the newly granted origin but keeping it when a note was taken there meanwhile, a link-shape run that returns nothing after the tab navigated, a shared origin whose tenant position becomes a capture with scope pending and Enable disabled until the user chooses (superseded 2026-10-27: pre-filled and confirmed, above), a draft unchecked down to one rule, and a user edit turning only the edited rule into user-written config whose draft-rule failures only warn.

*Step 7.7:* "The same harness isolation" below is superseded by open access: the agent is the user's full-access harness run through the Brauser service (open-access spec §6), the agent denylist no longer exists (removed in v6), and the safety boundary is the host-built input plus the host's judgment of the answer. Original step: Agent drafting, after M2 host invocation (step 8.2), using the same harness isolation, for the hint and for **Improve with AI** in the review.

*Step 8, the rest of M2 as planned before open access:*

2. Host invocation: an environment of only the allowlisted variables, argv from the template, page content on stdin inside the untrusted-data block, the host-owned working directory, a timeout, output cap, and process-tree kill, and the confirmed-install-location prefix check (§7.1). Test with fake harness binaries on both OSes, including one whose symlink target changes between runs (identity re-check), one moved outside the confirmed install location, one that emits a tool call or an unknown Codex event, and one that reports `is_error`. Run the §7.1 canary for each shipped harness template, including a prompt-injected page that asks Codex to read a planted file; it passes if the contents appear in no forwarded chunk or summary and any non-allowlisted event ends the run as `rejected`.

3. Streaming protocol: the host currently answers one request at a time, so it needs a reader that stays responsive while a harness runs. Add chunk, `rejected`, error, `cancel`, and `summary_result` messages correlated by `request_id`, each under the host's 900 KiB outbound cap (§4.3). Closing the panel (stdin EOF) kills the harness.

4. Content extraction with a bundled, pinned Readability-style library, the `summarize` command, context menu, and toolbar trigger, and the permissions in §8 (`scripting` is already present from M1.5b). First confirm with the real-Chrome smoke runner that (a) the command, context-menu, and toolbar-action paths grant `activeTab` to `chrome.scripting.executeScript`, and whether opening the panel from the action (`openPanelOnActionClick`) grants it; (b) the grant ends when the tab navigates; (c) the panel sees no URL for an ungranted tab; (d) the grant does not carry over to another tab in the same window.

5. `/summarize` in the panel, with denylist checks and the summary write and result contract in §5.2 and §4.4. Set the §3.1 extension-page CSP, with a test that the built manifest carries it. Test an external-editor race, a shared or overlapping summaries directory, and a config change during a fake-harness run, which must discard the output.

*Step 10.2, PR 2:* The `brauser` binary gains a `brauser service` subcommand, registered for development as a per-user LaunchAgent (`com.rauser.brauser.service`) by `scripts/register-dev-host.mjs`, which replaces it on every re-registration and removes it with `--unregister-service`. PR 2 registers no real harness: only the test-only fake harness runs, so a normal build's service answers `start_run` with `not_configured` ("No AI harness is set up for summaries.") until PR 3.

*Step 10.3, PR 3:* The service registers the real runners: Codex as a `codex app-server` JSON-RPC thread with `approvalPolicy: "never"`, `danger-full-access`, and the answer schema as `outputSchema`, and Claude Code as `claude -p` stream-json with `--dangerously-skip-permissions` and a resumable session (§7.1). Runs work in the knowledge base folder, whose recorded identity is checked first, with the service's environment plus `agent.env` and the configured per-run timeout. The §6.4 prompt names `page.md` by path, marks it untrusted, and lists the editable `summary_vocabulary` (a new required `ConfigSnapshot` field; v5 files load the defaults); the answer is validated with one repair turn (`host/src/answer.rs`), and `RunView.answer` carries its type, category, and importance. Harness setup moved into the service (detect on its `PATH`, probe through the real runner, commit `[summarizer]`), and the lockdown (argument templates, `--help` and feature checks, the emptied environment and allowlist, host-owned work directories) and `host/src/harness_setup.rs` are deleted. `open_path` opens a Codex thread or reveals `agent.env`; `get_agent_env` lists its names. In the extension: the settings "Summary harness" section, pinned Readability 0.6.0 with a byte-capped extractor, **Summarize & log** with the `summarize` command and context-menu item, and the polling run card restored from `list_runs` (§5.2). Nothing was written to the home folder until PR 4. The headless smoke run covers setup, Codex and Claude Code runs, a repair turn, a second invalid answer, a failing harness, Open in Codex, Cancel, restore, and `agent.env` names against fake `codex` and `claude` scripts.

*Step 10.4, PR 4:* The service's `SummaryFinisher` saves a validated answer under the configuration current at finish, following a key changed mid-run, with the answer's `answered_at` stamp (re-stamped when the daily log would no longer accept it) as the event time in the click's `requested_at` offset, so every Retry save builds the same file and log line; failures are plain reasons that end in what to do (§5.2 step 7). The summary file (§6.6) is rendered from the validated answer, written through the ownership-checked replace shared with artifact files, and named after the page note's stem; its body text goes through the context-free neutralizer (§5.2 step 6), checked by `npm run test:markdown-neutralizer` against markdown-it, micromark and marked in CI, and its front matter is JSON-quoted with no entities. The `summary` log event now carries `record_key`, `artifact_type`, and `harness`, links the file by a percent-encoded relative path, and attaches to an adapter's artifact on a recognized page or a `web.page` artifact otherwise. The index cache moves to format 6 (a format-5 cache migrates in place); summaries never count as opens or opened days (`brwr-hnr`), artifact files gain a Summary line and date-ordered Timeline summary lines (§6.7), Recent rows and `get_artifact` carry a nullable `SummaryView` (on an unrecognized page too, quietly null when the lookup fails), and the worklog names summaries. `open_summary` opens only an owned summary file with PR 3's desktop opener: the one at the `summary_path` a Recent row, summary line, or run names, else the page's record's. The panel shows the page's saved summary, summarized Recent rows, and the run card's saved-summary line with one retry, each with **Open summary** (§5.7). The headless smoke run covers the outputs end to end against the fake harnesses (plan Task 53): the summary file's identity, plain front matter and inert agent text, the log event, `list_artifacts`, the worklog, artifact files, Retry save over an unowned file, `reindex`, and **Open summary**.

## §14 Open Questions — history

Capture Site drafting decisions (2026-10-27, `brwr-t40`; plan `docs/superpowers/plans/2026-10-27-capture-site-unknown-sites.md`):

- **`brwr-gf1`: the leading-literal rule stood, then the scoped tenant became the anchor (revisited 2026-10-27).** First decided: Notion workspace pages (`/acme/Release-Plan-<hex32>`, `/acme/<hex32>`) have only the host's `workspace` tenant capture, a `name`, before the ID, and were refused as `draft_no_leading_literal`. Revisited after #126 made a scope-restricted capture an anchor: the drafter and the per-rule checks adopt that rule instead of keeping a tenant rule of their own, so these pages draft to one `notion.page` `hex(32)@tail` rule behind the scoped workspace, which can never be saved `unscoped` (§5.1). Step 7.6's Notion success case holds on Notion itself again, as well as behind a route word on a non-shared origin.
- **Shared-origin tenants are pre-filled and confirmed (2026-10-27, John: "make adapter creation as easy as possible and do the work on our side").** On a shared origin with a tenant position, the drafted review pre-fills the tenant from the current tab's URL as "Capture for `<tenant>`", already chosen, so Save is a one-click confirmation; it stays editable and keeps every safety check. This supersedes "scope pending, Enable disabled until the user chooses" (§5.1, §12.2 step 7.6), which now applies only when the URL names no tenant.
- **Additive messages stay within the version.** `draft_site` and `check_site_draft` were host-only additions within protocol v6, and moved onto version 7 with #126's containers; a changed existing message still takes the next version (§12.2 step 7).
- **Scope, `unscoped` and title are overlays.** They are applied by the panel after `check_site_draft`, never inside the checked tables, so the forbidden-key check stays a whole-draft refusal for drafted rules (§5.1).

Home folder decision (2026-10-27, John; supersedes the configurable content locations of §6.1 and PR #72's per-folder settings):

- **One home folder, fixed layout.** The user picks one home folder with the native picker; Brauser creates `notes/`, `logs/`, `artifacts/`, `summaries/`, and `later/` inside it, with no per-folder settings. Machine state (config.toml, index, intents, locks, harness work) stays in the OS config and cache directories; no bootstrap pointer is needed. The knowledge base folder stays a separate pick.
- **Rename once.** An existing install's older layout is moved automatically, no-clobber, under every writer's lock, and refused with a repair message when a fixed name is already taken or the layout is unusual (§6.1).

*Product decisions, “Users never see adapters”, before open access trimmed it:* The primary way to start capturing a site that no starter adapter covers is a **Capture Site** button in the side panel (plus the context-menu and toolbar entry points for an ungranted tab). The native confirmation remains the trust gate, but it is written in plain language: what Brauser will remember on this site, with example pages it matched, and what it will not capture, using the existing match preview and fixed safety probes.

*Product decisions (2026-10-16), superseded sequencing (its harness-auth precondition closed on 2026-10-26):*

- **Sequencing.** M1.5a steps 6.6–6.7 (worklog view first), then Capture Site and re-keying, then M2 with agent-refined Capture Site right after host invocation. The harness-auth question below must be decided before step 8.2. The goal is to reach daily use of M1.5a quickly.

Platform decision (2026-10-15, superseded 2026-10-16 above):

- **macOS only.** Brauser targets Google Chrome Stable on macOS and no other operating system; Windows support, previously planned alongside macOS and partially built (Windows dialog/COM-apartment handling in the picker flow, `MOVEFILE_REPLACE_EXISTING` summary-file replace, registry-based native-host registration, a Windows CI job, an M5 Windows installer and smoke run), is dropped rather than carried as unvalidated scope. Windows support had already stalled at "parked" since 2026-09-28 (§12.1, §12.2) for lack of a Windows machine to run its interactive checks on, so this makes the actual, long-standing state of things the declared one rather than a nominal commitment nothing was testing. §3.1, §9, §10, §11, §12, and §13 are updated accordingly; the vestigial Windows-only code paths in `host/src/` (noted at each site above) are not yet removed — that is separate follow-up cleanup, not done as part of this decision.

*From the 2026-09-28 artifact decisions:*

- **Sequencing.** M2 step 1 (harness setup, protocol v4) finishes first, then M1.5a (protocol v5), then M2 steps 2–5 (§12.2 step 8). M1.5a opens with the adapter-template engine (step 6.2, built to adapter-templates.md), which changes no protocol. Harness setup merged in PR #19 (2026-09-28), and M1.5a needs no agent. M1.5b follows once M1.5a has been in daily use for a few weeks and can run alongside M2 steps 2–5; Create adapter (step 7.6) needs no agent, and agent drafting (step 7.7) waits for M2 host invocation (step 8.2). `scripting` arrives with Tier 1 in M1.5b (step 7.3), and M2 content extraction (step 8.4) reuses it.
- **M1.5 is split.** M1.5a is Tier 0 with the lease prefilter, activity totals, batch intents, record keys (which must land before summaries exist), the artifact card, and Recent, with three starter adapters the user opens daily (Jira, SharePoint/OneDrive, and GitHub). M1.5b is aliases, Tier 1, worklog files, the remaining starter adapters, and Create adapter with the structural drafter. Moved issues and renamed repositories are rare, so aliases can wait until the log has been lived with for a few weeks.
- **Artifacts replace per-site logging.** Automatic capture records only URLs an enabled adapter recognizes as an artifact (§5.1, §7.2). Per-site "log every page" is not offered alongside it. `/log` remains the explicit one-off exception.
- **Log is the source of truth; artifact files are derived.** The daily log stays append-only. Artifact files and the index tables are rebuildable from it, so replacing an artifact file whole is safe under the same ownership rules as summaries (§4.4).
- **Measured, not inferred.** Attention is recorded as focused and background minutes, with no "glanced" or "worked on" labels. The idle detection interval is 5 minutes, so a long read without input still counts (§5.1).
- **Recognition in the worker.** The policy lease carries the confirmed adapter rules and Tier 1 selectors, so the worker prefilters and reads fields while the page is live. The host re-checks every event, so the worker can only narrow what is recorded (§3.2).
- **Opens are cold opens.** An open is a navigation that makes a tab show an artifact it was not showing. Tab switches, focus changes, and reloads (including Chrome's reload of a discarded tab) are not opens and are not logged; they only move focused and background time (§5.1).
- **Coarse log, lazy files.** The worker merges each artifact's activity per hour, the host writes one sync and one intent per delivery batch, and artifact files regenerate lazily with bounded Timelines (§5.1, §6.7).
- **No near-repeat window.** M1.5a retires `near_repeat_secs`. Hourly totals already bound artifact lines, and a `/log` entry is deliberate, so nothing is suppressed as a near repeat; replays are absorbed by event ID (§5.1).
- **Canonical ID plus aliases.** IDs change when issues move and repositories are renamed. Server redirects and declared identity signals record `alias` events, the index resolves them transitively, and `unalias` undoes a wrong one (§5.1).
- **Trails at creation time.** New-tab edges come from `onCreatedNavigationTarget` and the worker's tab-to-current-artifact map, not from reading the opener's URL later (§5.1).
- **Recent and the worklog view ship with M1.5a.** A cross-tool Recent list and a "what did I do yesterday" worklog view with Copy, so the milestone is useful without the M3/M4 queries; worklog files wait for M1.5b (§5.1).
- **Mode, not edits.** The URL's mode is recorded as seen; Brauser does not claim the user edited something.
- **Record keys.** Every per-page record (notes, summaries, read-later entries) is keyed by `artifact_id` on a recognized page and by URL otherwise, derived by the host alone. URL-keyed notes stay in place and are listed on the artifact; nothing is migrated automatically (§6.2).
- **URL shapes, not URLs, for Tier 2.** The unrecognized-URL sample stores only URL shapes, with identifiers and title-like text replaced by typed placeholders and no titles, for example `/wiki/{num}/view`. That is enough to draft an adapter, keeps no record of what the user browsed, and removes the prompt-injection path through titles. The sample is on by default (§5.1).
- **Adapter templates, not regular expressions.** Adapters are written in the declarative language in [adapter-templates.md](../adapter-templates.md), format version 1, with no raw-regex escape hatch. It is linear in both engines by construction, readable by a reviewer, and lets the host check draft structure and overlap mechanically, which replaces the two-engine regex dialect, its host-side rewrite, and most of the Tier 2 regex rejection rules (§7.2, §5.1). The shipped adapters for common tools are now called **starter adapters**. Shared origins need a `scope` or an explicit `unscoped` acknowledgment (§7.2). Title and meta aliases are suggested until confirmed or corroborated by a redirect (§5.1).

- **Title prefixes belong to the adapter (2026-09-28).** Title preparation removes nothing. A tool that adds an unread-count prefix such as `(3) ` is handled by its adapter's ordered list of title entries, and every starter adapter documents its title forms and trade-offs on its own page (adapter-templates.md §9.1, §11.1).

- **First starter adapters (2026-09-28).** Jira, SharePoint/OneDrive, and GitHub ship first, in step 6.8. The other starter adapters wait for M1.5b (step 7.5).
- **Users create their own adapters (2026-09-28).** A **Create adapter** button in the side panel drafts an adapter from the site the user is on, from the shapes of its URL, of the links on the page, and of the origin's sample. It never fetches or crawls pages. A deterministic structural drafter in the host makes it work without an agent in M1.5b, and the agent can refine the draft after M2. It starts from the context menu or the toolbar action on an ungranted tab, because a side-panel click grants no `activeTab`. It proposes one adapter per kind of page, checks each rule separately, and starts rules seen only in links unchecked. Only the rules the user edits become user-written. The user reviews every rule, the ID, and the scope before the usual native confirmation (§5.1).
- **Fixed-shape captures anchor a template (2026-09-28).** A path template needs a required anchor segment: a literal, an alternation, or a fixed-shape capture (`uuid`, or `hex` with an exact length of 32 or more). Optional segments never count. So Notion's all-capture URL shape is a valid starter adapter, while short or variable-length hex is not. Drafts still need a literal before their first identifier capture, or, since 2026-10-27, the scoped shared-origin tenant (adapter-templates.md §4.1, §10.4).
- **Scoped tenants also anchor a template (2026-10-27).** A required path capture a `scope` key restricts to a literal list counts as an anchor segment too, because the user's chosen tenant list is effectively a literal (adapter-templates.md §4.1, §6). This is what lets GitHub's repository catch-all, `/{owner:name|lower}/{repo:name|lower}/{*}`, validate once it is scoped to an organization list; it is a general rule for any shared origin with a tenant capture, such as a Notion or Linear workspace, not a GitHub exception. An `unscoped = "acknowledged"` adapter, or a capture scope does not restrict, still gets no anchor.

- **Rebuild the tab map at worker start (2026-09-28).** The step 6.1 probes showed that a worker loaded during startup misses the active restored tab's commit, and headless cannot show whether an installed extension loads in time. Rather than depend on Chrome's startup order, the worker rebuilds missing map entries on every start from `tabs.query` and `webNavigation.getFrame`, keeping only granted, recognized URLs and counting no open (§5.1 Opens). This also repairs the map after an extension update or reload.

*From the 2026-09-28 M2 decisions:*

- **Extraction permissions.** Any page can be summarized. A side-panel click does not grant `activeTab`: Chrome's documentation lists only the action click, context menus, keyboard commands, and the omnibox, and Chromium withholds tab permissions from the side panel. Without `tabs`, the panel also cannot see an ungranted tab's URL, so it has no origin to request. Summarizing an ungranted page therefore starts from a trigger that grants `activeTab`: the `summarize` keyboard command, the page context menu, or the toolbar action. The panel's button works when the origin is already granted or an `activeTab` grant is present. M2 adds `activeTab` and `contextMenus` and does not add `tabs` (§5.2, §8). This refines the earlier "prompt only if needed" answer: no Chrome prompt is needed, and no persistent grant is made for summarizing.
- **Summary destination.** A summary is a separate record from the page note: one Brauser-owned summary file per page, `brauser.kind: summary`, in the fixed `summaries/` folder, replaced whole on each new summary (user edits to it are replaced), linking to the page and its note (§5.2). The note holds only the user's own text. Because summaries never touch notes, M2 does not need the managed-block replacement in §6.4; the one replace primitive it needs, a checked rename limited to summary files, is in §4.4.
- **Harness modes.** Checked against the Claude Code 2.1.283 and codex-cli 0.144.4 documentation, help output, and trial runs (§7.1). Claude Code can run with no tools and is the recommended harness. Codex can run with its shell, web search, and user config disabled in a read-only sandbox, but documents no switch that removes every tool, so the host accepts only an allowlist of Codex event types and aborts on anything else.
- **Denylist setup.** Required once: the first harness setup's native confirmation shows the agent denylist with its suggested categories, and the host refuses every AI command until that confirmation sets `agent_denylist_confirmed`, even if the list is empty (§7.3).

*Codex isolation:* The Codex harness confirmation, the settings page, and the user documentation state this plainly. Tool use is still limited by the read-only sandbox, the disabled features, and the event allowlist (§7.1).

Open M2 question (2026-09-28; closed 2026-10-26 by `agent.env`, above):

- **Where a Chrome-spawned host gets auth values.** Chrome starts the host with the environment Chrome itself inherited, usually launchd's on macOS, not a login shell's. A Bedrock, Vertex, or API-key user's variables are then often absent, and setup can only report them as not set. Options: tell the user to set them where Chrome inherits them (for example `launchctl setenv`), read a host-owned env file the user edits, or let the harness use its own stored login where it has one. Undecided; step 1 passes through only names the host process already has.

*From the 2026-09-28 side panel decisions:*

- **Editable notes.** A page note is a scratch space the user can add to at any time, edited only in the side panel and autosaved as they type (§5.3). This replaces M1's create-only notes and review drafts. The host replaces the note whole and refuses a save based on an old version (§4.4).
- **No AI output in notes.** Commands send output to the sidebar, a new separate file, or the clipboard (§5.6). The `page_note` output is removed.
- **Notes on any page.** Notes work on any HTTP(S) page; the first note on a new site starts from the toolbar action, a keyboard command, or the context menu, because the panel cannot see an ungranted tab's URL; the panel then requests the site's origin from Chrome (§5.3).
- **`/log` on any page.** `/log` logs the current page now, even on a site not set up for logging, without enabling automatic logging there (§5.6).

*Side panel decision superseded by open access (no native confirmation exists for any change):*

- **No native confirmation for one-off actions.** Writing a note and running `/log` need only Chrome's site grant (§7). The trade-off: a compromised extension could write a note or log entry for any single page, where before it could do so only on sites the user confirmed. It still cannot enable automatic logging, change the home folder, or weaken privacy settings without the host's confirmation.

## AGENTS.md Current state — history

Moved from AGENTS.md's "Current state" on 2026-10-28 (`brwr-m68`), which is now a table. These notes describe each component as it was built, with code pointers; DESIGN.md holds the current rules. Six sentences that state invariants moved verbatim to AGENTS.md's Current state and are omitted here.

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

Unacknowledged note edits now have synchronous browser-local recovery (`extension/note-recovery.ts`, `brwr-3nq`). Every snapshot has an immutable key, and native acknowledgements/discards only remove the exact snapshot they saw. Reopening shows recovered copies beside the current host note with Copy/Discard; they never enter the editor or autosave automatically. Storage failures warn without preventing host saves.

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
an optional `agent.env` on each run's environment. The panel polls
`get_run`; there is no host-side streaming protocol.
`node scripts/register-dev-host.mjs` now also writes and bootstraps the dev
LaunchAgent; `--no-service` skips it and `--unregister-service` removes it.

Summarize & log (PR 3 of the open-access spec, DESIGN.md §12.2 step 10.3)
is merged (#98). Harness setup runs in the service:
`discover_harnesses` and `set_up_harness` are forwarded to it, it detects
`codex` and `claude` on its own `PATH` plus `$HOME/.local/bin` (an owned,
non-writable install and a parseable `--version`, with no version refused),
and it commits the chosen harness to `config.toml`'s `[summarizer]` table
(wire `ConfigSnapshot.agent`) only after a probe through the real runner
passes in the knowledge base folder. A v5 `[agent]`/`[harness_record]` file
loads as not set up. The registered runners are Codex as a `codex app-server
--listen stdio://` JSON-RPC thread (`approvalPolicy: "never"`, `sandbox:
"danger-full-access"`, and the answer schema as `outputSchema`) and Claude Code
as `claude -p --output-format stream-json --verbose --include-partial-messages
--dangerously-skip-permissions` with the prompt on stdin and a resumable
session. Both run with full access in the knowledge base folder, with the
service's environment plus `agent.env`, on the user's normal login. The §6.4
prompt names the run's `page.md` by absolute path, marks it untrusted, and
lists the editable `summary_vocabulary`; `host/src/answer.rs` validates the
final JSON (one repair turn, then `invalid_output`) and drops `related_notes`
outside the knowledge base; `RunView.answer` carries the validated type,
category, and importance. `open_path` opens a Codex thread (`codex://threads/<id>`) or
reveals `agent.env`, whose names (never values) `get_agent_env` lists. The
lockdown is gone: no argv templates, `--help` or feature checks, emptied
run environment, environment allowlist, or host-owned work directories. The
settings page detects, tests and chooses a harness, sets its timeout, shows
the setup warning, `agent.env` names, and the vocabulary. Every HTTP(S) page
gets **Summarize & log** in the panel, a `summarize` command
(Command+Shift+9 on macOS) and a "Summarize with Brauser" context-menu item:
the extension injects the pinned, hash-checked Readability 0.6.0
(`dist/vendor/readability.js`) and a self-contained extractor into every
frame (`allFrames: true`; `extension/summarize.ts` `choosePage` keeps the top
frame's text unless it is short and a subframe, such as Word for the web's
editor iframe, holds more; the title and address check stay top-frame), caps
the text in encoded bytes, and sends `start_run`. A polling run
card shows state, elapsed time, the streamed text as text only, Cancel,
Retry save, Copy, Open in Codex or Copy resume command, and the
type/category/importance line, and cards return from `list_runs` when the
panel reopens. The headless smoke run covers setup and runs against fake
`codex` and `claude` scripts.

Summary outputs (PR 4, DESIGN.md §12.2 step 10.4) are built on
`feat/summary-outputs-pr4`. The service's `SummaryFinisher`
(`host/src/service/finisher.rs`) saves a validated answer under the
configuration current at finish, holding the config lock: it rechecks the
home folder and layout, re-derives the record key and follows a changed one
(`written_record_key`), writes `summaries/<note stem>.summary.md`
(`host/src/summary.rs` `render_summary`, `Vault::write_summary`), then appends
a `summary` daily-log event whose marker carries `record_key`,
`artifact_type`, and `harness` and attaches to the adapter's artifact on a
recognized page or a `web.page` artifact otherwise. The event time is the
run's `answered_at` stamp in the offset of the click's `requested_at`, so a
Retry save rebuilds the same file and log line; the supervisor re-stamps an
answer the daily log's window no longer accepts, and failures are plain
reasons that say what to do. Summaries log while capture is paused. Agent
body text goes through `markdown::neutralize_remote`, a context-free escape
(`<` as `&lt;`, `\!` before `[`, and backticks, `$`, tabs, `~` runs of three
or more and code-length space runs as numeric entities), single-line fields
through `markdown::inline` or wikilinks, and front matter through
`markdown::yaml_string` with no entities; `npm run test:markdown-neutralizer`
renders the shared corpus with markdown-it, micromark and marked, in CI too.
The index cache is format 8 (`STATE_VERSION`; older formats 4–7 retain event
checkpoints and perform one bounded gap rescan, with unknown URL provenance
for old records); summaries never count as opens or opened days, artifact
files gain a Summary line and date-ordered
Timeline summary lines, and Recent rows,
`get_artifact` (an unrecognized page included, quietly null on a failed
lookup), and the worklog carry them. `open_summary` opens only the record's
owned summary file through the native host's `SetupContext.open` (PR 3's
opener). The panel shows the page's saved summary, summarized Recent rows,
and the run card's saved-summary line (one retried lookup), each with **Open
summary** through `ConfigSession.openSummary` or the card's host call.

Worklog gap review (`brwr-9r7`, 2026-10-28) uses additive protocol-v8
`worklog_report` / `worklog_reported`, leaving the original worklog pair intact.
The same caught-up snapshot supplies measured text and up to 32 owned overflow
notices (explicit truncation). Each full count/range is shown on every day/week
its recorded local endpoint dates enclose; overlapping notices stay separate,
retry IDs deduplicate, and missing records never become measured activity.
The panel always reviews notices and offers a checked-by-default Include
capture gaps choice; Copy uses the exact preview. Cache format 8 keeps system
notices separate from artifact events, with separate checkpoints so older
caches recover previously skipped overflow in one bounded rescan. Reindex
preserves notices; SQLite artifact tables are unchanged.

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
(`ConfigStore::note_key_rule`). Hand-removed contracts also take part in the
frozen-ID check (`validate_adapter_transition_with_history`, via
`retired_by_hand`). An unusable state file makes identity on every page the
current policy does not recognize unavailable (`NoteKeyRule::StateUnavailable`,
whose refusal names the file), and refuses adapter additions until it is
repaired; a damaged entry does both for its origin only. `config_result`
reports it as `adopted_state` (issue, revision, and recording warnings), and
settings' **Forget recorded adapters** (`forget_adopted_contracts`,
`ConfigStore::forget_adopted_contracts`, `extension/adopted-state-settings.ts`)
backs the file up and records afresh under the config lock. Those contracts alone decide an origin they
cover, so a stripped canonical URL shared with another page (a Jira board and
its issue modal) is never identity evidence, and older-note candidates must
map to the same artifact. An origin with no recorded contract uses the URL
key with no history lookup or note scan: its recorded URLs are stripped forms
and cannot name an artifact, and its URLs never count as older-note
candidates. Ambiguous contracts fall back to the URL key, while an
uncompilable recorded contract refuses loads and saves instead of guessing an
autosave target. A save that would make `strip_params` remove a query
parameter an adapter rule reads (a `query` key or `absent` key of a current,
history, or adopted contract) is refused as `invalid_config`
(`config::validate_strip_params_readers`, brwr-f4s); a config.toml that
already has one still loads and saves, and settings derives the warning from
the snapshot and offers **Stop stripping**.

Explicit preserved-note viewing (`brwr-r88`, 2026-10-28) is separate from
note-key resolution. The panel's **Preserved notes at this address** disclosure
uses additive protocol-v8 `list_preserved_notes` / `preserved_notes_listed`
and `view_preserved_note` (answered by `note_loaded`). It uses the existing
bounded vault scan, verifies an artifact note at its owned path and exact
stored canonical address, and lists at most 24 titles/IDs. The viewer is
read-only and copyable; it never enters NoteEditor, migrates a file, restores
capture, or treats a canonical address as autosave identity. No index or
recorded contracts are needed; ordinary uncovered-page note loads still do
no scan. Collapse/navigation clear results and invalidate pending reads.

Removed-site restoration (`brwr-cua`, 2026-10-28) uses additive protocol-v8
`get_site_restore` / `site_restore_loaded` before a new starter or draft, on an
explicit Capture Site click. The host returns only usable recorded recognition
contracts for that exact unconfigured origin, deduplicating identical entries
and refusing damaged or overlapping contracts (container overlaps included).
The panel names page kinds and fixed account scopes, re-reads the set before
an explicit revision-checked save, preserves global pauses, and uses the
existing note-save/recovery boundary. Title cleanup and title-derived aliases
or references were not recorded and are not reconstructed. No history is
forgotten, note migrated, page content read or capture resumed implicitly.

The panel's Capture Site flow now sets up previously unconfigured Jira Cloud,
SharePoint/OneDrive, and GitHub origins using the existing development starter
catalog. It reviews the current page, illustrative examples and fixed safety
probes inline. GitHub now offers repositories alongside pull requests and
issues, all three checked by default and sharing one organization field
(2026-10-27) instead of separate PR and issue lists; the field is pre-filled
from the current URL's owner segment (also `/orgs/<org>/…`), lowercased and
shown for the user to confirm or edit rather than applied unseen, and a page
with no usable owner (the GitHub root, a reserved top-level route, `/orgs`
with no org) gets no pre-fill and asks instead. The confirmation line names
only the types actually checked, so it is never wider than what is saved. It
then saves once through revisioned `update_config`. It preserves configured
sites and global pauses, refuses pending local pauses/removals and stale
reviews, and saves old note edits before reloading their host-derived identity.
Installing policy from a page seeds already-open recognized tabs for attention
measurement without inventing cold opens; background renewals retain their
normal navigation accounting.

GitHub repositories as container artifacts (2026-10-27): the adapter-template language gained a general mechanism for an adapter to declare `within = { type, id }`, naming a container artifact its own matches sit inside (adapter-templates.md §3, §8.3), and a required path capture a `scope` key restricts to a literal list now counts as an anchor segment too (§4.1, §6), not only a literal or a fixed-shape capture. `github.repo` is the first adapter to use either: a new starter, a catch-all over everything inside a GitHub repository that no PR or issue adapter claims, scoped to the owner like the other two, with `github.pr` and `github.issue` now declaring `within` it. `orderContainedFirst` (`extension/starter-adapters.ts`) keeps PR and issue adapters ahead of the repo catch-all on every save path (the panel, settings, and the JSON editor), because adapters on one origin are tried in config order and a PR or issue visit already rolls up into its repository. The host's index (`host/src/index.rs`) derives each contained visit's container by re-evaluating the logged URL, not by recording it at log time, so existing PR and issue history rolls up into repositories retroactively; a `within` edge records the relationship, worklog and day totals count a rolled-up visit once, and a container whose own page has never been opened directly still gets a title and URL derived from its ID instead of a stale or missing one. The route-history guard in `host/src/config.rs` is amended so a declared container may overlap the routes of a retired or removed contract it is exactly the container of, which is what lets settings add `github.repo` on top of already-configured PR/issue adapters (adding `within` to them in the same save) without tripping the existing-history freeze. Protocol version 7 exposes the relationship to the panel: `ArtifactView.within` and `.contained` on `get_artifact`/`list_artifacts`, with the card showing "In <repo>" on a contained artifact's own card and "Worked on inside" with its rolled-up contents on a container's card.

Single-event recovery (`brwr-3cc`, 2026-10-28): new per-event intents use format 2 with prepared bytes (512 KiB cap), their hash and a renderer-independent payload hash; the full JSON journal is capped at 1 MiB. Single and batch retries check that identity and exact torn bytes before resuming. V1 hash-only intents retain two frozen renderer generations in `host/src/capture/legacy.rs`: before the presentation sanitizer, and before inline math/code escaping (`brwr-2za`). New daily-log titles and Why text use `markdown::inline`. Complete visit markers still deduplicate by ID; summary retries require the same payload and selected folder too.

Stripped-parameter provenance (`brwr-srp`, 2026-10-28): canonicalization records the sorted, distinct decoded names actually removed, never values. New daily markers add `stripped=<hex JSON>` with main/mode/trail evidence; wire event shapes and the daily-log header are unchanged. Cache format 8 keeps it beside each event and SQLite's `url_provenance` table exposes it (JSON arrays, SQL NULL for legacy unknown). Notes and summaries record `brauser.stripped_params`; note saves and migrations preserve creation evidence, including a legacy missing field. Older-note discovery/view/migration rejects evidence that removed any current or recorded contract's query/absent key on that origin. It never chooses a primary autosave key. New single-event intents are v3 and bind that evidence; retain the frozen v2 hash and all three pre-provenance v1 presentation candidates for exact recovery.

Incoming navigation (`brwr-1g2`, 2026-10-28): protocol version 8 adds required
`ArtifactView.incoming_trails`, up to five host-derived source/transition pairs
ordered by latest recorded local day, then source ID and transition. The panel
shows text-only source titles, explicit HTTP(S) links and navigation wording.
Unknown legacy transitions, back/forward, unresolved sources and non-trail
relationships never supply provenance. Empty results say no known incoming
source was recorded; log and cache formats are unchanged.

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
Those shapes are also the only page-derived input to unknown-site capture.

Capture Site for unknown sites (`brwr-t40`, 2026-10-27, on
`feat/capture-site-unknown`) adds two host-only request/response pairs
within protocol v6, `draft_site`/`site_drafted` and
`check_site_draft`/`site_draft_checked`; no existing message changed shape.
Only the exact origin and private shapes cross (the current page's shape plus
the worker sample, and at most 50 click-time link shapes); no raw URL, link
`href`, or tab title reaches the host. The host's deterministic structural
drafter (`adapter-templates/src/structural.rs`) runs Filter, Group, Keep, Map,
Name and Rank, and every drafted rule then passes the per-rule checks
`check_draft_rules` (`adapter-templates/src/draft.rs`): a failing drafted rule
is dropped with its reason and the rest kept, a user-written rule only warns,
and a shared origin with no agreeing scoped adapter gets pending scope rather
than a refusal. Before a drafted rule's first identifier the anchor is a
literal or the scoped shared-origin tenant (#126's scoped-capture anchor;
`scope_anchored` rules cannot be saved unscoped). The review pre-fills the
tenant from the tab's URL as "Capture for <tenant>", already chosen, so Save
confirms it in one click (John, 2026-10-27), but only when a kept drafted rule
scoped to that value records the tab's page (`tenantPrefill`). The probe paths, search and paging keys, blocked route words,
mode words, fill values, starter types and shared-origin tenant/account-index
positions are one host-owned catalog, `protocol/capture-site/catalog.json`,
that the extension's tests pin. The panel's review is plain language with the
real current page evaluated locally; scope, `unscoped` and a title entry are
panel-owned overlays applied after `check_site_draft`, and scope travels as a
separate field so it never trips `draft_forbidden_key`. The save re-checks the
selection against the reviewed revision and then saves with the usual
revision-checked `update_config`. On a configured origin a recurring-pages
hint (an unrecognized sampled shape seen three times) drafts additions only;
existing adapters are never edited. **Open advanced settings** hands the draft
to the adapter editor through a 30-minute, read-once `storage.session` entry,
where an edited rule becomes user-written. Drafted rules never re-map an
already-recognized artifact: a shape the current policy or a recorded
contract (retired or hand-removed) recognizes is listed as already captured,
a rule that may overlap one is dropped, and `draft_site` refuses a draft
whose `remapped` is not 0. Secret and capability query keys (catalog
`secret_query_keys`) never become IDs, blocked route words are matched by
their hyphen/underscore parts and plurals and at the shared-origin tenant
position too, and a link shape never renames or reranks a visited kind. Known
limits: a Notion page with no workspace segment is not drafted (workspace
pages draft behind the scoped workspace, their anchor; `brwr-gf1` as
revisited, DESIGN.md §5.1), and on a shared origin a short
lowercase repository or namespace name stays a literal, so its rule covers
that one name. Starter live-tool certification remains human work, and
re-learning and re-keying (`brwr-6x2`) are not built.

Capture Site Improve with AI (`brwr-f45`, 2026-10-28, on
`feat/capture-site-agent`, DESIGN.md §5.1 and §12.2 step 7.7) refines a shown
structural draft, including the recurring-pages hint's, with the default
harness through the service; the structural draft always comes first and
stays savable, and **Use basic draft** returns to it. Additive within
protocol v7: the extension sends `start_site_draft` with the origin and
shapes only; the host re-runs the structural drafter, builds the agent's
untrusted-data block itself, and forwards a `start_run` with
`kind: "site_draft"` (a `kind` from the extension is refused). The service
validates only the answer's form (`host/src/site_draft_answer.rs`, one
repair turn, then `invalid_output`) and writes nothing; the panel then sends
`draft_site` with the run's `run_id`, and the host judges the tables with
`structural::check_agent_draft`: a forbidden key or a foreign origin rejects
the whole agent draft before any preview, and every other rule passes
`check_draft_rules` as `Drafted`. The answer is `site_drafted` with
`drafter: "agent"`; a rule no seen shape matches starts unchecked as
`unseen`, and the save is the unchanged `check_site_draft` plus
`update_config`. Without a harness the button is disabled with Summarize's
setup hint; on Windows the run messages answer `service_unavailable`.

The service worker delivers buffered events and renews its policy lease itself, from `alarms` and at browser startup, so capture does not depend on opening the panel (DESIGN.md §3.1); the panel's Send pending visits asks the worker to sync now. The rest of the agent integration, search index, signed installers, Chrome Web Store listing, and public release do not exist yet.
