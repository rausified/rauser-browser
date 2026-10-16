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

## Known Issues (Blocking Deploy)

- None.

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
- [ ] **M1.5a step 6.5 — Delivery and durability.** One write, one sync, and
  one intent record per delivery batch; the compact intent index replacing
  M1's one-intent-file-per-visit journal; buffer-pressure coalescing, the 80%
  badge, and overflow accounting in the strip and the log. Retire
  `near_repeat_secs` (accepted and ignored in old configs, dropped from the v5
  schema, confirmation line removed). Test two `/log` entries for one URL a
  minute apart, both recorded.
- [ ] **M1.5a step 6.6 — Index tables and artifact files.** `artifacts`,
  `visits`, and `edges` tables in `index.db`; lazy regeneration from the dirty
  set with skip-if-unchanged and Timeline rollups (§6.7); `brauser reindex`
  rebuilding both from the logs. Test a hostile title, a case-colliding ID, an
  external edit being replaced, a heavy-use day producing no file write until
  regeneration, and a year of daily opens staying within rollup bounds.
- [ ] **M1.5a step 6.7 — Panel, Recent, worklog, and record keys.** The
  artifact card (§5.7), the one-liner, the Recent tab (one history across
  tools, grouped by day, type and title filters), and the worklog view with
  Copy, backed by `get_artifact`, `list_artifacts`, and `worklog`. Record keys
  (§6.2) land here and **must ship before any M2 summary write**: `load_note`
  and `save_note` resolve and return the key, and older URL-keyed notes are
  listed. Cover every test case named in §12.2 step 6.7.
- [ ] **[human] M1.5a step 6.8 — Certify the three starter adapters.** Check
  the Jira, SharePoint/OneDrive, and GitHub drafts against each live tool and
  record the date in `docs/adapters/`. Needs John on macOS with real accounts;
  Cookie must not change the "development draft" label or mark 6.8 done.
- [ ] **§12.2 step 2 — Headless page-load check in CI (optional,
  recommended).** Load the built `extension/dist/` in headless Chrome and open
  the panel and settings page, so a stale-manifest class of failure is caught
  before a manual run.
- [ ] **M2 step 8.2 — Host invocation.** Start only after 6.4–6.7 are shipped.
  Allowlisted environment, templated argv, page content on stdin inside the
  untrusted-data block, host-owned working directory, timeout, output cap,
  process-tree kill, and the confirmed-install-location check (§7.1), with the
  fake-harness tests and the §7.1 canary listed in step 8.2.
- [ ] **M2 step 8.3 — Streaming protocol.** A reader that stays responsive
  while a harness runs; chunk, `rejected`, error, `cancel`, and
  `summary_result` messages correlated by `request_id` under the 900 KiB cap;
  stdin EOF kills the harness. Takes the next protocol version after v5.
- [ ] **M2 step 8.4 — Content extraction.** Bundled, pinned Readability-style
  extraction, the `summarize` command, context menu, and toolbar trigger, and
  the §8 permissions. **[human]** sub-step first: the real-Chrome `activeTab`
  checks (a)–(d) in step 8.4 need macOS; build the rest behind them only once
  they are recorded.
- [ ] **M2 step 8.5 — `/summarize` in the panel.** Denylist checks, the §5.2
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

### Cookie's own proposals (only after the plan work above)

- [ ] **Make first-tool setup one coherent transaction.** Let a new user choose
  Jira, SharePoint/OneDrive, or GitHub first, enter the exact tenant and any
  required scope, then request Chrome access and native-confirm the site plus
  starter together. Roll back a newly acquired grant on cancel and retain it
  when notes or another site purpose already needs it. This removes today's
  confusing intermediate state where a site is enabled but records nothing
  until a second adapter save and confirmation.

## In Progress / This Iteration

- None. The next cycle starts with M1.5a step 6.5 above.

## Shipped

- **Category:** New Capability. **Replace navigation rows with measured hourly artifact activity (2026-10-16).** Automatic capture now measures cold opens, focused and background time, modes, refs, and capped trail sources in durable offset-qualified hourly totals instead of producing a row for every navigation. The worker rebuilds recognized tab state after a restart, handles idle, focus, SPA, reload, discarded-tab, and daylight-saving boundaries, preserves upgrade-era visits, and exposes live versus pending activity in the panel; the host independently revalidates artifact identity, mode evidence, title refs, and every trail before an idempotent append. Activity and `why` markers retain a canonical machine payload for the coming index without exposing stripped URL parameters, while the human log stays concise and M1 markers remain readable. **Metrics:** 249 Rust tests and 8 extension suites passing; full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, and extension checks green.
- **Category:** New Feature. **Turn enabled work sites into reviewable starter-adapter drafts (2026-10-16).** Settings now recognizes root-scoped Jira Cloud, SharePoint/OneDrive, and GitHub sites, resolves the shared cross-engine fixtures to each exact origin, and opens a readable starter in the existing full-document preview before native-confirmed save. GitHub drafts stay disabled until the user names valid work organizations, never offer every-account capture, normalize and deduplicate the scope, and cover pull requests and issues separately; narrow site policies stay custom rather than being silently widened. Four per-type documentation pages record routes, frozen IDs, scope behavior, limits, and the still-pending live-tool check, so development usefulness is not mistaken for release certification. **Metrics:** 244 Rust tests and 7 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** User Experience. **Make artifact adapters reviewable and configurable in settings (2026-10-16).** The settings page now translates every adapter into readable ordered rules, ID, scope, title, and Tier 1 field disclosures; names both sides of overlap warnings; and carries the folder-sync caution. A native-confirmed JSON workspace supports add, edit, remove, and precedence changes with live full-document validation plus URL/title results showing the winning rule, canonical ID, title, and mode; fixed root, search, login, dashboard, auth, sharing, and capability probes highlight risky matches before save. Removing an origin's final site or reconciling its revoked Chrome grant now removes that origin's adapters in the same host update. **Metrics:** 244 Rust tests and 6 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** Security & Privacy. **Activate adapter-only capture at both trust boundaries (2026-10-16).** Every host-confirmed adapter document now travels in the revisioned worker lease, where the TypeScript engine drops rule misses, out-of-scope URLs, and ungranted navigations before reading tab metadata or buffering page-derived content. The native host independently re-evaluates every original URL and title under the config lock and terminally suppresses anything a stale or compromised worker widened; grant removal narrows sites and templates together, and a cached pre-adapter lease self-renews once instead of silently disabling background capture after upgrade. The macOS smoke flow now confirms a fixture adapter through the real native protocol before proving that only its recognized route reaches the log. **Metrics:** 244 Rust tests and 5 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** Testing & Verification. **Restore portable vault and harness verification (2026-10-16).** Directory durability now reopens the current capability before syncing, so Linux `O_PATH` handles no longer turn note saves and visit logging into `EBADF` failures while the filesystem boundary remains capability-scoped. Fake harness installs now use explicit private modes instead of inheriting the runner's umask, and their work-directory assertion selects the platform's actual `stat` syntax, keeping the security suite deterministic without weakening its unsafe-install checks. **Metrics:** 243 Rust tests and 5 extension suites passing; full protocol generation, build, format, clippy, typecheck, lint, and extension checks green.
- **Category:** New Capability. **Make adapter templates real, host-validated protocol v5 configuration (2026-10-16).** Brauser now round-trips `[adapters] format = 1` and `[[artifacts]]`, validates every definition with the Rust adapter-template authority and enabled-origin policy, surfaces bounded overlap warnings, and puts adapter changes behind native consent. Unsupported future formats remain intact on disk while every extension entry point and the autonomous worker suspend capture; unscoped shared origins disclose every-account capture, existing IDs stay frozen until migrations exist, and page notes and AI settings remain usable during adapter repair. **Metrics:** 12 protocol tests, 8 focused host regressions, and 5 extension suites passing; full build, format, clippy, typecheck, lint, and extension checks green.

## Notes / Ideas for Later

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
