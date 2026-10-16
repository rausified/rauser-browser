# Brauser Feature Backlog

This file is read by Cookie's overnight loop. DESIGN.md is the authoritative
spec: read it before starting any item below, follow its decisions (§14) and
security rules (AGENTS.md), and update it yourself when an implementation
choice changes a contract it describes. Do not treat this backlog as a
substitute for DESIGN.md — it exists to keep each cycle's scope bounded, not
to repeat the spec.

**Priority rule (set by John, 2026-10-16): the DESIGN.md §12.2 plan comes
first.** Each cycle, take the first unchecked item under "Plan work" below, in
the order listed, and build it before anything else. Do not ideate, pick, or
start self-proposed items under "Cookie's own proposals" while any plan item
you can complete on this VM remains unchecked. A plan item that needs a human,
real macOS Chrome, or a live work tool is marked **[human]**: skip it, do not
fake its evidence, and move to the next plan item. If a plan step is too big
for one cycle, split it into ordered sub-items here rather than reordering the
plan. You may still add proposals and notes at cycle end — just don't build
them ahead of the plan.

**Beads is the machine queue (2026-10-16).** `.beads/issues.jsonl` holds one
bead per open item below, structured like Privateer's queue. Known Issues
are P1 and plan work is P2. Cookie's proposals are P3, so none of them outranks
plan work. Dependencies are only the real ones (8.2 waits for 6.5-6.7, then
8.3, 8.4, 8.5 in turn), so `bd ready` shows every Known Issue at once. **Take
them in the order listed here, per the rule above.** **[human]** items carry
the `human` label (`bd human list`) and depend on the deferred `brwr-hpo`
HUMAN GATE bead. Do not close the gate to make them ready. Every open item
here carries its bead id; `scripts/check_queue_consistency.py` refuses a
launch when the two disagree.

**Known issues come before plan work (set by John, 2026-10-16).** While any
item under "Known Issues (Blocking Deploy)" is unchecked, take the first one
before any plan item. Fix it with a regression test, check it off with a
one-line note, and keep each fix to one cycle. These came from a code review
of PR #34 (merged to GitHub main as 68f180b). The review was not separately
verified, so confirm each failure scenario with a failing test first. If the
test shows the finding is wrong, check it off as "not reproduced" and say why.

## Known Issues (Blocking Deploy)

- [x] `brwr-7yp` **Adapter removal refused when two adapters share origin and type.**
  `validate_adapter_transition` (host/src/config.rs ~1254) checks each old
  adapter against a surviving sibling's id. Two `work.issue` adapters on one
  origin (`/issues` id X, `/tickets` id Y): removing the second fails with a
  misleading "id template ... cannot change" error. The same happens when
  `adaptersForSites` drops only one of the pair. Fixed by comparing counted ID
  template sets per origin and type, with a regression that preserves real ID
  replacement refusal.
- [x] `brwr-drp` **The id-template freeze can be bypassed** (host/src/config.rs ~1234).
  Removals are never checked and identity is (origin, type). So you can remove
  `jira.issue` and later re-add it with a new id, or change type and id in one
  edit, and either way existing artifact records are orphaned. That
  contradicts the doc comment's "impossible for a config edit to orphan
  activity". Decide the intended rule (for example, remember retired ids per
  origin), then enforce it. Fixed with host-owned durable recognition/ID
  contracts: overlapping routes cannot return under a new ID or type, while
  the original ID and genuinely disjoint routes remain available.
- [x] `brwr-rhg` **No repair path while `adapter_issue` is set** (host/src/consent.rs
  ~336, extension/options.ts ~161 and ~517). Every write that echoes the stored
  adapter document fails full `validate()`. That includes harness setup,
  denylist, and folder changes, and the UI disables every adapter control and
  refuses format != 1. This breaks the "notes and AI settings remain usable
  during adapter repair" contract. Non-adapter writes must succeed, and the UI
  needs a way to replace or remove the bad document. Fixed by allowing only an
  exact invalid-document echo for unrelated writes, keeping those controls and
  Pause usable, and offering a revisioned remove-all repair that stays
  fail-closed until the host re-reports a valid policy.
- [x] `brwr-99f` **Adapter editor keys on a stale array index.** Fixed by
  retaining the opened adapter's identity and canonical structural snapshot:
  external reorders follow the same target, while a changed, removed, or
  ambiguous target disables Save and keeps the newer config intact.
- [x] `brwr-xsq` **Removing an adapter or site re-confirms every later adapter**
  (host/src/consent.rs ~808, `approval_lines`). An index shift reads as a
  policy change, so revoking one site asks for "Enable or change ..." plus the
  every-account disclosure for each later unscoped adapter. With ~29 or more
  unscoped adapters it passes the 8192-byte cap and refuses the removal. That
  contradicts DESIGN "Removing or narrowing them takes effect immediately".
  Fixed by matching survivors on `(origin, type, id)` and separately checking
  relative order, so deletion shifts are immediate while real precedence
  reorders still require confirmation.
- [x] `brwr-uc0` **Invalid adapter policy discards recognized visits for good**
  (host/src/native.rs ~534). `adapter_policy_invalid` returns terminal
  Suppressed, while `config_issue` returns retryable. Visits recognized under
  a valid lease are lost during a transient fault. Fixed across legacy visits,
  single activity delivery, and current mixed batches: an invalid adapter
  document now returns retryable for every capture event and leaves batch
  overflow evidence pending, while an intentional Pause remains terminal.
- [x] `brwr-4kh` **Chrome revocations not written back while `adapter_issue` is set**
  (extension/settings.ts ~317; `removeSite` in options.ts).
  `reconcileRevocations` throws, so the host keeps a revoked origin
  allowlisted. Revocations must always narrow host policy. Fixed by committing
  the narrower site and capture policy while preserving an invalid/future
  adapter document exactly and suspended for explicit repair.
- [x] `brwr-flu` **Full adapter compile on every navigation** (extension/model.ts ~153).
  `isPolicyLease` runs `validateDocument` on every `readPolicy`, including the
  never-enabled-site fast path, and `matchingArtifact` stringifies the document
  for its cache key on every call. Fixed with a revision-keyed compiled-policy
  cache: stored leases compile once per worker revision, matching does no
  adapter serialization, and every untrusted install still validates afresh.
- [ ] `brwr-mwg` **Quadratic overlap scan on every host refresh** (host/src/config.rs
  ~950, ~1226). `read_disk_state` computes `overlap_report` on every Hello,
  GetConfig, LoadNote, SaveNote, and RecordVisit, and `adapter_matches`
  recompiles. Compute warnings only for GetConfig and post-commit, and reuse
  one compiled Document per revision.
- [ ] `brwr-7sb` **AGENTS.md "Current state" is stale.** It says the worker lease, the
  TypeScript prefilter, and host recognition are unbuilt, but they shipped.
  Update it to match README.md and DESIGN.md.
- [ ] `brwr-5fg` Smaller ones from the same review:
  - In `setStarterButtonState` (options.ts), `offer?.adapter !== null` is true
    when `offer` is null.
  - `hasAdapter` (starter-adapters.ts) can't see an existing GitHub adapter
    until an org scope is typed.
  - `StoredAdapters` uses `deny_unknown_fields`, so a newer `[adapters]` key
    makes the whole config unreadable (backup-and-rewrite) instead of just
    suspending adapter policy.
- **Standing rule (not a task): keep the smoke runner's dialog count right.**
  When a change adds or removes a scripted dialog or smoke step, update the
  expected count in `scripts/smoke-macos.mjs` in the same commit. The smoke run
  can't execute on this Linux VM, so GitHub CI's macOS job
  (`npm run smoke:macos:headless`) is the only place it runs. Note the expected
  count in the cycle summary. GitHub main (68f180b) was merged into this
  workspace on 2026-10-16 and brought in the count fix (14) and the gitignored
  `.last_cycle_review_output.md`.

## Proposed

### Plan work (DESIGN.md §12.2, in order)

- [x] **M1.5a step 6.4 — Measured artifact activity.** Replaced the temporary
  visit rows with hourly artifact activity: cold opens, focused and background
  seconds, modes, and trail sources in durable worker totals, frozen into one
  idempotent `activity` event per artifact and local hour, plus `why` events.
  Rebuild the tab map on worker start, and cover the hour boundary, UTC-offset
  change, browser restart with a pending total, idle/lock, discarded tabs,
  reloads, restored sessions, SPA route changes, board-modal switches, and the
  map-rebuild cases listed in §12.2 step 6.4 and §5.1.
- `brwr-6at` (in progress; see In Progress below) **M1.5a step 6.5 — Delivery and durability.** One write, one sync, and
  one intent record per delivery batch; the compact intent index replacing
  M1's one-intent-file-per-visit journal; buffer-pressure coalescing, the 80%
  badge, and overflow accounting in the strip and the log. Retire
  `near_repeat_secs` (accepted and ignored in old configs, dropped from the v5
  schema, confirmation line removed). Test two `/log` entries for one URL a
  minute apart, both recorded.
- [ ] `brwr-xeh` **M1.5a step 6.6 — Index tables and artifact files.** `artifacts`,
  `visits`, and `edges` tables in `index.db`; lazy regeneration from the dirty
  set with skip-if-unchanged and Timeline rollups (§6.7); `brauser reindex`
  rebuilding both from the logs. Test a hostile title, a case-colliding ID, an
  external edit being replaced, a heavy-use day producing no file write until
  regeneration, and a year of daily opens staying within rollup bounds.
- [ ] `brwr-m40` **M1.5a step 6.7 — Panel, Recent, worklog, and record keys.** The
  artifact card (§5.7), the one-liner, the Recent tab (one history across
  tools, grouped by day, type and title filters), and the worklog view with
  Copy, backed by `get_artifact`, `list_artifacts`, and `worklog`. Record keys
  (§6.2) land here and **must ship before any M2 summary write**: `load_note`
  and `save_note` resolve and return the key, and older URL-keyed notes are
  listed. Cover every test case named in §12.2 step 6.7.
- [ ] `brwr-5yn` **[human] M1.5a step 6.8 — Certify the three starter adapters.** Check
  the Jira, SharePoint/OneDrive, and GitHub drafts against each live tool and
  record the date in `docs/adapters/`. Needs John on macOS with real accounts;
  Cookie must not change the "development draft" label or mark 6.8 done.
- [ ] `brwr-npy` **§12.2 step 2 — Headless page-load check in CI (optional,
  recommended).** Load the built `extension/dist/` in headless Chrome and open
  the panel and settings page, so a stale-manifest class of failure is caught
  before a manual run.
- [ ] `brwr-6wz` **M2 step 8.2 — Host invocation.** Start only after 6.4–6.7 are shipped.
  Allowlisted environment, templated argv, page content on stdin inside the
  untrusted-data block, host-owned working directory, timeout, output cap,
  process-tree kill, and the confirmed-install-location check (§7.1), with the
  fake-harness tests and the §7.1 canary listed in step 8.2.
- [ ] `brwr-0cj` **M2 step 8.3 — Streaming protocol.** A reader that stays responsive
  while a harness runs; chunk, `rejected`, error, `cancel`, and
  `summary_result` messages correlated by `request_id` under the 900 KiB cap;
  stdin EOF kills the harness. Takes the next protocol version after v5.
- [ ] `brwr-0ij` **M2 step 8.4 — Content extraction.** Bundled, pinned Readability-style
  extraction, the `summarize` command, context menu, and toolbar trigger, and
  the §8 permissions. **[human]** sub-step first (`brwr-rva`): the real-Chrome `activeTab`
  checks (a)–(d) in step 8.4 need macOS; build the rest behind them only once
  they are recorded.
- [ ] `brwr-deo` **M2 step 8.5 — `/summarize` in the panel.** Denylist checks, the §5.2
  and §4.4 summary write and result contract, the §3.1 extension-page CSP with
  a manifest test, and the external-editor race, shared summaries directory,
  and mid-run config-change tests.

### Gated plan work (do not start yet)

- **M1.5b (§12.2 step 7.1–7.7)** — aliases, Tier 1 fields (`scripting`),
  worklog files, the remaining starter adapters, Create adapter with the
  structural drafter, then agent drafting. DESIGN.md gates this on M1.5a being
  in daily use for a few weeks; John will lift the gate here when it's time.
- **M3 — Related** (FTS5 in `brauser reindex`, related pages, read later),
  **M4 — Omnibar** (built-in commands, free text, custom commands), and
  **M5 — Release** (installer, store listing, signing and provenance, docs)
  follow M2, per the §12 table. M5 signing and store work is **[human]**.

### Focused quality follow-up

- [ ] `brwr-4vb` **Make the dialog reply test consume its request.** The fake
  child can exit after printing its reply before the parent writes the framed
  request, intermittently producing `EPIPE` even though production dialog
  children read the request first. Make the fixture obey the real handshake
  and exercise it repeatedly; this was discovered during the current cycle's
  final Rust verification and does not affect the shipped worker change.

### Cookie's own proposals (only after the plan work above)

- [ ] `brwr-ued` **Group cross-tool work into named focus sessions.** Let a
  person start and end one explicit local session from the panel so a Jira
  issue, design document, and pull request can appear together under the
  reason they were opened. Carry the stable session through worker restarts
  and measured activity into the append-only log and derived worklog; never
  infer a session from browsing or send it to a service. This should build on
  the artifact card and worklog after M1.5a rather than delay those foundations.
- [ ] `brwr-ac1` **Make first-tool setup one coherent transaction.** Let a new user choose
  Jira, SharePoint/OneDrive, or GitHub first, enter the exact tenant and any
  required scope, then request Chrome access and native-confirm the site plus
  starter together. Roll back a newly acquired grant on cancel and retain it
  when notes or another site purpose already needs it. This removes today's
  confusing intermediate state where a site is enabled but records nothing
  until a second adapter save and confirmation.
- [ ] `brwr-jnj` **Offer non-destructive adapter document repair.** The immediate repair
  path removes the entire invalid document. Add copy-out and a whole-document,
  format-1 replacement workspace so a user can salvage valid rules without
  asking an older build to interpret or migrate an unsupported future format.

## In Progress / This Iteration

- [ ] `brwr-6at` **M1.5a step 6.5 — Delivery and durability.** Replace per-event delivery
  and intents with one bounded batch, add pressure coalescing and the 80%
  badge, carry overflow evidence into the append-only log, and retire
  configurable near-repeat suppression without breaking older config files.

## Shipped

- **Category:** Performance & Resource Bounds. **Compile adapter policy once per worker revision (2026-10-16).** The service worker now validates and compiles a stored adapter document once when its host revision is first restored, then reuses that prepared document across policy reads and artifact matches instead of rebuilding the grammar and serializing the full policy on every navigation. Every install message still compiles independently at the untrusted boundary, malformed cold-start policy stays inert, and site and live-grant gates remain ahead of evaluation. **Metrics:** 265 Rust tests and 8 extension suites passing; 2 new cache and navigation-hot-path regressions including a failing-first compile-cache proof, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Security & Privacy. **Narrow host policy after Chrome revokes access during adapter repair (2026-10-16).** Settings now writes revoked origins out of the host's site allowlist even when an invalid or future adapter document has already suspended capture. The revisioned update preserves that unsupported document without structural changes for explicit repair, disables capture when its final site disappears, rechecks revocations under the configuration lock so a stale snapshot cannot remove a concurrently re-enabled site, and acknowledges revocations only after the host is already narrow. **Metrics:** 265 Rust tests and 8 extension suites passing; 3 new failing-first settings regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** Bug Fix. **Keep recognized capture queued while adapter policy needs repair (2026-10-16).** The native host now returns a retryable result when a transient invalid or future adapter document interrupts legacy visits, single hourly activities, or the current mixed delivery batch, so the worker retains every recognized event and any overflow evidence until policy is repaired. An intentional capture pause and a valid policy miss remain terminal, preserving immediate user control and defense-in-depth filtering. **Metrics:** 265 Rust tests and 8 extension suites passing; 3 failing-first ingress regressions (+2 net tests), with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** Bug Fix. **Let adapter and site removals take effect without re-confirming survivors (2026-10-16).** Native consent now matches adapters by stable origin, type, and ID instead of their mutable array slot, so removing an early adapter or an origin no longer makes every later unscoped rule look newly enabled or overflow the confirmation limit. Relative prior positions are still checked independently, preserving native confirmation for a genuine precedence-changing reorder, and ambiguous duplicate identities fail closed. **Metrics:** 263 Rust tests and 8 extension suites passing; 2 new failing-first and privacy-boundary regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Bug Fix. **Keep open adapter edits attached to the adapter the user chose (2026-10-16).** The settings editor now retains an adapter's origin, type, ID, and canonical structural snapshot instead of a mutable array index. A config refresh can reorder other adapters without redirecting the pending edit, while an externally changed, removed, or ambiguous target disables Save, explains the conflict, and preserves the newer revision for review. **Metrics:** 261 Rust tests and 8 extension suites passing; 2 new failing-first cross-page regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Bug Fix. **Restore settings and recovery while adapter policy is invalid (2026-10-16).** The host now permits unrelated settings and harness setup to carry forward only the exact already-inert adapter document, while every adapter edit still requires a fully valid replacement. Settings keeps folder, AI-privacy, harness, and Pause controls usable, offers a clear remove-all repair action, and re-reads host adapter status before publishing any worker lease so the recovery path remains fail-closed. **Metrics:** 259 Rust tests and 8 extension suites passing; 5 new regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** Bug Fix. **Keep adapter record keys frozen across removal and type changes (2026-10-16).** Removing or changing an adapter now persists its minimal recognition and ID contract in host-owned config state that never crosses the extension protocol. A type or ID swap and a later remove/edit/re-add on an overlapping route are both refused until alias migrations exist, while restoring the exact contract or introducing a genuinely disjoint route remains valid; disjoint same-ID reuse cannot erase older route history, and the history survives host restart and is replaced atomically with config. **Metrics:** 256 Rust tests and 8 extension suites passing; 3 regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** Bug Fix. **Allow one same-type adapter to be removed without misclassifying its sibling (2026-10-16).** Adapter transition validation now compares counted ID-template sets for each origin and artifact type, so deleting either member of a same-type pair is a legitimate narrowing instead of a false frozen-ID edit. Additions and reorderings still work, while a real template replacement—including one hidden among siblings—remains refused until alias migrations exist. **Metrics:** 253 Rust tests and 8 extension suites passing; 1 new regression, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** New Capability. **Replace navigation rows with measured hourly artifact activity (2026-10-16).** Automatic capture now measures cold opens, focused and background time, modes, refs, and capped trail sources in durable offset-qualified hourly totals instead of producing a row for every navigation. The worker rebuilds recognized tab state after a restart, handles idle, focus, SPA, reload, discarded-tab, and daylight-saving boundaries, preserves upgrade-era visits, and exposes live versus pending activity in the panel; the host independently revalidates artifact identity, mode evidence, title refs, and every trail before an idempotent append. Activity and `why` markers retain a canonical machine payload for the coming index without exposing stripped URL parameters, while the human log stays concise and M1 markers remain readable. **Metrics:** 249 Rust tests and 8 extension suites passing; full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** New Feature. **Turn enabled work sites into reviewable starter-adapter drafts (2026-10-16).** Settings now recognizes root-scoped Jira Cloud, SharePoint/OneDrive, and GitHub sites, resolves the shared cross-engine fixtures to each exact origin, and opens a readable starter in the existing full-document preview before native-confirmed save. GitHub drafts stay disabled until the user names valid work organizations, never offer every-account capture, normalize and deduplicate the scope, and cover pull requests and issues separately; narrow site policies stay custom rather than being silently widened. Four per-type documentation pages record routes, frozen IDs, scope behavior, limits, and the still-pending live-tool check, so development usefulness is not mistaken for release certification. **Metrics:** 244 Rust tests and 7 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** User Experience. **Make artifact adapters reviewable and configurable in settings (2026-10-16).** The settings page now translates every adapter into readable ordered rules, ID, scope, title, and Tier 1 field disclosures; names both sides of overlap warnings; and carries the folder-sync caution. A native-confirmed JSON workspace supports add, edit, remove, and precedence changes with live full-document validation plus URL/title results showing the winning rule, canonical ID, title, and mode; fixed root, search, login, dashboard, auth, sharing, and capability probes highlight risky matches before save. Removing an origin's final site or reconciling its revoked Chrome grant now removes that origin's adapters in the same host update. **Metrics:** 244 Rust tests and 6 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** Security & Privacy. **Activate adapter-only capture at both trust boundaries (2026-10-16).** Every host-confirmed adapter document now travels in the revisioned worker lease, where the TypeScript engine drops rule misses, out-of-scope URLs, and ungranted navigations before reading tab metadata or buffering page-derived content. The native host independently re-evaluates every original URL and title under the config lock and terminally suppresses anything a stale or compromised worker widened; grant removal narrows sites and templates together, and a cached pre-adapter lease self-renews once instead of silently disabling background capture after upgrade. The macOS smoke flow now confirms a fixture adapter through the real native protocol before proving that only its recognized route reaches the log. **Metrics:** 244 Rust tests and 5 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** Testing & Verification. **Restore portable vault and harness verification (2026-10-16).** Directory durability now reopens the current capability before syncing, so Linux `O_PATH` handles no longer turn note saves and visit logging into `EBADF` failures while the filesystem boundary remains capability-scoped. Fake harness installs now use explicit private modes instead of inheriting the runner's umask, and their work-directory assertion selects the platform's actual `stat` syntax, keeping the security suite deterministic without weakening its unsafe-install checks. **Metrics:** 243 Rust tests and 5 extension suites passing; full protocol generation, build, format, clippy, typecheck, lint, and extension checks green.
- **Category:** New Capability. **Make adapter templates real, host-validated protocol v5 configuration (2026-10-16).** Brauser now round-trips `[adapters] format = 1` and `[[artifacts]]`, validates every definition with the Rust adapter-template authority and enabled-origin policy, surfaces bounded overlap warnings, and puts adapter changes behind native consent. Unsupported future formats remain intact on disk while every extension entry point and the autonomous worker suspend capture; unscoped shared origins disclose every-account capture, existing IDs stay frozen until migrations exist, and page notes and AI settings remain usable during adapter repair. **Metrics:** 12 protocol tests, 8 focused host regressions, and 5 extension suites passing; full build, format, clippy, typecheck, lint, and extension checks green.

## Notes / Ideas for Later

- Worker policy revisions are SHA-256 identities for the host configuration,
  so they are the right cache key for compiled adapter templates. Test fixtures
  must preserve that invariant: reusing one fake revision for different
  documents can hide stale-policy bugs or create failures impossible in a real
  host exchange.
- The full Rust rerun exposed `brwr-4vb`: the successful-dialog fixture can
  answer without reading its framed request and race the parent into `EPIPE`.
  It passed the opening gate, its isolated rerun, and the next complete suite;
  the production child handshake is unaffected, and the focused fixture repair
  remains queued instead of being mixed into this deploy-blocker cycle.
- Once the artifact card and worklog exist, a named focus session could supply
  the missing human intent that measured attention deliberately does not infer.
  `brwr-ued` keeps that proposal local and explicit: the user names the work,
  and Brauser only groups already-authorized artifacts under that name.
- Legacy `record_visit`, compatibility `record_activity`, and current
  `record_batch` had drifted into three different answers for the same invalid
  adapter state. Keep their shared policy-state outcomes aligned until the old
  entry points have had a release to drain and can be removed; event-shape and
  artifact-identity validation can remain specific to each payload.
- Automatic capture now emits measured activity, while the legacy visit path exists only to drain pre-upgrade queue entries. Step 6.5 should batch new activity writes and compact intents without removing that compatibility until old queues have had a release to drain.
- The headless macOS smoke flow was updated to confirm and install a fixture adapter through native messaging. It could not be executed on this Linux VM; run `npm run smoke:macos:headless` on macOS before relying on that new end-to-end step.
- Protocol v5 currently freezes an existing adapter's `id` template before any alias migration exists. When activity events land, replace that conservative guard with the designed activity-aware migration and explicit alias events rather than relaxing it in place.
- The adapter workspace now resolves overlap indices to type names and previews fixed probes. It deliberately has no URL-shape sample or already-recognized-artifact re-map count yet; those inputs arrive with Create adapter in M1.5b step 7.6 and should extend this review rather than create a second editor.
- The JSON definition is the advanced configuration surface, while its surrounding cards and preview translate the consequences. Starter adapters should make the common first-use path a choose-and-scope flow, leaving JSON for custom and self-hosted tools.
- The first-tool gallery now supplies that choose-and-scope flow, but its entries intentionally say development draft until a macOS pass checks current Jira, SharePoint/OneDrive, and GitHub URLs and titles against live tools and dates each `docs/adapters/<type>.md` page. Do not remove that label based only on fixture tests.
- The starter catalog is intentionally code-generated from exact sites rather than stored in user config as placeholder origins. Its regression suite compares every generated object byte-for-structure with the shared Rust/TypeScript fixtures, so a future template edit must update the catalog, cross-engine vectors, and human docs together.
- First use still asks the user to enable a raw site, accept one native confirmation, find the starter gallery, and accept another. The gallery makes the second half understandable, but the product-level follow-up should compose both trust changes into one reviewed transaction while preserving Chrome-grant rollback semantics.
- The settings-page fake-DOM flow covers every mutation, but this Linux VM cannot prove layout, focus behavior, native dialogs, or optional-permission prompts in Chrome on macOS. Include the adapter workspace in the next requested interactive macOS smoke pass.
- Step 6.6's index reader should treat the hex marker payload as the authority and the Markdown sentence as presentation only. It must keep reading older visit markers, reject malformed or oversized activity payloads without losing the rest of a log, and apply the same URL-normalization assumptions as the writer.
- Real idle/lock transitions and cross-application focus accounting remain outside headless proof. When John next requests the interactive macOS pass, compare one known reading interval and one terminal-away interval with the panel's live focused/background totals before calling the engagement labels field-verified.
- Removed adapter recognition/ID contracts now remain as compact host-owned `adapter_id_history` entries until their exact ID is restored. M1.5b alias migrations should consume or supersede this conservative history explicitly, rather than merely relaxing the transition check and reopening the orphaning path.
- Adapter-only repair now keeps the invalid document byte-for-structure and makes destructive reset explicit, but it cannot selectively salvage rules. `brwr-jnj` tracks a copy-out and full-document replacement workspace; it must never auto-downgrade or reinterpret an unsupported future format.
- Cross-page adapter edits can safely follow `(origin, type, id)` plus an unchanged structural snapshot today. If M1.5b aliases introduce persistent adapter or rule identifiers, carry those identifiers into the editor target instead of weakening this fail-closed conflict check; same-identity ambiguity must never fall back to array position.
- Consent deltas now use the same `(origin, type, id)` identity as the settings editor but deliberately keep precedence as a separate ordered concern. If persistent adapter IDs arrive with aliases, migrate both consumers together and retain explicit reorder detection; conflating identity with array position was the root cause of two independent deploy blockers.
- A Chrome revocation can leave a rule for that origin inside an unsupported adapter document because the host's repair exception permits only an exact echo. Capture remains suspended and the host site allowlist is narrowed immediately; `brwr-jnj`'s replacement workspace should call out and remove these now-orphaned rules when the user converts the document to a supported format.
