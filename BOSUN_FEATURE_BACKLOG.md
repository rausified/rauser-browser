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

**Resequenced by John, 2026-10-16 (architecture review).** Two product
decisions are recorded in DESIGN.md §14. First, users never see adapters:
**Capture Site** in the side panel learns a site's pattern and starts
capturing, with a plain-language confirmation. That moves Create adapter
(M1.5b step 7.6), the re-keying it needs, and later agent refinement (step
7.7) onto the critical path ahead of the rest of M2. Second, Windows support
is back in scope, starting with Windows CI. The order below already reflects
this. The goal is to get John using M1.5a daily as soon as possible, so the
worklog comes first within step 6.7. Treat new adapter-configuration hardening
findings as P3 unless they describe a concrete user-facing failure.

**Beads is the machine queue (2026-10-16).** `.beads/issues.jsonl` holds one
bead per open item below, structured like Privateer's queue. Known Issues
are P1 and plan work is P2. Cookie's proposals are P3, so none of them outranks
plan work. Dependencies are only the real ones (for example 8.2 waits for
6.5-6.7 and the harness-auth decision, then 8.3, 8.4, 8.5 in turn), so
`bd ready` shows every Known Issue at once. **Take
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
`brwr-rto`, `brwr-1fo`, and `brwr-992` come instead from a 2026-10-16
read-only security review of the host code at c908fc5. Treat them the same way.
Their line numbers predate step 6.6, so re-check them.

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
  adapter serialization, every non-expired untrusted install still validates
  afresh, and an expired install cannot alter the active revision's matcher.
- [x] `brwr-mwg` **Quadratic overlap scan on every host refresh** (host/src/config.rs
  ~950, ~1226). `read_disk_state` computes `overlap_report` on every Hello,
  GetConfig, LoadNote, SaveNote, and RecordVisit, and `adapter_matches`
  recompiles. Compute warnings only for GetConfig and post-commit, and reuse
  one compiled Document per revision. Fixed by keeping one validated Rust
  document on `ConfigStore` for each on-disk revision, reusing it across all
  event evidence checks, and running the pairwise warning scan only for
  settings reads or immediately after a successful commit.
- [x] `brwr-7sb` **AGENTS.md "Current state" is stale.** It says the worker lease, the
  TypeScript prefilter, and host recognition are unbuilt, but they shipped.
  Fixed by aligning AGENTS.md, README.md, and DESIGN.md with the shipped lease,
  defense-in-depth recognition, and hourly activity boundary, protected by a
  whitespace-tolerant CI documentation regression.
- [x] `brwr-5fg` Smaller ones from the same review:
  - In `setStarterButtonState` (options.ts), `offer?.adapter !== null` is true
    when `offer` is null.
  - `hasAdapter` (starter-adapters.ts) can't see an existing GitHub adapter
    until an org scope is typed.
  - `StoredAdapters` uses `deny_unknown_fields`, so a newer `[adapters]` key
    makes the whole config unreadable (backup-and-rewrite) instead of just
    suspending adapter policy.
  Fixed all three: starter availability now requires a real draft, installed
  GitHub types are detected without typed scope, and future adapter-table keys
  are preserved host-side as an adapter-only issue with explicit reset repair.
- [x] `brwr-rto` **Restrict trail transitions to browser values.** Confirmed
  the gap with a failing native-delivery regression, added the allowlist plus
  `forward_back`, and retained older bounded activity without deriving unknown
  edges. Both single and batch delivery terminally reject invented values.
- [x] `brwr-1fo` **Bidi and zero-width characters spoof log rows.**
  `safe_markdown_line` strips control chars but not Unicode format chars
  (U+202E, U+200B–U+200F, U+2066–U+2069, U+FEFF), so a hostile title can
  visually disguise an entry. Step 6.6's artifact renderer
  (`index.rs::safe_markdown`) has the same gap. Cover both with one shared
  sanitizer. Fixed with a shared bounded presentation sanitizer for both
  delivery paths, YAML artifact titles, Markdown context, and plain worklogs;
  exact encoded payloads and source logs survive regeneration unchanged.
- [x] `brwr-992` **Activity checks are duplicated across delivery paths.**
  Confirmed duplication and made single delivery reuse `check_activity_policy`.
  No current behavioral divergence reproduced: 13-scenario parity passes both
  before and after consolidation, with explicit policy outcomes and real writes.
- [x] `brwr-ee5` **Recover missing index ingestion from durable logs.**
  Reproduced the missing-B failure and fixed it with bounded per-file log
  checkpoints checked before delivery and query use, including after restart.
  Durable capture stays acknowledged during cache failure; history APIs report
  unavailable until catch-up and publication succeed. Unchanged logs are skipped,
  partial trailing markers are revisited, and old caches rebuild automatically.
- [x] `brwr-ezb` **Keep artifact notes reachable after adapter or site removal.**
  Verified native save/revisioned removal/load across adapter removal, rule
  narrowing and site removal; recovered notes retain separate older URL notes
  on all recorded routes, including mode and context-only evidence. Cache
  deletion/reindex, stale keys and versions, ambiguity and capture suppression
  pass without automatic merges or overwrites.
- [x] `brwr-t61` **Refuse incomplete note scans before selecting an autosave target.**
  Discovered during the removal review: primary recovery now refuses truncated,
  oversized or unreadable candidate sets. Four injected scan faults preserve
  note bytes and recover after cleanup; optional older-note lists stay usable.
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
- [x] **M1.5a step 6.5 — Delivery and durability.** Current delivery
  now groups same-day records into bounded, replay-stable batches backed by
  one compact intent and append. Pressure coalescing, the 80% badge, durable
  overflow evidence, and near-repeat retirement are built and verified.
- [x] **M1.5a step 6.6 — Index tables and artifact files.** Durable
  activity now updates disposable `artifacts`, `visits`, `edges`, and dirty-set
  SQLite tables without touching the notes folder. Connection-close passes
  regenerate ownership-checked, skip-if-unchanged artifact Markdown with
  case-safe paths and bounded Timeline rollups; `brauser reindex` recreates
  both cache and files from bounded owned log markers.
- [ ] `brwr-b2e` **Replace the hand-written SQLite snapshot with an incremental
  real-SQLite index.** Step 6.6 shipped a custom SQLite file writer
  (`build_sqlite`) because the sandbox can't fetch crates. Every delivery
  re-derives all history, rewrites a whole-history `index-state.json` (64 MiB
  cap), and rewrites all of `index.db`. Move to `rusqlite` (bundled) with
  incremental upserts in transactions, keeping the log authoritative and
  `brauser reindex` as the rebuild. This is required for M3 FTS5. It is
  blocked on **[human]** `brwr-mff`, which adds the crate outside the sandbox.
- [ ] `brwr-mff` **[human] Add `rusqlite` (bundled) to the workspace and warm
  the VM's cargo cache.** Update `Cargo.toml` and `Cargo.lock`, pass
  `cargo deny`, and run `cargo fetch` on the VM so in-sandbox builds work
  offline.
- [ ] `brwr-7vf` **index.db across concurrent host processes.** The panel,
  settings, and every worker sync each start a host, so `index.db` gets
  several openers at once. Use WAL, a busy timeout, and one writer at a time
  for index updates and dirty-set regeneration. Test concurrent hosts with no
  surfaced `SQLITE_BUSY`, no lost or duplicated rows, and no double
  regeneration.
- [ ] `brwr-5yn` **[human] M1.5a step 6.8 — Certify the three starter adapters.** Check
  the Jira, SharePoint/OneDrive, and GitHub drafts against each live tool and
  record the date in `docs/adapters/`. Needs John on macOS with real accounts;
  Cookie must not change the "development draft" label or mark 6.8 done.
- [ ] `brwr-1hj` **Capture Site (step 7.6 moved forward).** A side-panel
  button, plus the existing context-menu and toolbar entry points for an
  ungranted tab, that learns the site's artifact pattern with the structural
  drafter (no agent) and every step 7.6 safety rule. The native confirmation
  stays, in plain language: what Brauser will remember here, with real example
  pages it matched, and what it won't (search, dashboards, login, sharing).
  The JSON editor becomes an advanced view nobody has to open. Tier 1 is not
  required. Split into ordered sub-items here if it's too big for one cycle.
- [ ] `brwr-6x2` **Re-learn a site with automatic re-keying.** Users can't
  hand-fix a wrong machine-drafted ID. Add a Re-learn action that migrates
  existing activity, notes, and index rows through explicit append-only
  alias/re-key events (the needed part of step 7.2). This replaces the
  conservative ID freeze instead of relaxing it.
- [ ] `brwr-bzx` **Windows CI.** Add a `windows-latest` job for the host build,
  tests, and clippy, plus the extension checks. Fix code and tests that have
  assumed macOS/Unix since 2026-10-15, and keep the existing Windows code
  paths. Interactive Windows checks are separate **[human]** work.
- [ ] `brwr-i1g` **[human] Accept the core Chrome workflow on Windows.** After
  Windows CI, record an isolated real-Chrome run with source and OS/browser
  versions, native registration, picker/consent and first grants, background
  capture, Pause/revocation, notes, Recent/worklog and later Capture Site.
  Windows remains unvalidated until its own acceptance is recorded.
- [ ] `brwr-9yc` **[human] Decide how a Chrome-spawned host authenticates the
  AI harness** (DESIGN §14 open M2 question; it affects John's Bedrock setup).
  Cookie may write an options proposal in §14; John decides. Blocks 8.2.
- [ ] `brwr-6wz` **M2 step 8.2 — Host invocation.** Start only after 6.4–6.7 are shipped.
  Allowlisted environment, templated argv, page content on stdin inside the
  untrusted-data block, host-owned working directory, timeout, output cap,
  process-tree kill, and the confirmed-install-location check (§7.1), with the
  fake-harness tests and the §7.1 canary listed in step 8.2.
  The authoritative queue also waits for safe Re-learn, browser interaction
  stabilization, and the harness-auth human decision.
- [ ] `brwr-ooy` **Bound AI harness execution on Windows.** Before Windows M2
  acceptance, prove the same cleared environment, private work directory,
  bounded output, timeout/cancellation and process-tree containment with fake
  executables in Windows CI. A Windows build alone is not harness support.
- [ ] `brwr-jfe` **Concurrent native message loop (before 8.3).** Refactor
  `serve_with_context` into a stdin reader thread, a worker for long requests,
  and one serialized writer, reusing `run_bounded`'s thread/mpsc pattern. No
  protocol change. A short request must be answered while a long one runs,
  and stdin EOF must tear both down.
- [ ] `brwr-0cj` **M2 step 8.3 — Streaming protocol.** A reader that stays responsive
  while a harness runs; chunk, `rejected`, error, `cancel`, and
  `summary_result` messages correlated by `request_id` under the 900 KiB cap;
  stdin EOF kills the harness. Takes the next protocol version after v5.
- [ ] `brwr-f45` **Agent-refined Capture Site (step 7.7 moved forward).** After
  8.2, the configured harness can refine the structural draft under the same
  isolation, with the same per-rule checks and plain-language confirmation.
  Capture Site must keep working with no agent configured.
- [ ] `brwr-0ij` **M2 step 8.4 — Content extraction.** Bundled, pinned Readability-style
  extraction, the `summarize` command, context menu, and toolbar trigger, and
  the §8 permissions. **[human]** sub-step first (`brwr-rva`): the real-Chrome `activeTab`
  checks (a)–(d) in step 8.4 need separate macOS and Windows evidence; build
  the rest behind those and streaming. Reuse scripting from Capture Site or
  add it if absent; the other M1.5b work is not a prerequisite.
- [ ] `brwr-deo` **M2 step 8.5 — `/summarize` in the panel.** Denylist checks, the §5.2
  and §4.4 summary write and result contract, the §3.1 extension-page CSP with
  a manifest test, and the external-editor race, shared summaries directory,
  and mid-run config-change tests.

### Gated plan work (do not start yet)

- **M1.5b (§12.2 step 7.1–7.5)**: the rest of aliases, Tier 1 fields
  (`scripting`), worklog files, and the remaining starter adapters. Create
  adapter (7.6) and agent drafting (7.7) moved into Plan work above as
  Capture Site (`brwr-1hj`, `brwr-f45`), along with the re-keying part of
  7.2 (`brwr-6x2`). DESIGN.md gates the rest on M1.5a being in daily use for
  a few weeks; John will lift the gate here when it's time.
- **M3 — Related** (FTS5 in `brauser reindex`, related pages, read later),
  **M4 — Omnibar** (built-in commands, free text, custom commands), and
  **M5 — Release** (installer, store listing, signing and provenance, docs)
  follow M2, per the §12 table. M5 signing and store work is **[human]**.

### Focused quality follow-up

- [ ] `brwr-x0d` **Share list and streaming-text rendering when a consumer needs it.**
  Recent and worklogs already shipped without the proposed helper. Keep this
  P3: extract a small framework-free primitive only when Capture Site or
  streaming demonstrates repeated rendering needs; do not rewrite the panel
  solely to create an abstraction.
- [ ] `brwr-9xg` **Exercise artifact panel interactions in headless Chrome.**
  Extend the existing built-page smoke check so it opens the artifact card,
  saves context, filters Recent, changes and copies a worklog, migrates an
  older note, and refreshes after background delivery. Cover close/reopen
  during debounce and an in-flight save, retaining any lost edits in durable
  recovery. This P2 stabilization gate precedes M2; use only isolated local
  fixtures and preserve setup smoke and dialog counts.
- [ ] `brwr-4vb` **Make the dialog reply test consume its request.** The fake
  child can exit after printing its reply before the parent writes the framed
  request, intermittently producing `EPIPE` even though production dialog
  children read the request first. Make the fixture obey the real handshake
  and exercise it repeatedly; this was discovered during the current cycle's
  final Rust verification and does not affect the shipped worker change.
- [ ] `brwr-m64` **Deliver overflow evidence after the final batch drains.** A
  drop that races the last in-flight batch stays counted and visible, but its
  marker currently waits for another authorized event to provide a safe log
  anchor. Make that evidence independently deliverable without accepting an
  unvalidated URL or vault path.
- [ ] `brwr-o1l` **Share one authorize-and-open path across delivery entry
  points.** `record_visit`, `record_activity`, and `record_batch` repeat the
  lock, refresh, policy, and store setup, and `capture.rs` has twin
  mode/trail authorization loops. Extract the shared steps and set a removal
  release for the legacy paths.
- [ ] `brwr-ahb` **One adapter identity for consent and transition checks.**
  `consent.rs` and `config.rs` compute identity separately, so the
  confirmation text could drift from what is enforced.
- [ ] `brwr-2iz` **Bound `adapter_id_history` inside the 64 KiB config cap.**
  When it fills, unrelated writes fail with a generic size error. Add a
  budget, a specific error, and a safe prune, or fold it into `brwr-6x2`.
- [ ] `brwr-m68` **Split DESIGN.md into a spec and an as-built history.** Move
  dated as-built and probe records to `docs/history.md`, and make AGENTS.md
  "Current state" a short table.
- [ ] `brwr-3cc` **Preserve single-event recovery across presentation changes.**
  Legacy intents retain a rendered-entry hash rather than prepared bytes.
  Independent review added exact-hash compatibility for this sanitizer
  upgrade; future rendering changes still need recovery independent of the
  renderer, with the same integrity checks. Compact batches retain their bytes.

### Cookie's own proposals (only after the plan work above)

- [ ] `brwr-r88` **Show ambiguous historical notes for explicit read-only viewing.**
  When several artifacts share a canonical URL after removal, list the
  verified preserved notes in the panel so a person can read one deliberately.
  Keep the primary autosave key, separate URL note, ownership checks and
  capture policy unchanged; never turn a read-only choice into a migration.
- [ ] `brwr-1g2` **Show the incoming trail on the artifact card.** The card
  currently shows only a linked count even though the host records validated
  cross-tool edges. After ordered plan work, surface bounded incoming sources
  with a title, honest navigation wording and explicit safe links, following
  DESIGN.md §5.7. Keep back/forward and missing sources honest and never turn
  unknown historical transition text into a relationship.
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

- None. `brwr-ezb` and its discovered scan-completeness fix `brwr-t61` are
  complete after three failing-first regressions, a fresh full diff review and
  the full gate (332 Rust tests, eight extension test files). Follow `bd ready`
  next; Capture Site remains the major product investment behind its existing
  queue prerequisites and the P3 proposals stay behind ordered plan work.

## Shipped

- **Category:** Bug Fix. **Keep shared and older notes reachable after capture rules are removed (2026-10-27).** Historical note recovery now retains the older URL-note list and its read-only viewing action after adapter removal, recognition narrowing and site removal. Both URL lookup directions share durable activity, mode and explicit-context evidence, so alternate routes and context-only history recover the same note without inventing activity. Native request tests prove separate URL notes, migration refusals, stale-key copy-out, cache deletion/reindex and suppressed capture preserve existing content. **Metrics:** 332 Rust tests (+5) and 8 extension test files passing; full required build/quality gate and both Clippy configurations green; macOS smoke unavailable on Linux, 14 scripted dialogs unchanged.
- **Category:** Security & Privacy. **Refuse incomplete historical note scans before autosave (2026-10-27).** Review of note removal reproduced a partial candidate scan selecting a note despite an omitted conflicting identity. Primary recovery now fails closed at the file-count, per-file byte or total-byte limit and on unreadable regular candidates; optional older-note discovery remains best effort. Native loads and saves return correlated unavailable-identity errors, preserve note bytes and recover once the fault clears. **Metrics:** 332 Rust tests and 8 extension test files passing; 4 scan-fault cases within 1 new regression, plus activity/context ambiguity and cache-failure coverage; full gate green.

- **Category:** Bug Fix. **Recover missing artifact history from durable logs (2026-10-27).** Per-file ingestion checkpoints now catch up valid older caches before delivery updates and history queries, including after restart without replaying the missing event. Changed logs reparse in full, unchanged logs are skipped, SQLite publishes before the checkpoint cache, and all cache work shares the configuration lock. Durable capture stays acknowledged during cache failures, while artifact, Recent, and worklog queries report unavailable history instead of silently incomplete totals; bounded failures publish no partial checkpoint. **Metrics:** 326 Rust tests (+4) and 8 extension test files passing; full required build/quality gate and both Clippy configurations green; macOS smoke unavailable on Linux, 14 scripted dialogs unchanged.

- **Category:** Reliability. **Share activity authorization across single and batch delivery (2026-10-27).** Single activity delivery now calls the same host policy checker as batches under the existing configuration lock, removing duplicate identity, mode, trail, reference, and shape checks. A 13-scenario parity test verifies explicit outcomes and reasons, refusal before log/index writes, and exact decoded durable payloads for valid activity through both paths. The baseline already agreed, so no existing behavioral bypass is claimed. **Metrics:** 322 Rust tests and 8 extension test files passing; 1 new test covering 26 deliveries; full required build/quality gate and both Clippy configurations green.
- **Category:** Security & Privacy. **Keep invisible formatting out of generated work history (2026-10-27).** A shared host sanitizer now replaces Unicode format and control characters with spaces in daily row titles/context, regenerated artifact titles/context, and plain worklogs, and escapes Obsidian comments, highlights, and tags in generated Markdown. Both single and batch delivery use it while canonical encoded event payloads stay exact; reindex regenerates safe owned files without changing source logs. The review recorded legacy interrupted-entry recovery across renderer upgrades separately as `brwr-3cc`. **Metrics:** 319 Rust tests (+4) and 8 extension suites passing; two failing-first regressions, all 170 Unicode 17 format scalars verified locally, and full protocol generation, current-state consistency, build, format, both clippy gates, clean npm install, typecheck, lint, and extension build/tests green.
- **Category:** Security & Privacy. **Reject invented navigation trails while preserving older activity (2026-10-27).** The native host now restricts new trail transitions to Chrome's known values plus the worker's existing back/forward encoding, returning terminal `rejected` before log or index writes through both single and batch delivery. Older bounded log and cache payloads retain their measurements and exact replay identity, while unknown transition kinds cannot become derived edges; reindex keeps the original shape limits and leaves source logs untouched. **Metrics:** 314 Rust tests (+3) and 8 extension suites passing; full protocol generation, current-state consistency, build, format, both clippy gates, clean npm install, typecheck, lint, extension build/tests, and queue consistency green.
- **Category:** New Feature. `brwr-m40` **Turn artifact history into a daily workspace (2026-10-17).** Recognized pages now show one shared artifact card across alternate routes, including measured focus/background/open totals, mode, links, and append-only “Why I'm here” context. A filtered Recent tab combines artifact-day history across tools and generates copyable today, yesterday, or current-week worklogs without an agent; host-derived record keys make Google Docs and Jira modal routes share notes, preserve pre-adapter notes for explicit no-clobber migration, and copy out edits when adapter identity changes mid-save. Settings can also regenerate artifact files on demand and names every unowned path it refuses to overwrite. **Metrics:** 293 Rust tests and 8 extension suites passing; 9 new Rust and 2 new extension regressions, with full protocol generation, current-state consistency, build, format, both clippy gates, clean npm install, typecheck, lint, extension build/tests, and queue export green.
- **Category:** New Capability. **Turn durable activity into a local index and bounded artifact records (2026-10-16).** Each accepted activity now updates a standard SQLite `index.db` with artifact, visit, edge, and dirty-set tables while leaving the append-only Markdown log authoritative. Brauser lazily regenerates only dirty, owned artifact files at native connection boundaries, preserves unchanged mtimes, gives case-colliding IDs distinct hash-suffixed paths, replaces external edits only after checking ownership, neutralizes hostile presentation text, and bounds long histories with daily, weekly, then monthly Timeline rollups. `brauser reindex` reconstructs the database and files from bounded owned markers, and older configs receive a non-overlapping `artifacts_dir` default without weakening native consent. **Metrics:** 284 Rust tests and 8 extension suites passing; 13 new index, ownership, delivery-integration, migration, replay-integrity, multi-page SQLite, chronology, rollup, and wire regressions, with full protocol generation, build, format, both clippy gates, clean npm install, typecheck, lint, extension build/tests, and queue export green.
- **Category:** Reliability. `brwr-6at` **Ship bounded, crash-recoverable activity batches (2026-10-16).** The service worker now sends up to 64 same-local-day records under a replay-stable batch ID and exact overflow snapshot, while the native host validates every event before publishing the survivors through one compact intent, one append, and one log sync. Queue pressure coalesces only compatible undispatched hours that still fit the hard byte cap, lights the toolbar badge at 80%, and carries dropped-hour count and range into the append-only log; lost acknowledgements cannot erase newer overflow. The retired near-repeat window is gone from protocol, consent, new config writes, and log scanning, so deliberate `/log` actions are never collapsed, while old configs and queues still drain safely. **Metrics:** 271 Rust tests and 8 extension suites passing; 6 durability regressions, with full protocol generation, current-state consistency, build, test, format, both clippy gates, clean npm install, typecheck, lint, extension checks, and queue export green.
- **Category:** Bug Fix. **Finish the remaining adapter review repairs (2026-10-16).** Starter cards now recognize an installed GitHub pull-request or issue adapter before any organization scope is typed, and a missing draft can no longer become clickable through JavaScript's `undefined !== null` edge. The native host treats unknown future keys inside `[adapters]` as an adapter-only policy issue instead of an unreadable whole config: capture stays suspended, notes and other settings remain usable, unrelated saves preserve the unknown values, and the explicit remove-all repair clears them. **Metrics:** 267 Rust tests and 8 extension suites passing; 3 new failing-first regressions, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Documentation & Process. **Align contributor status with shipped artifact capture (2026-10-16).** AGENTS.md now describes the active revisioned worker lease, TypeScript prefilter, independent native-host recognition, and measured hourly activity instead of telling contributors those paths are unbuilt. README and DESIGN.md name the same product boundary and separate it from the unfinished delivery, index, and panel slices; a whitespace-tolerant CI check prevents the obsolete claims from returning. **Metrics:** 266 Rust tests and 8 extension suites passing; 1 new documentation regression, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Performance & Resource Bounds. **Compile native adapter policy once per configuration revision (2026-10-16).** The native host now keeps one validated Rust adapter document with each adopted config revision and reuses it for visits, hourly activities, mode and trail evidence, and why records instead of reparsing the complete policy for every check. Unchanged Hello, note, capture, and settings refreshes retain the compiled document; conservative pairwise overlap warnings run only when settings requests them or immediately after a successful commit, while invalid revisions still suspend capture fail-closed. **Metrics:** 266 Rust tests and 8 extension suites passing; 1 new failing-first compile/overlap hot-path regression, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
- **Category:** Performance & Resource Bounds. **Compile adapter policy once per worker revision (2026-10-16).** The service worker now validates and compiles a stored adapter document once when its host revision is first restored, then reuses that prepared document across policy reads and artifact matches instead of rebuilding the grammar and serializing the full policy on every navigation. Every non-expired install message still compiles independently at the untrusted boundary, expired installs are rejected before they can replace the active matcher, malformed cold-start policy stays inert, and site and live-grant gates remain ahead of evaluation. **Metrics:** 265 Rust tests and 8 extension suites passing; 3 new cache and navigation-hot-path regressions including failing-first compile-cache and rejected-install proofs, with full protocol generation, build, test, format, clippy, clean npm install, typecheck, lint, extension checks, and queue consistency green.
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

- `brwr-ee5` is fixed with a reproduced missing-append regression. The cache
  remains a provisional whole-history snapshot; `brwr-b2e` now records that
  transactional SQLite ingestion should resume bounded batches across requests,
  rather than permanently stop at the current per-pass read cap. Preserve the
  new recovery and unavailable-history assertions when replacing it.
- `brwr-ezb` now passes the complete removal matrix. Historical identity uses
  the union of note and log evidence, including explicit context, and refuses
  incomplete sources (`brwr-t61`). `brwr-r88` is the visible follow-up: let a
  person read preserved ambiguous artifact notes without guessing a primary
  autosave key. It remains P3 behind plan work.
- Capture Site remains the next major visible product investment after its
  queued prerequisites. Re-learn and named focus sessions already cover the
  useful follow-ups; another internal health view would duplicate existing work.

- `brwr-992` is preventive consolidation, not a reproduced behavioral fix:
  the complete parity matrix passed before and after. Keep explicit expected
  outcomes in cross-path tests so two permissive paths cannot pass by agreement.
  `brwr-o1l` still owns the broader authorize/open deduplication; do not widen
  this cycle into that refactor. Its then-next deploy blocker `brwr-ezb` is now
  complete; continue from the authoritative ready queue.
- Product direction remains Capture Site (`brwr-1hj`): the confirmation should
  use concrete matched pages and plain exclusions so someone can start useful
  capture without understanding adapter JSON. Existing Recent/worklog and the
  incoming-trail proposal should explain remembered work, not add another
  internal status surface. No new duplicate proposal is needed this cycle.
- The queue database lacked the committed independent-review note on
  `brwr-3cc`. Restored it through `bd update` before export so the known
  sanitizer-upgrade repair is not lost; that P3 task stays open for future
  renderer-independent intent storage.
- The shared presentation sanitizer belongs at the output boundary: raw
  canonical events must remain usable for replay and rebuilding, while YAML,
  Markdown, and copyable worklogs need the appropriate plain or escaped
  rendering. Future field and relationship displays should reuse it instead
  of growing another sanitizer. `brwr-3cc` records the older single-entry
  intent limitation found during this review; complete markers still dedupe.
- After the remaining deploy blockers, `brwr-1hj` (Capture Site) is still the
  most direct product improvement: a person should be able to start useful
  capture from the page they are reading. The existing named-session and
  incoming-trail proposals can later explain why cross-tool work belongs
  together; another internal status view would not answer that need.
- Keep live-delivery validation separate from historical decoding whenever a
  new semantic constraint lands: preserving measurements does not require
  interpreting an unknown trail. `brwr-1g2` is the product follow-up that turns
  the already-valid relationships into useful context on the current card;
  it stays behind the ordered plan rather than extending this blocker fix.
- M1.5a steps 6.5–6.7 shipped, but the security-review blockers and the
  authoritative queue's derived-history follow-ups still precede new plan
  work. `brwr-npy` was already closed by source review on 2026-10-18 because
  macOS CI drives the built panel and settings; that is not a new browser run.
  `brwr-9xg` adds the missing artifact interactions. Starter certification
  remains behind its human/live-tool gate.
- The 2026-10-27 queue export carries forward existing database planning
  updates: `brwr-ee5`, `brwr-ezb`, `brwr-i1g`, and `brwr-ooy`; the existing
  `brwr-npy` closure; M2 dependency changes; and `brwr-x0d`'s P3 deferral.
  `brwr-b2e` remains the canonical SQLite replacement (`brwr-83m` is its
  closed duplicate). These are reconciled queue state, not work shipped in
  this cycle.
- Artifact-file conflicts are now visible from the settings regeneration
  action, including bounded relative paths, while the generator continues to
  leave those unowned occupants and their dirty records untouched.
- `brwr-9xg` follows the artifact workspace with browser-level interaction
  coverage for the composed panel. The Rust boundary tests, note-state test,
  and settings fake DOM do not prove the full card/filter/worklog click path;
  extend the existing headless smoke fixture rather than add another harness.
- An overflow that appears while the final queued batch is in flight is now
  preserved rather than over-acknowledged, but it needs a later artifact event
  before it has a validated log anchor. `brwr-m64` records the follow-up for an
  independently deliverable marker; do not solve it by letting the extension
  choose a file or smuggle an unchecked URL into the host.
- Step 6.6 can use the batch boundary as the natural dirty-index transaction:
  derive affected artifact IDs from the same canonical payloads after the log
  sync, but keep the Markdown markers as the rebuild authority so index loss
  or corruption never weakens the append-only source of truth.
- A configured GitHub starter can now identify itself without draft scope, but
  the scope input intentionally remains blank because pull-request and issue
  adapters may have different owner lists. The `brwr-ac1` coherent first-tool
  setup should make that distinction visible rather than guessing one shared
  editable value from existing policy.
- The current-state CI check now protects lease delivery, worker prefiltering,
  native-host rechecking, hourly activity, and batched durable delivery. When
  indexing or the artifact panel ships, update all three summaries and extend
  the protected fact set in the same cycle rather than weakening or deleting
  the check.
- The worker and host now use the same lifecycle rule for prepared adapter
  policy: a host configuration revision is the cache identity, and untrusted
  replacement input must validate before it can replace the active matcher.
  Keep future index and panel reads on this prepared host document rather than
  reintroducing snapshot-level compile helpers as new native messages arrive.
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
- Automatic capture now emits measured activity through bounded batches and a
  compact intent, while the legacy visit path exists only to drain pre-upgrade
  queue entries. Remove that compatibility only after old queues have had a
  release to drain.
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
