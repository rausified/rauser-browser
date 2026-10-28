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
capturing, with a plain-language review (a native confirmation until open
access, 2026-10-26). That moves Create adapter
(M1.5b step 7.6), the re-keying it needs, and later agent refinement (step
7.7) onto the critical path ahead of the rest of M2. Second, Windows support
is back in scope, starting with Windows CI. The order below already reflects
this. The goal is to get John using M1.5a daily as soon as possible, so the
worklog comes first within step 6.7. Treat new adapter-configuration hardening
findings as P3 unless they describe a concrete user-facing failure.

**Beads is the machine queue (2026-10-16).** `.beads/issues.jsonl` holds one
bead per open item below, structured like Privateer's queue. Known Issues
are P1 and plan work is P2. Cookie's proposals are P3, so none of them outranks
plan work. Dependencies are only the real ones (for example open access PRs
2, 3 and 4 follow each other in turn), so
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

**Open access and the Brauser service (John, 2026-10-26).** The four PRs of
`docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md` (plan: `docs/superpowers/plans/2026-10-26-open-access-and-agent-service.md`) replace the old M2 steps 8.2-8.5 and come first, in
order. They supersede the denylist, harness-token, `activeTab`, and per-site
grant parts of the items below wherever those conflict. PR 1 (open access
and the protocol v6 removals) and PR 2 (the service skeleton) are done,
PR 3 (real runners, extraction, and the run card) is merged (#98), and PR 4
(summary outputs) is built on `feat/summary-outputs-pr4`; see Shipped.

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
  regeneration. **Dependency-blocked:** `brwr-b2e` first requires human
  `brwr-mff`; bundled `rusqlite` is still absent from the manifest, lockfile,
  and local crate cache as of `5fb15210` (2026-10-28).
- [ ] `brwr-5yn` **[human] M1.5a step 6.8 — Certify the three starter adapters.** Check
  the Jira, SharePoint/OneDrive, and GitHub drafts against each live tool and
  record the date in `docs/adapters/`. Needs John on macOS with real accounts;
  Cookie must not change the "development draft" label or mark 6.8 done.
- [ ] `brwr-hq8` **Plan re-learning impact before migrating identities.**
  Next prerequisite of `brwr-6x2`: derive a revision-bound plan from recorded
  evidence; report splits, merges and key reuse, reject insufficient query
  provenance, and inspect existing notes/summaries without changing them.
  The offline rehearsal is complete; durable apply remains later.
- [ ] `brwr-i1g` **[human] Accept the core Chrome workflow on Windows.** After
  Windows CI, record an isolated real-Chrome run with source and OS/browser
  versions, native registration, the folder picker, standing HTTP(S) access,
  background capture, Pause, notes, Recent/worklog and later Capture Site.
  Windows remains unvalidated until its own acceptance is recorded.

### Gated plan work (do not start yet)

- **M1.5b (§12.2 step 7.1–7.5)**: the rest of aliases, Tier 1 fields
  (`scripting`), worklog files, and the remaining starter adapters. Create
  adapter (7.6) and agent drafting (7.7) moved into Plan work above as
  Capture Site (shipped; agent refinement `brwr-f45` remains), along with the re-keying part of
  7.2 (`brwr-6x2`). DESIGN.md gates the rest on M1.5a being in daily use for
  a few weeks; John will lift the gate here when it's time.
- [ ] `brwr-ooy` **Windows equivalent of the Brauser service (deferred).** The
  cleared-environment harness lockdown it described is superseded by open
  access. Agent runs live in the macOS-only Brauser service, which compiles
  out elsewhere; the spec defers a Windows service. When Windows agent support
  is scheduled, John approves a Windows design against the spec first.
- **M3 — Related** (FTS5 in `brauser reindex`, related pages, read later),
  **M4 — Omnibar** (built-in commands, free text, custom commands), and
  **M5 — Release** (installer, store listing, signing and provenance, docs)
  follow M2, per the §12 table. M5 signing and store work is **[human]**.

### Focused quality follow-up

- [ ] `brwr-x0d` **Share list or run-card text rendering when a second consumer needs it.**
  Step 6.7 shipped and Recent, the worklog and the artifact card did so
  without a shared helper. Open access runs are polled through `get_run`
  (spec §5.3), not streamed, so any text helper is an append of polled
  run-card text. Keep this P3: extract a small framework-free primitive only
  when Capture Site or the open access PR 3 run card repeats rendering the
  panel already has; do not rewrite shipped views solely to create an
  abstraction. Close it as not needed if no second consumer appears by PR 3.

- [ ] `brwr-o1l` **Share one authorize-and-open path across delivery entry
  points.** `record_visit`, `record_activity`, and `record_batch` repeat the
  lock, refresh, policy, and store setup, and `capture.rs` has twin
  mode/trail authorization loops. Extract the shared steps and set a removal
  release for the legacy paths.
- [ ] `brwr-m68` **Split DESIGN.md into a spec and an as-built history.** Move
  dated as-built and probe records to `docs/history.md`, and make AGENTS.md
  "Current state" a short table.

- [ ] `brwr-ppn` **Curate a worklog before copying.** Select artifact-day rows
  and preview the exact text to share without altering measured activity;
  preserve selections across refreshes. This remains behind the ordered plan.

### Confirmed low-severity defects (lane C review, 2026-10-27)

A 2026-10-27 review of main at 1c32114 confirmed each of these with a probe
test that fails on that commit. None blocks deploy, so they are P3 bugs here
rather than Known Issues, and they wait behind the plan work above. Line
numbers are on 1c32114; re-check them against current main. For each one,
write the regression test described in its bead first, show it fails, then
fix, and keep each fix to one cycle. If the test does not fail on current
main, check the item off as "not reproduced" and say why. The decided
strip_params note-identity follow-up has shipped (see Shipped).

### Cookie's own proposals (only after the plan work above)

- [ ] `brwr-wg5` **Bound adapter comparisons when sandbox child pipes are denied.**
  The optional 5,000-case differential check stalled; a one-case probe timed
  out after 30 seconds, and the child-pipe guard reports EPERM. Establish the
  exact wait and reuse explicit sandbox diagnostics with a bounded runner;
  required CI mode must still fail on denial. No engine changed this cycle.

- [ ] `brwr-7lq` **Restore overlapping recorded container groups with explicit identity review.**
  The removed-site flow refuses overlapping repository/PR history because it
  cannot establish one note identity. Extend the host's impact/alias planning
  (`brwr-6x2`) with a deliberate review that retains notes and considers
  intervening repository-only capture; never relax the freeze or infer a
  primary key from a canonical address.

- [ ] `brwr-ued` **Group cross-tool work into named focus sessions.** Let a
  person start and end one explicit local session from the panel so a Jira
  issue, design document, and pull request can appear together under the
  reason they were opened. Carry the stable session through worker restarts
  and measured activity into the append-only log and derived worklog; never
  infer a session from browsing or send it to a service. This should build on
  the artifact card and worklog after M1.5a rather than delay those foundations.
- [ ] `brwr-jnj` **Offer non-destructive adapter document repair.** The immediate repair
  path removes the entire invalid document. Add copy-out and a whole-document,
  format-1 replacement workspace so a user can salvage valid rules without
  asking an older build to interpret or migrate an unsupported future format.

## In Progress / This Iteration

- [ ] `brwr-6x2` **Re-learn capture while preserving artifact history and notes.**
  Cookie claimed the highest-priority ready item after the coordinator cleared
  the assigned queue. The [four-stage plan](docs/superpowers/plans/2026-10-28-capture-relearning.md)
  starts with offline alias rehearsal, then revision-bound impact planning,
  durable migration/rebuild, and the panel review. The parent remains in
  progress: no migration is enabled and the ID freeze stays enforced.


- In progress in the Lane A session, not Cookie. Cookie must not claim these:
- [ ] `brwr-f45` **Agent-refined Capture Site (step 7.7 moved forward).**
  Structural first, agent second: for unknown sites, "Improve with AI"
  refines the structural draft by launching an agent job through the Brauser
  service (the open access PR 2 service, shipped in #75, and PR 3 runners, shipped in #98;
  spec above), not the old M2 step 8.2 harness path. Agent drafts are
  untrusted and get the same per-rule host checks and plain-language review.
  Capture Site must keep working with no agent configured.
  As built (2026-10-28, `feat/capture-site-agent`): DESIGN.md §5.1 agent
  drafter bullet and §12.2 step 7.7; plan
  `docs/superpowers/plans/2026-10-28-capture-site-improve-with-ai.md`.

- [ ] `brwr-9xg` **Exercise artifact panel interactions in headless Chrome.**
  **Blocked on platform execution; implementation complete.** The built-page
  smoke covers artifact/context interactions, Recent filters, worklog Copy,
  older-note migration, background refresh and close/reopen recovery during
  debounce and an in-flight save. Run `npm run smoke:macos:headless` on an
  isolated macOS checkout, retaining the existing scripted-dialog assertions,
  and record the tested commit and result before closing. On this Linux VM
  the runner refuses before setup and no Chrome executable was found on PATH;
  mocked panel regressions cannot supply browser acceptance. Do not rebuild
  this implementation or port the runner merely to replace macOS evidence.

- [ ] `brwr-bzx` **Restore Windows CI for the native host and extension.**
  **Blocked on current platform results; implementation and skip audit
  complete.** The reusable `build-os.yml` already covers Windows/macOS Rust
  build, tests, both Clippy builds and extension checks. Operator evidence
  records Windows green at `5a80dda` (run 40481905465); that historical result
  does not establish current status. POSIX permission, symlink/inode, shell
  and macOS launchctl/plist exclusions have documented reasons. The remaining
  action is to record current Windows and macOS green CI against a named
  commit, then close. This Linux cycle cannot access those runners or live
  APIs. Keep interactive Windows acceptance `brwr-i1g` open for a human.


Completed foundations: Open access PR 1 (`brwr-oa1`, #69) and PR 2 (`brwr-sv2`, #75) are merged, with PR 2's launchd checks left as **[human]** work, and PR 3 (`brwr-sr3`, #98) is merged with its real-harness checks left as **[human]** work; open access PR 4 (`brwr-so4`) is built on `feat/summary-outputs-pr4`, headless smoke (`brwr-t53`) included, with its Obsidian and VS Code preview check left as **[human]** work. `brwr-5yn` remains a human-only live-tool gate.

## Shipped

- **Category:** New Capability. **Restore removed sites from their recorded capture rules (2026-10-28).** Completed assigned `brwr-cua`: Capture Site reviews usable recorded page kinds and account scope before a new starter or draft, then re-reads the set before an explicit revision-checked save. Custom identities, note bytes, other sites and global pauses stay intact; no page content or agent is needed. Damaged or overlapping history remains refused, with container identity review tracked in `brwr-7lq`. **Metrics:** 997 workspace Rust outcomes (+7 regressions; 43 documented IPC sandbox skips), 11 new panel cases and all 15 extension files, both Clippy configurations and 859 scripted-build outcomes green.

- **Category:** New Capability. **Review capture gaps before sharing worklogs (2026-10-28).** Completed assigned `brwr-9r7`. The worklog reviews durable overflow ranges and missing activity-hour record counts, with a checked-by-default choice to include an incompleteness note in the exact copied preview. Local-date intersections preserve full ranges/counts, overlapping notices stay separate, and retries deduplicate without inventing work. Attached and standalone markers survive restart, bounded cache upgrades and reindex; stale or failed reports cannot leave Copy enabled. **Metrics:** 990 workspace Rust outcomes (+3 regressions; 43 explicit IPC sandbox skips), six new panel cases and all 15 extension files, both Clippy configurations, 853 scripted-build outcomes, two script files and 10,696 inert Markdown renderings green.


- **Category:** New Capability. **Preview artifact identity corrections offline (2026-10-28).** `brauser adapters aliases-preview` derives endpoint identities from supplied old and proposed adapter documents, then shows canonical groups and pending suggestions for confirmed, suggested and unalias operations. Chains, moves back and pairwise corrections replay deterministically; forged IDs, out-of-scope endpoints, cross-type links and insufficient stripped-query evidence refuse the rehearsal. It reads no settings or vault and grants no migration authority. Completes `brwr-sq9`; parent Re-learn (`brwr-6x2`) remains in progress for impact planning, durable migration and panel review. **Metrics:** 987 reported Rust passes (+15 tests, including 3 real CLI isolation cases; guarded IPC skips included), 15 extension files, both Clippy configurations and all required/README gates green.


- **Category:** Bug Fix. **Recover older activity and context without rewriting history (2026-10-28).** Historical uppercase and unhyphenated UUID payloads now survive reindex with canonical cache identities, preserving measured work and Why context. Older v1 interrupted appends reconstruct the original queued UUID spelling across all three historical renderers and resume only with the exact intent hash and torn tail; edited tails and mismatched payloads remain refused. Completes assigned `brwr-smu`. **Metrics:** Two failing-first regression matrices (33 cases); 972 workspace Rust outcomes (43 IPC skips), 835 scripted-dialog outcomes (53 IPC skips, one ignored doctest), 15 extension files, two script files, and 10,696 Markdown renderings; full required/README gate green.

- **Category:** New Capability. **Keep stripped query names beside saved page addresses (2026-10-28).** Host-derived, names-only evidence now accompanies daily-log URLs (including activity modes and trail sources), cache format 7, a queryable SQLite provenance table, and note/summary creation addresses. Older-note discovery and migration refuse candidates that lost a query or absence key read by a current or recorded contract, while tracking-only removal stays usable. Legacy evidence remains unknown; note re-keying and later saves preserve it, and frozen v1/v2 recovery completes only original bytes while new v3 intents bind the names. Completes assigned `brwr-srp`. **Metrics:** 10 new regression tests; 968 workspace Rust outcomes (43 IPC skips), 831 scripted-dialog outcomes (53 IPC skips, one ignored doctest), 15 extension files, two script files, 10,696 Markdown renderings; full required/README gate green.


- **Category:** User Experience. **Review recovered notes one copy at a time (2026-10-28).** Completed assigned `brwr-kws`. Each recovered copy now shows its page address, reason and read-only text with individual Copy and Discard controls; empty edits are explicitly labelled. Copy preserves exact text, and Discard selects an immutable snapshot ID, leaving live typing, other copies and newer snapshots from another panel intact. Clipboard and storage failures remain visible and retryable, and review never fills or saves the editor automatically. **Metrics:** Six new regressions; 34 editor and 163 built-panel cases, all 15 extension files, 915 exercised Rust tests plus 43 explicit IPC skips, two script files, both Clippy configurations and 10,696 Markdown renderings pass.

- **Category:** Security & Privacy. **Keep daily-log titles and context inert in Markdown (2026-10-28).** Completed assigned `brwr-2za`. New visit/activity titles and Why text use the shared inline escaper so page-supplied math and code stay text. Both historical v1 renderer generations are frozen for recovery, with whole-entry hash and exact torn-byte checks; machine markers and existing rows remain unchanged. **Metrics:** Two new Rust regressions cover 15 delivery/recovery scenarios; all 54 capture tests, 933 workspace Rust outcomes, 821 scripted-dialogs outcomes (plus one ignored doctest), 15 extension files, two script files and 10,696 Markdown renderings pass.

- **Category:** User Experience. **Explain why the current page is outside capture (2026-10-28).** Completed assigned `brwr-vny`. The panel distinguishes host and browser-local pauses, pending site removals, workspace/account scope exclusions and unmatched page rules, with settings or the existing Capture Site review as the next action. It reuses the revision-cached policy compiler and ordered evaluator, follows navigation and policy/status changes, clears on unreadable/private tabs, and never inspects links or changes capture or note identity. **Metrics:** 14 new composed-panel cases; all 158 panel cases and 15 extension files pass; 956 workspace Rust outcomes (43 explicit IPC skips), scripted-dialogs suite, full required/README gates and 10,696 inert Markdown renderings green.

- **Category:** Testing & Verification. **Isolate scripted host lifecycle configuration across platforms (2026-10-28).** Completed assigned `brwr-3rt`. A debug scripted-dialogs-only absolute config-path override keeps both host picker fixtures and their cache inside temporary directories despite Windows Known Folders ignoring HOME. Ordinary builds ignore the override, invalid overrides refuse without fallback, and release builds retain the feature prohibition; macOS smoke defaults and dialog counts are unchanged. **Metrics:** Four process-boundary regressions pass in scripted builds; ordinary-build exclusion passes; all seven scripted picker lifecycle tests and the full required/README gates pass. Windows execution remains with CI.

- **Category:** New Capability. **Browse preserved artifact notes without changing autosave (2026-10-28).** Completed assigned `brwr-r88`. Added an explicit panel disclosure listing verified note titles and artifact IDs when multiple historical notes share a stored address. Each opens in a separate text-only, copyable read-only viewer; the occupied URL note, primary autosave key, migrations and capture policy stay unchanged. The host rechecks ownership and address on open, uses the existing bounded scan with at most 24 results, and works without the history cache or recorded contracts. **Metrics:** 955 Rust outcomes (43 explicit IPC skips; 7 new tests), all 15 extension files, 144 panel cases (+8), two script files, and 10,696 inert Markdown renderings.

- **Category:** Reliability. **Recover interrupted log entries across renderer upgrades (2026-10-28).** Completed assigned `brwr-3cc`. New single-event journals retain bounded prepared bytes plus a hash of unrendered normalized event fields, so visits, activity, Why and summaries resume the original append after presentation changes. Single and batch retries preserve edited tails, unrelated rows, changed-folder refusals and complete-marker deduplication; v1 frozen-renderer recovery remains covered explicitly. **Metrics:** Four new Rust regressions; 52 capture tests and 923 workspace Rust outcomes (43 explicit IPC sandbox skips), all 15 extension and two script files, full required/README gate and 10,696 inert Markdown renderings green.

- **Category:** New Capability. **Show recorded incoming navigation on artifact cards (2026-10-28).** Completed assigned `brwr-1g2`. The card lists up to five host-derived source/transition pairs with explicit HTTP(S) links, measured navigation wording and the latest recorded day. Repeated observations stay one row; unresolved, self, back/forward and unknown historical sources never invent provenance. Protocol v8 carries the required list without changing logs or cache formats. **Metrics:** 4 new Rust and 2 composed-panel regressions; 944 Rust outcomes (901 exercised, 43 explicit IPC sandbox skips), 1,680 extension tests, the complete required/README gate and 10,696 inert Markdown renderings green.

- **Category:** Bug Fix. **Keep retired adapter history within a reserved config budget (2026-10-28).** Completed assigned `brwr-2iz`. History growth now has a 32 KiB pretty-TOML budget inside the 64 KiB config cap. Older larger histories may stay unchanged or shrink, so Pause and disjoint site additions remain available while the file fits. Exact restoration and duplicate pruning reclaim history space without forgetting distinct note identities; budget and whole-file refusals reach settings with specific recovery actions. **Metrics:** 4 new Rust regressions; 940 Rust outcomes (897 exercised, 43 explicit IPC sandbox skips), all 15 extension and 2 script files, full required gate and 10,696 inert Markdown renderings green.

- **Category:** Reliability. **Deliver capture-loss evidence when the last batch drains (2026-10-28).** Completed assigned `brwr-m64`. Overflow arriving during the final native delivery now follows it as a standalone batch without waiting for another capture. A stored ID and frozen count/range survive lost replies, later drops and worker restarts; retries back off, Pause still gates delivery, and the host chooses the owned log only from validated timestamps and the checked home folder. Existing anchored batches keep their recovery bytes. **Metrics:** 3 new worker cases and 4 new Rust regressions; 936 Rust outcomes including existing IPC sandbox skips, all 15 extension and 2 script files, the full required gate and 10,696 inert Markdown renderings green.

- **Category:** Bug Fix. **Complete interrupted captures before applying the new URL limit (2026-10-28).** Completed assigned `brwr-u8r`. A retry now reaches its existing v1 intent before an oversized normalized page, mode or trail URL can reject the event. Recovery retains the exact old bytes and all hash, prefix, ownership and folder checks; changed payloads stay retryable, while new oversized writes remain rejected. Rejected events cannot choose the log day for valid batch neighbors. **Metrics:** 2 new failing-first Rust regressions covering 8 recovery variants and both mixed-batch orders; 931 Rust outcomes (888 exercised, 43 explicit IPC sandbox skips), all 15 extension and 2 script files, full required gate and 10,696 inert Markdown renderings green.

- **Category:** Bug Fix. **Check recorded identities before Capture Site offers Save (2026-10-28).** Completed assigned `brwr-t8m`. Starter reviews now wait for the host to check the proposed rules against active, retired and hand-removed contracts, then name a conflicting recorded type and ID template before Save is available. Edited selections and closed reviews discard older replies; an unsupported host or mismatched response leaves Save disabled. Existing revision-checked saves remain authoritative, and a fresh site still enables in one click. **Metrics:** 7 new panel cases, 1 native regression and 1 protocol round trip; 134 panel cases, all 15 extension and 2 script files pass; 929 Rust outcomes (886 exercised, 43 explicit IPC sandbox skips); full required gate and 10,696 inert Markdown renderings green.

- **Category:** Bug Fix. **Reload the current note after an external adapter change (2026-10-28).** Completed assigned `brwr-jdi`. The panel compares host config revisions and reloads the note through the editor's serialized save/load queue before the next edit uses its current key. Pending and in-flight drafts retain old-key conflict copy-out and durable recovery, including newer typing; a successful later save keeps the conflict copy. An unchanged revision leaves the editor and debounce alone. **Metrics:** 4 new panel cases; 159 panel/editor cases and all 15 extension files pass; 927 Rust outcomes (884 exercised, 43 explicit sandbox IPC skips); complete local build, format, clippy, typecheck, lint, script and Markdown gates green.

- **Category:** Bug Fix. **Keep adapter repair available for preserved invalid configs (2026-10-28).** Completed assigned `brwr-5nq`. The response guard accepts format 0 and more than 64 artifact objects so the host's policy issue and revision reach settings. Repair reset remains available; malformed artifact entries are refused and the adapter engine still rejects these documents as new policy. **Metrics:** Three shared host-reply cases, two failing-first cases; 884 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and two script files green.
- **Category:** User Experience. **Explain pending pauses when the host cannot answer (2026-10-28).** Completed assigned `brwr-gob`. The Why hint names Pause capture only when it is usable, explains a host outage and asks to reopen the panel, and restores the normal hint when the host returns. The reload status avoids retaining a pointer to a control that can later be disabled. **Metrics:** One failing-first panel regression covers outage and recovery; full required gate green, plus 10,696 inert Markdown renderings.
- **Category:** Bug Fix. **Keep Why context off sites awaiting removal (2026-10-28).** Completed assigned `brwr-ufa`. Both Save and its handler refuse Why entries for this page's pending-removal origin and point to settings, preserving the typed draft. Pending removal of a different origin leaves Save available. **Metrics:** One failing-first panel regression and one other-origin control; all 15 extension test files pass, full Rust gate green.
- **Category:** Bug Fix. **Keep coalesced activity refs with their title evidence (2026-10-28).** Completed assigned `brwr-rft`. Under queue pressure, a new title replaces the previous title's refs rather than accumulating evidence the host would reject. A missing new title retains the older title and refs, and retained refs stay deduplicated and bounded. **Metrics:** One failing-first worker case and three controls; 884 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and two script files green.

- **Category:** Bug Fix. **Describe capture and unreadable tabs accurately (2026-10-28).** Completed assigned `brwr-09c`. The enabled status reuses the settings wording about sites with adapters, including with no adapters configured. Chrome pages and tabs hidden by withdrawn access clear the artifact verdict and old card; readable unrecognized pages still say Not an artifact. **Metrics:** Four failing-first panel regressions pass; full local gate green, 883 exercised Rust tests and 43 explicit IPC skips, all 15 extension and two script files passing.
- **Category:** Bug Fix. **Refresh artifact totals when the host reconnects (2026-10-28).** Completed assigned `brwr-b50`. A successful note autosave after an outage now refreshes the current artifact and clears the disconnected status while retaining an unsaved context draft. Ordinary subsequent saves send no extra artifact lookup. **Metrics:** One failing-first composed panel regression passes; full local gate green and all nine new panel cases pass.
- **Category:** Bug Fix. **Refresh delivered activity after every status reader (2026-10-28).** Completed assigned `brwr-mm0`. Central session observation detects delivery generations seen by Dismiss, Pause and Discard as well as the poll, so those controls cannot absorb an update and leave stale totals. Each observed change refreshes the current card once; unchanged polls make no request. **Metrics:** Three failing-first action cases plus the normal-poll control pass; full local gate green, including 10,696 inert Markdown renderings.

- **Category:** Bug Fix. **Name summaries for the current item after in-app navigation (2026-10-28).** Completed assigned `brwr-sti`. Extraction prefers a unique current main-page H1 over stale tab, document and Readability metadata, with existing fallbacks when headings are absent or ambiguous. A list-to-item SPA regression runs the real summarize entry point, verifies the item title reaches the run, and keeps URL-change refusal intact. **Metrics:** Full local gate green; four new extension regressions, all 15 extension and two script files passing, 883 exercised Rust tests plus 43 explicit IPC skips, and 10,696 inert Markdown renderings.
- **Category:** Testing & Verification. **Keep capture fixtures stable across midnight (2026-10-28).** Completed assigned `brwr-mdn` and its duplicate `brwr-mdt`. Capture and native unit tests share one recent UTC noon per test thread, keeping independent events on the same log day without altering production clocks or delivery windows. A failing-first regression reruns the batch/recovery and full-log rejection scenarios at 23:59:59 and midnight in UTC-12, UTC and UTC+14. **Metrics:** Full required gate green; one new Rust regression, 883 exercised Rust tests (+1) plus 43 explicit IPC skips, all 15 extension and two script files passing.

- **Category:** Bug Fix. **Offer older-note migration only while the shared note is absent (2026-10-28).** Completed assigned `brwr-v5t`. Older URL notes keep read-only View; Use as artifact note appears only when the host reports no artifact note. Confirmed saves and stale-save responses update availability without reloading or disturbing copy-out recovery. **Metrics:** 882 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and two script files green; five new extension regressions across this batch, including both rendered action sets and read-only viewing.
- **Category:** Bug Fix. **Preserve the original URL when migrating an older note (2026-10-28).** Completed assigned `brwr-w4k`. The migration renders the shared artifact note with the verified source note's creation URL, retaining identity evidence even when the action was clicked from another route. The host regression checks the stored URL, source removal and a later save from the destination route; existing ownership and no-clobber checks remain in force. **Metrics:** Full required local gate green; 882 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and two script files, and 10,696 inert Markdown corpus renderings.

- [x] `brwr-smr` **The smoke detects an extension rebuilt underneath it (2026-10-27).** PR #135's `timed out waiting for a new run card` and `Panel shows: null` stops came from `extension/dist` being rebuilt during the run by a codex review in the same worktree, not from CPU load or a product race. One build at the click reproduced the timeout exactly, with the panel saying it could not read the page, because `vendor/readability.js` was gone. One build at panel reopen reproduced the missing panel page. The smoke now refuses to start while `extension/.build` exists, and fails or annotates a run whose `dist` files changed. The run-card wait stops at once with the panel's own failure line. TESTING.md documents the rule. After PR 4 (#96) merged, `cardIn`, which PR 4's t53 `summarizeWithCodex()` also uses, moved onto `waitUntil`; on timeout it reports what the service says about the run. **Metrics:** before, 11 of 11 passed under load without a rebuild (6 at full load, 5 at 3x); after, 10 consecutive passes (5 idle, 5 under full load), and 11 more on the tree merged with PR 4 (5 idle, 6 under full load).
- **Category:** Bug Fix. **Keep captured activity and context after rebuilding history (2026-10-28).** Activity and Why log payloads now use the same canonical UUID as their marker hash and immediate index update. Uppercase and unhyphenated valid IDs survive reindex without losing measurements or context or double-counting work, and batch replies echo the request ID so acknowledgements still correlate. Completes assigned `brwr-q7e`; historical marker and partial-intent recovery remains `brwr-smu`. **Metrics:** Three new UUID regression matrices plus a test-first batch-reply assertion; full gate green, 755 exercised Rust tests and 43 explicit IPC skips, all 15 extension and two script files.
- **Category:** Bug Fix. **Reject encoded URLs that rebuilt history cannot retain (2026-10-28).** Capture checks the stored URL after authorization and normalization, refusing values over 8192 bytes as `url_too_long` before creating logs or intents. The same bound covers page, mode and trail URLs in single/batch capture and the shared Why/summary writer, while an exactly-at-limit Unicode URL survives reindex. Completes assigned `brwr-u8n`. **Metrics:** Three new URL regression tests, six new Rust functions across both fixes; 798 reported Rust outcomes (755 exercised, 43 IPC skips), all required checks, both Clippy configurations, 15 extension and two script files pass; macOS smoke unavailable on Linux.

- **Category:** New Feature. `brwr-t43`, `brwr-t44`, `brwr-t45`, `brwr-t46`, `brwr-t47`, `brwr-t48`, `brwr-t49`, `brwr-t50`, `brwr-t51`, `brwr-t52`, `brwr-t53`, `brwr-t54`, `brwr-so4` **Open access PR 4: summary files, the summary log event, and summaries in the index and panel (2026-10-27).** Built on `feat/summary-outputs-pr4`, not yet merged; every plan task (43-54) is done, so `brwr-so4` is closed. The service's `SummaryFinisher` saves a validated answer under the configuration current at finish, following a key changed mid-run, with the answer's `answered_at` stamp (re-stamped when the daily log's window would refuse it) in the click's `requested_at` offset, so Retry save is idempotent; failures are plain reasons that say what to do. Summary files are rendered from the answer, written through the shared ownership-checked replace and never adopted; body text goes through a context-free escape (`&lt;`, `\!`, and backticks, `$`, tabs, `~~~` runs and code-length space runs as numeric entities) with wikilink and single-line rules, front matter is JSON-quoted YAML with no entities, and `npm run test:markdown-neutralizer` (a new CI step) renders the shared corpus with markdown-it, micromark and marked. The `summary` event gains `record_key`, `artifact_type` and `harness` and attaches to the adapter's artifact on recognized pages; the index cache is format 6 with an in-place format-5 migration, summaries never count as opens or opened days, artifact files gain a Summary line and date-ordered Timeline summary lines, and Recent rows, `get_artifact` (unrecognized pages too, quietly null on failure) and the worklog carry them. `open_summary` opens only an owned summary file with PR 3's opener, by the `summary_path` the Recent row, summary line or run card shows, so a later key change cannot send it to another record's file; the panel shows the page's saved summary, summarized Recent rows, and the run card's saved line with one retried lookup. The headless smoke (`brwr-t53`) covers the outputs against the fake harnesses: an answer carrying a Markdown image, an `<img>` tag, a Mermaid fence and a Dataview query reaches the summary file only in escaped form with plain YAML front matter, and the run checks the log event, `list_artifacts` (unrecognized and adapter-recognized pages), the worklog, artifact-file Summary links and Timeline order, Retry save over an unowned file, `reindex`, and **Open summary**. **Metrics:** 3 consecutive headless smoke passes (45 steps each, about 100 s). **[human]**: the neutralized corpus in Obsidian and VS Code preview (TESTING.md "Real-UI acceptance").
- [x] `brwr-hnr` **Summary-only days no longer count as opened days (2026-10-27, open access PR 4).** Fixed by PR 4 Task 47 (697e515): the Summary arm of `derive` no longer inserts a day opened, so a summary never adds to `opens` or `days_opened` (it can still move first and last seen). Covered by Task 48's day-A/day-B regression `index::tests::an_m1_visit_and_a_later_summary_open_the_page_on_one_day` (an M1 legacy visit on day A plus a summary on day B is one open on one day, in the index, the artifact file and the worklog) and by `index::tests::a_summary_on_an_adapter_artifact_without_activity_creates_that_row` (a summary-only artifact shows `opens: 0` and `days_opened: 0`).
- [x] `brwr-deo` **M2 step 8.5, `/summarize` in the panel: superseded by open access PRs 3-4 (2026-10-27).** The tracking pointer is closed: Summarize & log, extraction and the run card shipped in PR 3 (`brwr-sr3`, #98), and the summary file, log event and index/panel integration in PR 4 (`brwr-so4`), with no denylist check.
- [x] `brwr-fgb` **Offer Forget recorded adapters for a full adopted record (2026-10-27).** `adopted_state.full` (additive v6) marks the 1,024-contract / 1 MiB warning, and settings shows Forget recorded adapters with the same in-page confirmation for it; write and lock failures, which forgetting cannot fix, show no button and no Forget wording.
- [x] `brwr-smk` **The macOS headless smoke waits for state, not time (2026-10-27).** Since #98, main's required smoke had stopped three ways, each passing on rerun. `Extensions.loadUnpacked` had 15 s to cover Chrome's whole cold start. A reopened panel's restored Codex card was taken for the new Claude Code run. The capture step navigated before the worker was shown to hold the new lease. A fourth stop showed up under load: one timed-out `chrome.sidePanel.open` ended the run. `scripts/smoke-macos.mjs` now has one helper, `waitUntil(what, observe, accept)`, whose timeout reports the last state observed. It is used to wait for Chrome before loading the extension, for the worker's lease (`workerHoldsHostPolicy`, `reloadSettings`), for a new run card that the service did not already list (`summarizeFromPanel(harness)`), and to retry opening the panel safely. TESTING.md "Waiting for state, not time" documents it for later smoke steps. No product race was found, and no step asserts less. **Metrics:** before, 3 of 3 idle and 3 of 5 under full CPU load passed; after, 16 consecutive passes (6 idle, 6 under full load, 4 under 3x load).
- **Category:** Bug Fix. **Keep brief activity visible in worklogs and Timelines (2026-10-27).** Positive focus and background durations below one minute now read `<1 min` instead of `0 min` through the shared prose formatter. Zero durations stay omitted, longer durations keep whole minutes, and artifact frontmatter retains integer counters. Two regressions first reproduced the old output, then verified the actual worklog and regenerated artifact file. Completes `brwr-d3m`. **Metrics:** 569 exercised Rust tests (+2), 43 explicit IPC skips, all 15 extension and 2 script test files; full required gate and both Clippy configurations green.
- **Category:** Bug Fix. **Load Recent when the host finishes connecting (2026-10-27).** Opening Recent during startup shows waiting text for the list and worklog, with Copy disabled, then fills both automatically once the host configuration arrives. Only loads that were waiting are retried, avoiding duplicate requests if the user opens Recent during policy installation; startup can still load page notes concurrently. Completes `brwr-p9r`. **Metrics:** Three new composed-panel timing cases, all 15 extension and 2 script test files green; typecheck, lint, extension build, Rust suites and both Clippy configurations pass. macOS smoke is unavailable on Linux; no dialog or smoke-step changes.
- **Category:** Bug Fix. **Keep unsaved page notes across panel close and reopen (2026-10-27).** Restored the unmerged `8ae533c` recovery work for `brwr-3nq`: each edit synchronously writes an immutable local snapshot before autosave, so closing during debounce or a held save preserves the latest text. Reopening loads the current host note alongside separate Copy/Discard recovery; conflicts and empty deletions remain, acknowledgements remove only their own snapshot, and quota/blocked-storage failures warn without blocking native saves. Recovered text never autosaves into a changed revision or artifact key. **Metrics:** Full required gate green; 567 Rust tests exercised with 43 explicit IPC skips (610 reported passes), all 15 extension and 2 script test files passing, 15 new regressions (29 editor and 60 composed-panel cases). macOS smoke refuses Linux; browser acceptance remains separate.
- **Category:** Testing & Verification. **Guard missing-service tests when a sandbox denies IPC (2026-10-27).** Six service/proxy tests now use the existing permission-denial guard, including missing-listener connections and the keyed-frame forwarding boundary. Assertions stay intact; simulated denial skips locally and fails when CI requires IPC. This closes `brwr-elx` and verifies the already-merged 60-exchange dialog fixture (`brwr-4vb`, `00dbc75`). **Metrics:** Full required gate green; workspace 567 exercised/43 IPC skips; scripted-dialogs 522 exercised/53 IPC skips; six CI-required denial failures verified.
- **Category:** Testing & Verification. **Verify Capture Site attention across hour boundaries (2026-10-27).** Reproduced `brwr-647` by starting its 15-second interval ten seconds before the hour: status correctly queued 10 seconds and kept 5 seconds live. Fixed local clocks and explicit live-plus-queued assertions now cover both mid-hour and boundary-crossing intervals while retaining zero-open, tab-identity and renewal checks. **Metrics:** 77 worker cases (+1), 20 repeated worker runs, three time zones, and three complete 15-file extension runs green; full required gate and both Clippy configurations pass.

- [x] `brwr-rq5` **Unix-socket tests skip in a sandbox that denies local IPC (2026-10-27, #125).** Codex's `workspace-write` sandbox on Linux refuses to bind or connect a Unix socket (EPERM), so Cookie's in-cycle health gate failed every socket test and refused product work for 20+ cycles. That was a limitation of the runner, never a product defect or a red `main`. Every test that binds or connects a socket now calls one cached probe first (`host/src/ipc_test_guard.rs` for Rust, `scripts/tests/ipc-guard.mjs` for `test:scripts`): a permission denial skips it with a one-line `skipping: local IPC denied by sandbox` reason on stderr; with `BRAUSER_REQUIRE_IPC_TESTS=1` (set by CI since #107) it fails instead, and any other probe error always fails. `BRAUSER_TEST_SIMULATE_IPC_DENIED=1` exercises both paths where IPC works. Guarded: 47 Rust tests (38 unit, 9 `scripted-dialogs` integration) and 2 `test:scripts` tests. TESTING.md "Local IPC in sandboxes" documents it.
- [x] `brwr-hrr` **Repair a damaged adopted-adapter record from settings (2026-10-27).** Hand-retired adapter contracts freeze their routes' IDs (#122), and `config_result.adopted_state` (additive protocol v6) now names a damaged `adopted-contracts.json`, why it cannot be used, and that notes on unrecognized pages are paused; settings' **Forget recorded adapters** confirms in the page, sends `forget_adopted_contracts` with the shown revision, and the host backs the file up as `adopted-contracts.json.<timestamp>.bak` and records afresh under the config lock. A full or unwritable record is a non-blocking settings warning. Covered by host, wire, settings and headless smoke tests.
- **Category:** Bug Fix. `brwr-f4s` **Refuse `strip_params` entries an adapter rule reads (2026-10-27).** A settings save that would make `strip_params` remove a query parameter an adapter rule reads (a `query` key or `absent` entry; current, historical, or hand-removed adapters) is refused with a message naming the adapter, origin, parameter and entry, because the pages that rule tells apart would share one stripped address. Existing conflicts still load and save; settings shows them with a one-click **Stop stripping** fix. Matching uses the host's strip rule, pinned by shared vectors in `protocol/strip-params/vectors.json`. Recording which parameters were stripped is split out as `brwr-srp`.

- **Category:** New Feature. `brwr-sr3` **Open access PR 3: Summarize & log with full-access runners (2026-10-27).** The Brauser service now runs real agents: Codex as a `codex app-server` JSON-RPC thread with `approvalPolicy: "never"`, `danger-full-access`, and the answer schema as `outputSchema`, and Claude Code as `claude -p` stream-json with `--dangerously-skip-permissions` and a resumable session, both in the knowledge base folder with the service environment plus `agent.env`. The §6.4 prompt names `page.md` by path and marks it untrusted, with an editable type and category vocabulary; the answer is validated with one repair turn. Harness setup moved into the service (detect on its `PATH`, probe through the real runner) and the harness lockdown is deleted. Settings detects, tests, and chooses the harness and shows `agent.env` names; every HTTP(S) page gets **Summarize & log**, a `summarize` command, and a context-menu item that extract the page with pinned Readability 0.6.0 under encoded-byte caps; the polling run card offers Cancel, Retry save, Copy, Open in Codex, and Copy resume command, and is restored from `list_runs`. Saving summaries landed in PR 4 (`brwr-so4`). Closes `brwr-0ij` and `brwr-6wz`. **[human]**: a real Codex and a real Claude Code summary from the dev build, and Open in Codex opening Codex Desktop (TESTING.md "Real-UI acceptance"). Merged (#98).

- [x] `brwr-hrc` **Note identity: remember contracts removed by hand edits (2026-10-27, #85).** Adapter contracts that vanish from `config.toml` outside the settings path are recorded, so a hand-deleted board adapter cannot leave its modal rule's notes unrecognized.

- **Category:** New Feature. **Inspect private page patterns from the panel and settings (2026-10-27).** A collapsed panel review shows the shaped current address and same-origin link patterns only after a click, including on unknown sites. The shared shaper removes identifier/query values and title-like segments; the isolated top-frame reader checks page identity, scans at most 500 anchors and returns at most 50 shapes, generalizing singleton literals. Configured sites retain at most 20 visited shapes for seven days with local show/clear controls, including during host repair; incognito tabs, pauses and removals stay excluded, and samples never enter delivery or the vault. Closing, collapsing or navigating drops link results. This completes `brwr-b39`; unknown-site host drafting and enabling remain in `brwr-t40`. **Metrics:** 332 Rust tests and 11 extension test files passing; 21 new shaper, worker and composed UI scenarios; full required gate, both Clippy configurations and queue consistency green. macOS smoke unavailable on Linux; no scripted-dialog or smoke-step changes. Salvaged onto main by hand after the overnight rebase failed; the independent review pass also stopped attention timing on the previous artifact after an oversized same-origin navigation.

- [x] `brwr-hme` **One Brauser home folder (2026-10-27, #82).** The user picks one home folder; Brauser keeps `notes/`, `artifacts/`, `summaries/` and `logs/` inside it in a fixed layout, migrating older layouts once (`pages/` → `notes/`, `log/` → `logs/`, no-clobber). Machine state stays in OS directories.
- **Category:** New Capability. **Start capturing known work sites from the current page (2026-10-27).** The panel now reviews and enables Jira Cloud, SharePoint/OneDrive and separately scoped GitHub pull requests/issues without opening JSON. It shows the actual page's match, labeled illustrative examples and all fixed safety probes, preserves configured sites and pauses, and refuses stale reviews or pending removals. Existing note edits save before identity changes, with copy-out on failure; page-installed policy starts attention on already-open artifacts without inventing cold opens or suppressing real opens during background renewal. Unknown-site drafting stays queued in `brwr-t40`, and the parent remains open. **Metrics:** 332 Rust tests and 10 extension test files passing; 26 new model, composed-panel and worker regressions; full required gate, both Clippy configurations and queue consistency green. macOS smoke refused on Linux; 14 scripted dialogs unchanged.

- **Category:** Bug Fix. **Keep shared and older notes reachable after capture rules are removed (2026-10-27).** Historical note recovery now retains the older URL-note list and its read-only viewing action after adapter removal, recognition narrowing and site removal. Both URL lookup directions share durable activity, mode and explicit-context evidence, so alternate routes and context-only history recover the same note without inventing activity. Native request tests prove separate URL notes, migration refusals, stale-key copy-out, cache deletion/reindex and suppressed capture preserve existing content. **Metrics:** 332 Rust tests (+5) and 8 extension test files passing; full required build/quality gate and both Clippy configurations green; macOS smoke unavailable on Linux, 14 scripted dialogs unchanged.
- **Category:** Security & Privacy. **Refuse incomplete historical note scans before autosave (2026-10-27).** Review of note removal reproduced a partial candidate scan selecting a note despite an omitted conflicting identity. Primary recovery now fails closed at the file-count, per-file byte or total-byte limit and on unreadable regular candidates; optional older-note discovery remains best effort. Native loads and saves return correlated unavailable-identity errors, preserve note bytes and recover once the fault clears. **Metrics:** 332 Rust tests and 8 extension test files passing; 4 scan-fault cases within 1 new regression, plus activity/context ambiguity and cache-failure coverage; full gate green.

- **Category:** Bug Fix. **Recover missing artifact history from durable logs (2026-10-27).** Per-file ingestion checkpoints now catch up valid older caches before delivery updates and history queries, including after restart without replaying the missing event. Changed logs reparse in full, unchanged logs are skipped, SQLite publishes before the checkpoint cache, and all cache work shares the configuration lock. Durable capture stays acknowledged during cache failures, while artifact, Recent, and worklog queries report unavailable history instead of silently incomplete totals; bounded failures publish no partial checkpoint. **Metrics:** 326 Rust tests (+4) and 8 extension test files passing; full required build/quality gate and both Clippy configurations green; macOS smoke unavailable on Linux, 14 scripted dialogs unchanged.

- **Category:** Reliability. **Share activity authorization across single and batch delivery (2026-10-27).** Single activity delivery now calls the same host policy checker as batches under the existing configuration lock, removing duplicate identity, mode, trail, reference, and shape checks. A 13-scenario parity test verifies explicit outcomes and reasons, refusal before log/index writes, and exact decoded durable payloads for valid activity through both paths. The baseline already agreed, so no existing behavioral bypass is claimed. **Metrics:** 322 Rust tests and 8 extension test files passing; 1 new test covering 26 deliveries; full required build/quality gate and both Clippy configurations green.
- **Category:** Security & Privacy. **Keep invisible formatting out of generated work history (2026-10-27).** A shared host sanitizer now replaces Unicode format and control characters with spaces in daily row titles/context, regenerated artifact titles/context, and plain worklogs, and escapes Obsidian comments, highlights, and tags in generated Markdown. Both single and batch delivery use it while canonical encoded event payloads stay exact; reindex regenerates safe owned files without changing source logs. The review recorded legacy interrupted-entry recovery across renderer upgrades separately as `brwr-3cc`. **Metrics:** 319 Rust tests (+4) and 8 extension suites passing; two failing-first regressions, all 170 Unicode 17 format scalars verified locally, and full protocol generation, current-state consistency, build, format, both clippy gates, clean npm install, typecheck, lint, and extension build/tests green.
- **Category:** Security & Privacy. **Reject invented navigation trails while preserving older activity (2026-10-27).** The native host now restricts new trail transitions to Chrome's known values plus the worker's existing back/forward encoding, returning terminal `rejected` before log or index writes through both single and batch delivery. Older bounded log and cache payloads retain their measurements and exact replay identity, while unknown transition kinds cannot become derived edges; reindex keeps the original shape limits and leaves source logs untouched. **Metrics:** 314 Rust tests (+3) and 8 extension suites passing; full protocol generation, current-state consistency, build, format, both clippy gates, clean npm install, typecheck, lint, extension build/tests, and queue consistency green.
- **Category:** New Feature. `brwr-sv2` **Open access PR 2: the Brauser service skeleton (2026-10-26).** `brauser service` runs as an on-demand per-user LaunchAgent on a 0600 Unix socket in the 0700 support directory and exits after 10 idle minutes. The native host forwards the v6 run messages (`start_run`, `get_run`, `list_runs`, `cancel_run`, `retry_save`), resolves the record key itself, starts the service with `launchctl kickstart`, restarts a stale build after a `service_hello` build-identity mismatch, and answers `service_unavailable` otherwise. Run records, a two-run FIFO queue with one run per record key, a 10-minute timeout, process-group cancel, restart recovery, 30-day/200-run retention, and the 0600 `agent.env` overlay are covered against a fake harness only; `scripts/register-dev-host.mjs` registers, replaces, and (`--unregister-service`) removes the dev LaunchAgent. `brwr-9yc` decided (`agent.env`); `brwr-jfe` and `brwr-0cj` superseded by polling. The launchd checks are **[human]** (TESTING.md "Real-UI acceptance"): register the dev build and close Chrome; run `start_run` from the panel console; confirm `launchctl print gui/$UID/com.rauser.brauser.service` shows the service and that it exits after 10 idle minutes; confirm `agent.env` values reach a run (needs PR 3's real runners); confirm a rebuild plus a request restarts the service. Merged as PR #75 (2026-10-27) with the Codex review fixes.
- **Category:** Security & Privacy. `brwr-oa1` **Open access PR 1: standing HTTP(S) access and protocol v6 (2026-10-26).** Brauser now holds HTTP(S) access from install, so a note can be typed on any page at once, and a site is on exactly when it has an adapter. The native confirmation, the `macos-alert` crate, the AI denylist, `log_incognito`, grant reconciliation, the sync caution, and the harness token are gone; harness setup detects and probes; a knowledge base folder is chosen with the native picker; v5 config files load unchanged. `brwr-ahb` closed as obsolete: the consent identity it would have unified no longer exists.
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

- **2026-10-28 assigned P2 follow-up.** `brwr-mdn` and `brwr-mdt` were
  duplicate reports of the same midnight failures and are now closed together.
  Summary title freshness must come from the extracted current page, because
  Readability can reuse the same stale metadata as Chrome. Ambiguous/missing
  headings still use the documented title fallbacks; live GitHub navigation
  remains a macOS browser check. Ten assigned P3 items remain; continue those
  before self-directed work. Once the assigned queue and ordered plan clear,
  existing `brwr-ppn` (curating worklogs before copying) remains a useful next
  product improvement for sharing a day's work without unrelated rows.

- **2026-10-27 queue restore.** Cookie's cycle-1 queue update (PR #77,
  `aa200d3`) exported a stale local beads database over `.beads/issues.jsonl`
  and rewrote this file to match, reverting the open-access retarget from #69.
  The rows and backlog wording were restored from `5a80dda`: `brwr-sv2`,
  `brwr-sr3`, and `brwr-so4` are real beads; `brwr-0cj`, `brwr-9yc`,
  `brwr-rva`, `brwr-ac1`, `brwr-ahb`, and `brwr-jfe` are closed as recorded
  below; `brwr-4vb` stays open because its fix never merged. Cycle 1's
  Capture Site work (`brwr-qox`, `brwr-t40`, the `brwr-1hj` claim) is kept.
  `bd import` merges dependency edges and never deletes them, so a database
  that imported the stale export keeps `brwr-f45`'s retired blocking edges on
  `brwr-0cj` and `brwr-6wz` after it re-imports this file. Before the next
  export, run `bd dep remove brwr-f45 brwr-0cj` and
  `bd dep remove brwr-f45 brwr-6wz` (or rebuild the database from this
  file), then confirm `bd export` matches the committed rows.
- Capture Site is split into known-site setup and unknown-site drafting. Keep
  its next slice on the host-owned shaper/drafter path; do not turn arbitrary
  page links into starter rules in the panel. A configured site stays in
  settings until explicit re-key migrations support Re-learn.

- **2026-10-27 queue retarget for open access** (spec
  `docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md`). Closed:
  `brwr-9yc` (decided: `agent.env` in the Brauser service, spec §5.1);
  `brwr-jfe` and `brwr-0cj` (superseded by polling `get_run`, spec §5.3);
  `brwr-rva` (no `activeTab` trigger or grant expiry with standing access);
  `brwr-ac1` (no grant to request or roll back and no native confirmation; its
  choose-and-scope part folds into Capture Site). `brwr-6wz`, `brwr-0ij`, and
  `brwr-deo` are now pointers to open access PRs 2-4; `brwr-f45` depends on
  `brwr-sv2`/`brwr-sr3` instead of `brwr-6wz`; `brwr-ooy` is retargeted and
  deferred; `brwr-1hj` is reopened with the rescoped Capture Site. Windows
  support stays in scope (DESIGN §14, 2026-10-16), so `brwr-bzx` and
  `brwr-i1g` remain open.

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
- Overflow that races the final queued batch is now independently delivered
  (`brwr-m64`, 2026-10-28). The extension supplies no URL or path; the host
  derives the log day from its validated last timestamp inside the checked
  home folder. `brwr-9r7` follows up on showing gaps in worklog review.
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

### Cycle observations (2026-10-27 — page-pattern review)

- `brwr-b39` completes the private input and user inspection surface; continue
  `brwr-t40` for a real unknown-site draft and enable flow rather than adding
  another diagnostic view. The parent remains open, and the external
  open-access service work remains outside this cycle.
- Review the Notion contract in `brwr-gf1` before implementing kind grouping: a
  bare fixed-shape identifier has no leading route literal. The existing
  checker must not be bypassed to make a named success case appear green.
- Short lowercase route words survive shaping by design. Keep the disclosure
  concrete; patterns are useful drafting evidence, not guaranteed anonymous
  data. Link counts are not visits, and link-only candidates must remain
  unchecked in the future rule review.
- `brwr-vny` is a later product idea: help a person understand a skipped page
  using an actual policy reason, after the ordered Capture Site work.

### Cycle observations (2026-10-27 — dialog fixture repair and sandbox IPC)

Corrected 2026-10-27. Cycles on baselines `728b200` through `5231f9e` (merged
in #93, #94, #101 and #108) recorded the health gate as blocked and asked the
driver for a different runner. That was a sandbox limitation, not a product
blocker: Codex's `workspace-write` sandbox denies local Unix-socket IPC
(bind/connect return EPERM, and Node's child pipes, which are socketpairs on
Linux, fail the same way), so every socket test failed there while `main` was
green in CI. `brwr-rq5` fixed it in the tests: socket tests now skip in such a
sandbox with a printed reason and still run, and must pass, everywhere
`BRAUSER_REQUIRE_IPC_TESTS=1` is set, which includes CI (TESTING.md "Local IPC
in sandboxes"). A gate whose only failures are socket tests is no reason to
stop product work, and no history reset or production IPC change is called for.

- The successful-dialog fixture must read the framed request before answering,
  as production does. Large requests reliably exposed the old fixture's
  early-exit race; small requests alone passed by scheduling luck.
- Resume the assigned Capture Site work, `brwr-1hj` through `brwr-t40`: host
  drafting, selectable rules and example artifact grouping using the existing
  private pattern inputs. Resolve the Notion leading-literal contradiction
  (`brwr-gf1`) before claiming its identity cases. A useful later review would
  show which example pages selected rules group into one artifact before they
  are enabled, within the existing review; no additional diagnostic view or
  health surface is needed first.

### Cycle observations (2026-10-27 — green health gate)

- The health-only cycle on `349b6ca0` repaired `brwr-elx` and `brwr-647`,
  verified and closed `brwr-4vb`, and changed no product contract. The full
  required gate and scripted-dialogs tests now pass under the documented
  sandbox policy. Do not describe the explicit IPC skips as exercised socket
  coverage; CI still requires those tests and must verify them outside this
  sandbox. Logs are in `target/overnight-health/final-repair/` (gitignored).
- Missing-listener connections need the same IPC guard as listener tests.
  Attention checks spanning an hour must count both the live totals and the
  completed events handed to delivery; use fixed local clocks for that case.
- `brwr-t40` and `brwr-f45` remain reserved for Lane A. The parent
  `brwr-1hj` stays open. Once health passes next cycle, the next independent
  assigned user benefit is `brwr-3nq`: retain unsaved page-note drafts across
  panel close/reopen, with explicit recovery that preserves host revision
  checks. Reuse the existing note copy-out flow rather than inventing another
  recovery surface; no duplicate proposal is needed.
- The local beads export initially carried older notes for `brwr-1hj` and
  `brwr-4vb` and lacked `brwr-hnr`'s committed assignment. Imported those three
  committed records before updates/export to preserve the operator's queue.
  No assignment labels or other sessions' ownership were changed. Export also
  refreshes `brwr-sr3`'s derived dependent count; its edges are unchanged.

### Cycle observations (2026-10-27 — durable note recovery)

- Clean opening health gate on `fe81cbd`: no quarantine file or incomplete
  changes. Capture Site parent `brwr-1hj` remains in progress because its
  remaining `brwr-t40` work is explicitly reserved for Lane A. Cookie took
  the next ready assigned item, `brwr-3nq`, and restored its prior unmerged
  implementation without importing unrelated history changes from PR #67.
- Recovery is a local safety copy, not a second editor or a promise that
  teardown can finish a native request. Preserve the load-current-note and
  explicit-copy boundary when changing the panel or artifact identity. Keep
  quota failures visible: a failed new write cannot guarantee newest text
  survives close, even when an older copy remains durable.
- Follow-up `brwr-kws` proposes per-copy review for people with drafts from
  several pages; it is optional UX work, not grounds to delay recovery or
  displace the remaining assigned issues. Prioritize the queue over further
  recovery internals. Real macOS panel teardown/storage validation stays with
  the existing browser acceptance work (`brwr-9xg`), which remains open.

### Cycle observations (2026-10-27 — brief activity and startup Recent)

- Opening gate on `7cb3a75e719e` passed with a clean tree and no quarantine
  file. Capture Site parent `brwr-1hj` stays open: its remaining `brwr-t40`
  implementation belongs to Lane A. Completed the next two ready assigned
  issues, `brwr-d3m` and `brwr-p9r`, with failures reproduced before fixes.
- The next assigned queue work is `brwr-q7e` and `brwr-u8n`, preserving
  captured IDs and normalized URLs across index rescans. Keep their raw log
  and rebuilt-index assertions together; do not displace them with further
  formatting polish. Refresh the authoritative queue before claiming.
- A useful later product direction is the already-proposed worklog curation
  (`brwr-ppn`): let people pick what to share and preview the resulting text,
  especially for brief reviews now visible as `<1 min`. Reuse Recent and its
  existing Copy flow; do not introduce a separate history screen or alter
  recorded durations. This is guidance for that bead, not new assigned work.
- Startup has a period after configuration arrives while the worker installs
  its lease. Fetches opened during that period already work, so an automatic
  retry must only repeat the loads that actually waited. Test before hello,
  during policy install, and after startup to protect that distinction.
- Final Rust result: 612 reported outcomes, comprising 569 exercised tests
  and 43 explicit IPC-denial skips. All 15 extension and 2 script test files
  pass. Headless smoke refuses Linux; dialog accounting still compares the
  dynamic `dialogsShown` total with the host log, with no added steps or
  scripted answers in this cycle.


### Cycle observations (2026-10-28 — capture/index round trips)

- Opening gate on `46b46495` was clean; no quarantine file or unfinished work.
  Capture Site parent `brwr-1hj` stays in progress because `brwr-t40` is
  explicitly reserved for Lane A. Completed next assigned items `brwr-q7e`
  and `brwr-u8n` with failing regressions before fixes, then the full gate.
- Durable identity and reply correlation have different jobs: canonicalize
  stored UUIDs and immediate index updates together, while replies echo the
  request ID. Raw log payload assertions and rebuild checks caught the loss;
  checking cache totals alone could have hidden it. Old noncanonical records
  and partial intents are preserved for the focused `brwr-smu` follow-up.
- The next assigned older-note batch (`brwr-v5t`, `brwr-w4k`) completed on
  2026-10-28. Continue `bd ready --label assigned`; `brwr-09c` shipped in the panel
  refresh batch below, and `brwr-5nq` remains ready. Do not claim Lane A's work.
- The existing `brwr-ppn` worklog-curation proposal remains a valuable later
  capability: let someone select the work to share and annotate its purpose
  before copying. Reuse Recent and keep recorded measurements intact, with
  local preview and explicit copy rather than an automatic external send.
- Manual complete-diff review found the batch reply correlation gap; added
  the failing assertion before fixing it. The external `codex exec review`
  pass was not invoked because this unattended run forbids live external API
  calls. The driver's independent reviewer remains required. Final logs are
  in `target/overnight-health/cycle-20261028/review-final-*.log`; 43 Rust IPC
  skips are explicit, and macOS headless smoke refuses Linux.


### 2026-10-28 — Assigned older-note migration batch

- Opening health was clean on `33d1a310`: no quarantine file or unfinished
  diff. Added failing regressions first, implemented both fixes, reviewed the
  complete diff and ran the required gate plus README checks. The first final
  format check caught one Rust wrapping difference; `cargo fmt` fixed it and
  the entire gate then passed. No production code changed after that pass.
- Existence comes from the host flag, not note-body emptiness. First saves,
  concurrent creation and deletion can change migration availability while
  the panel stays open. Recovery remains copy-only and immutable-snapshot
  cleanup is unchanged. Migration retains a source URL already checked by
  the ownership parser; it accepts no new extension identity or path.
- Validation: 882 exercised Rust tests, 43 explicit socket-denial skips,
  15 extension files and two script files passed. macOS headless smoke
  refuses Linux. Logs and failing-first evidence are in
  `target/overnight-health/older-notes-20261028/`. Reviewed the full local
  diff manually; the external Codex review was not invoked because this run
  forbids live external API calls. The driver's independent review remains.
- Beads export also retains pre-existing database metadata on closed
  `brwr-1hj` (its driver-applied assigned label and cleared expired lease).
  Cookie changed only the statuses of `brwr-v5t` and `brwr-w4k`; the remaining
  ten assigned bugs stay open. No human gate was closed.
- Product direction after assigned work: prioritize the existing curated
  worklog (`brwr-ppn`) and named focus-session (`brwr-ued`) proposals. They
  turn captured cross-tool activity into an intentional account of work,
  which adds more visible value than another capture-health diagnostic.
- Commit handoff: the one git add/commit attempt could not create
  `.git/index.lock` on the read-only Git mount. The complete green diff is
  left uncommitted for the driver; `.bosun_commit_msg` contains the exact
  subject and body. No retry, push or service action was attempted.


### Cycle observations (2026-10-28 — assigned panel refresh batch)

- Clean opening health on `043a88c5`, no quarantine report. Completed assigned
  `brwr-09c`, `brwr-b50` and `brwr-mm0`; seven assigned items remain, with
  `brwr-5nq` next in driver order. Keep following the assigned queue and leave
  Lane A's work alone.
- Eight new cases failed before the production fix and the poll control passed;
  all nine now pass. Observe delivered generations where session changes are
  rendered, because every status reader can consume the worker's new counter.
  Keep connection and delivery observations advanced before host requests so
  response notifications cannot loop. Reuse the existing artifact coalescer.
- Full local gate passed: 926 reported Rust outcomes, comprising 883 exercised
  tests and 43 explicit IPC skips; all 15 extension and two script files pass,
  plus 10,696 inert Markdown renderings. Failing-first and final evidence is in
  `target/cookie-panel-red-detail.log`, `target/cookie-panel-green-detail.log`
  and `target/cookie-final-health.log`. Headless smoke refused this Linux VM.
- Reviewed the complete code/test diff manually; no additional defect found.
  The external Codex review was not invoked because this run forbids live API
  calls and real harnesses. The driver's independent review remains required.
- Product direction after assigned work: the existing `brwr-vny` explanation
  of why a readable page is outside capture can make the neutral verdict
  actionable. Pair it with the existing `brwr-1g2` incoming-trail view to help
  someone understand both what this page is and how they reached it, using
  already-validated history rather than collecting new page content.
- The single commit attempt was refused at `git add` because `.git/index.lock`
  is on the read-only Git mount. The complete green diff and excluded
  `.bosun_commit_msg` remain for the driver; no retry or remote action occurred.


### Cycle observations (2026-10-28 — assigned repair and capture-state batch)

- Clean opening health on `20b512161dad`, no quarantine report or unfinished
  diff. Completed `brwr-5nq`, `brwr-gob`, `brwr-ufa` and `brwr-rft` after
  reproducing each with a failing test. No human gate or assignment label was
  changed. The next assigned implementation work remains `brwr-jdi` and
  `brwr-t8m`; keep using `bd ready --label assigned`.
- `brwr-iqx` remains open with triage evidence: `VaultCheckFinisher` and its
  obsolete confirmation wording no longer exist. `SummaryFinisher` already
  says to choose a notes folder in settings and Retry save, covered by
  `a_config_that_needs_repair_or_has_no_folder_names_the_fix`. Reconcile its
  original criteria against this replacement before claiming another fix.
- Final gate: 927 reported Rust outcomes (884 exercised, 43 explicit sandbox
  IPC skips), all 15 extension and two script files, plus 10,696 inert
  Markdown renderings. Added ten extension cases and one Rust fixture check.
  Detailed failing-first, final and review evidence is in
  `target/cookie-assigned-20261028/`; headless smoke refuses Linux.
- Read the complete code/test diff and fixtures manually. The one additional
  review repair avoids a disabled Pause capture pointer while the panel is
  busy or needs config repair. No real harness or external Codex review was
  invoked under the no-live-API rule; the driver's independent reviewer
  remains required. Shared host wire fixtures preserve the distinction
  between reading an inert document and validating policy to enable.
- Product direction after assigned work: the existing `brwr-ppn` curated
  worklog remains more useful than another status view. Let someone select
  cross-tool work and add purpose before copying a standup update, retaining
  the measured history. Existing `brwr-vny` can explain an unrecognized page
  using the actual policy reason and lead directly into Capture Site.

- Commit handoff: the single git add/commit attempt could not create
  `.git/index.lock` on the read-only mount. The reviewed green diff and
  excluded `.bosun_commit_msg` are ready for the driver; no retry or remote
  action was attempted.


- **2026-10-28 assigned note-identity cycle (`brwr-jdi`).** Opening health
  on `dd8ff6795678` was clean with no quarantine report. Added the panel
  regression before the fix and read the full code/test/doc diff afterward.
  The change uses the existing save/load queue; no new identity computation,
  recovery replay or protocol surface was introduced. Full local gate green,
  with details under `target/cookie-jdi-20261028/`. The macOS smoke runner
  refuses Linux. Manual review completed; external `codex exec review` was
  not invoked because this run forbids live external APIs. The driver's
  independent review remains the publication gate.
- **Next assigned work:** `brwr-t8m` remains ready. Starter planning sees
  only active adapters, whereas host transition checks also use retired and
  hand-removed contracts. Reproduce both the host removal-then-starter case
  and the panel promise before implementing a pre-save history check. Keep
  the host's frozen-ID refusal authoritative and name the conflicting
  contract in the panel. Do not reuse a retired identity by guessing.
- **Obsolete assigned criteria:** `brwr-iqx` remains open with appended
  evidence, since its `VaultCheckFinisher` and obsolete wording are gone.
  The replacement SummaryFinisher's existing missing-folder regression
  passed again. The operator should reconcile the deleted-type criteria;
  Cookie did not close the assignment or change its labels.
- **Product direction after assignments and plan gates:** the existing
  `brwr-ued` focus-session proposal offers more value than more status
  wording: a person could connect the issue, document and pull request they
  used for one task and recover that context later. The current fix also
  reinforces `brwr-kws`: recovered text should stay a deliberate, individual
  copy/discard action, separate from the current host note. Keep those
  proposals queued behind assigned work and the ordered DESIGN plan.
- **Commit handoff:** the one `git add -A`/commit attempt was refused while
  creating `.git/index.lock` on the read-only mount. No retry was attempted;
  the green diff, queue export and excluded `.bosun_commit_msg` are ready for
  the driver.


### Cycle observations (2026-10-28 — Capture Site history preflight)

- Opening health on `44504d7e445a` was clean, with no quarantine report or
  unfinished diff. Completed assigned `brwr-t8m` with host and panel failures
  recorded before the production change. The first host control incorrectly
  used two config filenames in one support directory as two installations;
  corrected it to use a separate directory because adopted history is shared
  there. The complete required gate then passed. No production changes
  followed that gate.
- `brwr-iqx` is now blocked for operator reconciliation, retaining its assigned
  label. Its named VaultCheckFinisher and confirm wording are deleted; the
  replacement SummaryFinisher's existing missing-folder regression passes.
  Do not recreate obsolete code just to satisfy a stale acceptance test, or
  claim a failing-first fix that never happened. No actionable assigned item
  remains in this cycle.
- The preflight shares host transition checks instead of exporting history or
  copying the Rust overlap policy into TypeScript. It grants no token and
  cannot bypass the save's independent revision/history checks. Unknown-site
  drafting retains its own existing check. A late response cannot enable a
  changed selection, invalid organization field or reopened review.
- Validation: 929 reported Rust outcomes, 886 exercised with 43 explicit IPC
  sandbox skips; 134 composed panel cases, all 15 extension and two script
  files, and 10,696 inert Markdown renderings pass. macOS headless smoke
  refuses this Linux VM. Evidence is in `target/cookie-t8m-*.log`. Complete
  source, generated-wire, test and documentation diff reviewed manually;
  external Codex review was not invoked because this run forbids live APIs.
  The driver's independent reviewer remains the publication gate.
- New proposal `brwr-cua` makes the refusal actionable: restore a removed
  site's recorded custom rules through a plain-language review and explicit
  save. Preserve original identity/scope and fail closed on ambiguity. This
  complements the existing curated-worklog and focus-session proposals; it
  must wait behind assigned work and the ordered DESIGN plan.

- Commit handoff: the single `git add -A` / commit attempt failed creating
  `.git/index.lock` because the Git mount is read-only. The complete green
  diff, exported queue and excluded `.bosun_commit_msg` remain for the
  driver. No retry, remote operation or live-service action was attempted.

### Cycle observations (2026-10-28 — capture recovery before URL limits)

- Opening health on `e604019b` passed with a clean tree and no quarantine
  report. Claimed and completed highest-priority ready assignment `brwr-u8r`.
  No live service, credential, remote Git operation or external API was used.
- The regression independently reconstructs pre-limit presentation and payload
  bytes, publishes a v1 intent and truncates the log. The original code
  terminally rejected the oversized event before checking that intent. The
  new test covers visit/page/mode/trail URLs through single and batch paths,
  refuses changed payloads, completes the original bytes and deduplicates a
  second delivery. An initial fixture serialized the struct directly rather
  than the old host's JSON value; corrected its JSON key ordering.
- Review caught and tested an implementation issue before handoff: delaying
  the bound left rejected oversized candidates participating in the batch's
  calendar-day check. The new mixed-batch regression failed, then passed
  after only accepted new writes were allowed to select the log day. The
  existing raw and stored URL caps and exact 8192-byte boundary still pass.
- Final gate: 931 Rust outcomes (888 exercised, 43 explicit IPC sandbox skips),
  all 15 extension and two script files, current-state consistency and 10,696
  inert Markdown renderings pass. Evidence is in `target/cookie-u8r-*.log`.
  The macOS headless smoke runner explicitly refused Linux. Complete code,
  test and documentation diff reviewed manually; external Codex review was
  not invoked because this run forbids live APIs. The driver's independent
  reviewer remains the publication gate.
- Queue priorities remain authoritative: 13 assigned items remain, including
  the two blocked items; do not jump to self-directed product work. The
  existing `brwr-1g2` incoming-trail card and `brwr-kws` individual recovery
  review offer visible workflow improvements after higher-priority work.
  For future recovery-format changes, keep a fixture of exact historical
  bytes: regenerating both the fixture and expected output with today's
  renderer can hide the compatibility failure.

- Commit handoff: the single `git add -A` / commit attempt failed creating
  `.git/index.lock` on the read-only mount. The green diff, exported queue
  and excluded `.bosun_commit_msg` remain for the driver. No retry or remote
  operation was attempted.

### Cycle observations (2026-10-28 — standalone overflow evidence)

- Opening health on `f4f12f7c` was clean; no quarantine report or incomplete
  tree. Completed the first ready assignment, `brwr-m64`. The next ready
  assignment is `brwr-2iz`; twelve assigned items remain, two blocked.
- The worker regression failed before implementation. Its final fixture
  freezes two actual completed hours while one byte-full native batch is in
  flight, drops both through the real pressure path, then sends their marker
  without a later navigation. A first fixture used URLs beyond the adapter
  engine's 2,048-character match bound; it was corrected to use valid URLs
  and bounded mode/title evidence to fill the queue instead.
- Full code, test and documentation diff reviewed manually. Snapshot ownership
  remains with a delayed event batch, so a separate marker cannot duplicate
  its attached evidence. Standalone replies that report false retain the
  marker and stop the send loop; older v7 hosts safely leave evidence pending.
  Rust tests cover partial-write recovery, duplicate retries and all selected
  folder/policy gates. Regenerating protocol types produces no changes: only
  the schema array minimum was widened.
- Full required gate passes: 936 Rust outcomes (including existing sandbox
  IPC skips), 15 extension files and 2 script files; 191 Markdown corpus
  inputs produce 10,696 inert renderings. macOS headless smoke was not run on
  Linux; its dialog flow and `dialogsShown === logged` check are unchanged.
  External Codex review was not invoked because this run forbids live APIs;
  the driver's independent review remains the publication gate.
- New proposal `brwr-9r7` makes existing gap evidence visible during worklog
  review and optional in copied text. It is deliberately unassigned; finish
  assigned work first. Durable evidence is useful only if the person sharing
  their work can notice an incomplete day without opening raw Markdown logs.
  Existing named focus sessions (`brwr-ued`) remain the larger product swing
  after assigned work and ordered plan work are complete.

- Commit handoff: the single staging/commit attempt could not create
  `.git/index.lock` on the read-only mount. The green diff, exported queue
  and excluded `.bosun_commit_msg` are ready for the driver; no retry or
  remote operation was attempted. Local validation logs are under
  `target/cookie-m64-*`; the worker file contains 87 passing cases.


### Cycle observations (2026-10-28 — retired adapter history budget)

- Opening health on `a7ed9437` was clean: no quarantine file or incomplete
  tree, and every required baseline check passed. Completed first ready
  assigned item `brwr-2iz`. Continue `bd ready --label assigned`; eleven
  assigned items remain, including two blocked ones, with `brwr-1g2` next.
- Plan implemented: a 32 KiB pretty-TOML growth budget, grandfathered legacy
  histories, actionable typed size refusals through native settings, and
  pruning limited to exact duplicates and exact active restorations. The
  adopted state file and all distinct recorded route contracts remain intact.
- The first store and native regressions failed before implementation. Test
  iteration caught fixture assumptions: restoring a different same-type ID
  must precede removing the current one; enough contracts must be restored
  to get below the history budget before growing it again; active adapters
  resolve through adapter_match, with note_key_rule reserved for fallback.
- Reviewed the complete code/test/design diff. Full required and README gate
  passes: 940 Rust outcomes (897 exercised, 43 explicit IPC skips), all 15
  extension and 2 script files, and 10,696 inert Markdown renderings. Logs:
  `target/cookie-2iz-final-*.log`. External Codex review was not invoked because
  live APIs are forbidden; the driver supplies independent review. macOS
  headless smoke was not run on Linux; dialog behavior is unchanged.
- Existing proposal `brwr-cua` is the useful next extension of this work:
  offer the original recorded rules in an explicit restore review instead of
  requiring reconstruction in the advanced editor. Reuse that issue rather
  than create a duplicate. Show history budget separately from total-file
  space: restoring an adapter frees the first but moves bytes within the
  second. After assigned and ordered work, curated worklogs (`brwr-ppn`)
  remain a larger product improvement for sharing only relevant work.

- Commit handoff: the single staging/commit attempt failed creating
  `.git/index.lock` on the read-only mount. The completed green diff,
  exported queue and excluded `.bosun_commit_msg` remain for the driver.
  No retry, push, or remote operation was attempted.


### Cycle observations (2026-10-28 — incoming navigation)

- Opening health on `21de2229` was clean, with no quarantine report and all
  required baseline commands passing. Claimed the first ready assigned item,
  `brwr-1g2`, and completed its native query, protocol, panel and test work.
  Continue `bd ready --label assigned`; ten assigned items remain, eight ready,
  with `brwr-3cc` next and the existing two blocked items unchanged.
- The list is five source/transition pairs, not five inferred relationships
  or a click count. Source links use the known artifact's current recorded
  URL. Ordering is by local day, then source ID and transition, so no
  within-hour chronology is claimed. A known source may appear for multiple
  navigation kinds; back/forward stays unattributed even in old paired data.
- Full diff reviewed locally. The initial feature gate caught stale version-7
  wire fixtures after the v8 bump; updated all current-message fixtures and
  added an explicit required/bounded/known schema regression. The repeated
  full required and README gate passes: 944 Rust outcomes (43 explicit IPC
  skips), all 15 extension files, both script files and 10,696 inert Markdown
  renderings. Also ran every extension file separately without child test
  isolation: 1,680 tests passed, including 136 panel cases. Logs are
  `target/cookie-final-gate.log` and `target/cookie-direct-extension.log`.
- No live API or external Codex review was invoked; the driver supplies the
  independent review. Linux cannot run the macOS-only smoke runner. Its
  protocol constant is v8; the existing three folder-picker answers and
  dynamic answered-versus-logged dialog check are unchanged. Source-tab
  activation is covered with local scripted panel fixtures, while real
  Chrome anchor default behavior remains a macOS verification step.
- Product direction: existing `brwr-ued` named focus sessions and `brwr-ppn`
  curated worklogs remain valuable after assigned and ordered work. Explicit
  sessions can explain the person's intent across tools, while trails only
  explain measured navigation. Keep those meanings distinct; do not grow
  another inferred-relationship view or duplicate these existing proposals.

- Commit handoff: the single staging/commit attempt failed creating
  `.git/index.lock` on the read-only mount. The complete green changes,
  exported queue, cycle log and excluded `.bosun_commit_msg` are ready for
  the driver. No commit retry or remote operation was attempted.


### Cycle observations (2026-10-28 — single-event prepared recovery)

- Health started clean at `ba8ee6b3`, with no quarantine and a clean working
  tree; all required baseline checks passed. Completed first ready assigned
  `brwr-3cc`, with the new-format regression failing first against version 1.
- Prepared bytes are bounded per journal (512 KiB entry, 1 MiB JSON), not a
  global retention policy. Journals remain the event-ID authority as before;
  future cleanup must not forget duplicate IDs. The raw payload hash includes
  text past display truncation and shares one fixed tuple across delivery paths.
- Old v1 intents have no prepared bytes to recover from: only exact known
  renderings may complete them. `brwr-2za` still needs the second frozen
  generation when daily-log escaping changes; this task does not close it.
- Full diff reviewed locally, including the version-specific legacy fixtures.
  Full required and README gates passed: 923 Rust outcomes, including 43
  explicit IPC sandbox skips, all 15 extension files, two script files, and
  10,696 inert Markdown renderings. No remote Codex review was invoked under
  the no-live-API constraint; the driver supplies independent review. macOS
  headless smoke remains unavailable on this Linux VM; no browser/UI changes.
- Continue `bd ready --label assigned`; `brwr-r88` is next. Read-only viewing
  of ambiguous preserved notes and explaining skipped capture (`brwr-vny`)
  remain useful visible improvements already queued; no duplicate proposal
  or new robustness task is needed while assigned work remains.
- The single commit attempt failed at `git add` because `.git/index.lock`
  cannot be created on this read-only mount. No retry or remote operation:
  the completed green diff, closed/exported bead and excluded
  `.bosun_commit_msg` are ready for the driver.

### Cookie cycle — preserved artifact-note viewing (2026-10-28)

- Began from a clean tree without a quarantine report; every required health
  check passed. Claimed and completed the first ready assigned bead,
  `brwr-r88`. Read the complete code, tests, generated protocol and doc diff
  fresh, and reused existing response validators and vault scan boundaries.
- The explicit read-only lookup is intentionally independent of note identity:
  canonical-address equality is useful for a person choosing what to read but
  remains insufficient evidence for an autosave or migration. No new ordinary
  note-load scan or dependency on index.db was introduced. The limitation is
  documented: today's stripped page address must equal the stored creation
  address, and the bounded scan can omit notes.
- Required and README gates passed: 955 Rust outcomes (912 exercised and 43
  explicit IPC skips), all 15 extension files, 144 panel cases exercised
  individually, two script files and 10,696 inert Markdown renderings. No
  remote Codex review ran under the no-live-API instruction; the driver
  supplies independent review. Interactive macOS Chrome styling and keyboard
  acceptance remain human work, not claimed by the mocked panel checks.
- Test development exposed two fixture assumptions, both corrected: fake DOM
  `disabled` does not initialize from HTML, and Serde does not enforce schema
  array caps. Panel waits now observe an actual note request; wire bounds use
  the existing schema validator. These are recorded in TESTING.md.
- Continue assigned work with `bd ready --label assigned`; `brwr-3rt` is next
  unless the driver changes the queue. Existing `brwr-kws` individual recovery
  review and `brwr-vny` capture explanations remain useful visible follow-ups;
  prefer those over another internal robustness idea. A future search view
  should reuse this deliberate read-only boundary instead of changing note
  identity from a selected search result. No duplicate proposal was added.
- The single commit attempt failed at `git add`: `.git/index.lock` is on a
  read-only mount. No retry or remote operation was attempted. The completed
  green diff, closed/exported bead and excluded `.bosun_commit_msg` are ready
  for the driver to commit with Cookie as author.

### Cycle 13 — Capture explanations and isolated host fixtures (2026-10-28)

- Health started clean: no gate-failure quarantine and no incomplete working
  tree. The complete baseline passed before assigned work began. Completed
  `brwr-3rt` and `brwr-vny`; both beads are closed and exported. Follow the
  remaining assigned queue with `bd ready --label assigned` next cycle.
- Final required and README gates pass: 956 workspace Rust outcomes (43
  explicit IPC-denial skips), 819 scripted-dialogs outcomes (53 explicit IPC
  skips and one ignored documentation example), all 15 extension files,
  158 panel cases exercised individually (+14), both script files, and
  10,696 inert Markdown renderings. Four new config-path process tests pass;
  the ordinary build exercises the shared override-exclusion test too.
- Fresh full-diff review checked recognition precedence, policy revision
  changes, action wiring, privacy and config/cache isolation. It caught
  repeated live-region text replacement, which now updates only when the
  reason changes. Test development also corrected an invalid URL-shape
  placeholder and an assertion that expected parser details instead of the
  host's intentional generic configuration error; exact file revision now
  proves which configuration was read.
- macOS smoke explicitly refuses Linux; Windows execution is with CI. No
  scripted dialog or smoke-step count changed. No remote Codex review was
  run under the no-live-API instruction; the driver supplies independent
  review. No live service, deployment, credential or git remote was touched.
- Product direction: the capture explanation answers a concrete question
  without becoming another monitoring view. Keep further capture diagnostics
  subordinate to the remaining assigned work. Existing individual recovery
  (`brwr-kws`), curated worklog sharing (`brwr-ppn`) and explicit focus sessions
  (`brwr-ued`) offer more visible value than another internal status surface;
  reuse those proposals rather than filing duplicates. Scope explanations
  should continue to point to deliberate rule review, never silently widen
  capture in response to a skipped page.
- The required single commit attempt failed at `git add` because
  `.git/index.lock` is on a read-only filesystem. No retry or remote operation
  was attempted. The green working tree, closed/exported queue and excluded
  `.bosun_commit_msg` are ready for the driver to commit as Cookie.

### Cycle 14 — Inert daily-log text with historical recovery (2026-10-28)

- Health started clean: no quarantine notice and no incomplete working tree.
  The required baseline passed before Cookie claimed the first ready assigned
  bead, `brwr-2za`. It is now closed and exported; continue assigned work with
  `bd ready --label assigned` next cycle (`brwr-srp` is next unless the driver
  changes priorities). Do not take blocked index work ahead of its dependency.
- The title regression first failed with live `$x$` and code syntax. New prose
  uses the existing inline escaper. Historical v1 recovery now has two frozen
  renderer generations, including a frozen Unicode table: this deliberate
  duplication is part of the on-disk compatibility contract, so do not merge
  it back into live presentation helpers. V2 prepared-byte recovery and
  summary rendering did not change. Old rows stay append-only.
- Fresh full-diff review covered current and historical renderers, prefix
  reconstruction, payload hashes, exact tails, fallback text, limits and
  single/batch paths. Two independent historical literals cover ten restart
  recovery scenarios; five new-write scenarios cover inert output and original
  marker text. Changed payloads and edited torn bytes remain untouched.
- Required and README gates pass: 933 workspace Rust outcomes and 821
  scripted-dialogs outcomes (plus one ignored doctest), including all 54
  capture tests; 15 extension files, two script files, and 10,696 inert
  Markdown renderings. Some IPC tests return explicit sandbox-denial skips
  within those Rust outcomes; the driver/CI supplies unrestricted verification.
  The macOS smoke runner refuses Linux. No remote Codex review ran under the
  no-live-API instruction; the driver supplies independent review.
- Product direction: the next assigned parameter-provenance work should store
  parameter names without retaining stripped values, and preserve the rule
  that a stored canonical URL cannot establish autosave identity. Individual
  recovered-copy review (`brwr-kws`) remains the visible follow-up for note
  recovery; timestamps and separate copy/discard actions can help a person
  choose a draft without introducing automatic restore. Reuse those existing
  beads rather than duplicating proposals or adding another diagnostic view.
- The one authorized commit attempt stopped at `git add`: `.git/index.lock`
  is on a read-only filesystem. No retry or remote operation was attempted.
  The green complete diff, closed/exported bead, cycle log and excluded
  `.bosun_commit_msg` are ready for the driver to commit as Cookie.


- **2026-10-28 recovery review observation:** Individual copy review completes
  the visible follow-up to durable note recovery (`brwr-kws`). Keep recovery
  as a deliberate copy-out workspace; automatic Restore would have to resolve
  changed host keys/revisions and risks blurring that boundary. For later
  product cycles, the existing named focus sessions (`brwr-ued`) and curated
  worklog sharing (`brwr-ppn`) offer larger user value than adding further
  recovery infrastructure. Continue the remaining assigned queue first:
  stripped-parameter provenance (`brwr-srp`) and historical UUID recovery
  (`brwr-smu`); concurrent index work (`brwr-7vf`) remains dependency-blocked,
  and the deleted-finisher wording report (`brwr-iqx`) needs operator
  reconciliation. No new duplicate proposals or human gates were closed.

- **2026-10-28 `brwr-kws` handoff:** The single git add/commit attempt
  refused `.git/index.lock` on the read-only Git mount. The completed green
  diff, closed/exported bead and excluded `.bosun_commit_msg` remain for the
  driver; no commit retry or remote operation was attempted. Queue consistency
  passes. The required and README gates passed before documentation-only
  closure updates; the macOS smoke explicitly refused this Linux platform.

### Stripped-parameter provenance handoff (2026-10-28)

- Completed assigned `brwr-srp` after a clean health gate. Continue with
  `bd ready --label assigned`; `brwr-smu` is the remaining ready historical
  UUID recovery task. Preserve all pre-provenance v1 presentation candidates
  and the frozen v2 payload hash while adding that compatibility. Concurrent
  index work (`brwr-7vf`) still depends on real SQLite (`brwr-b2e`), and
  `brwr-iqx` remains blocked for operator reconciliation.
- Source/test/doc review found an implementation regression where early URL
  evidence parsing changed malformed activity URLs from terminal rejection to
  retryable. A failing-first regression covers page, mode and trail URLs in
  single/batch delivery. Capture now gathers evidence during its original
  authorized normalization, keeping the previous error semantics and avoiding
  duplicate URL parsing. The complete diff was read, including the final fix.
- Names cannot recover a removed value or distinguish values of the same
  parameter. They are negative identity evidence: refuse a collision, never
  recover an autosave target from a stripped address. Missing legacy evidence
  must not become an empty list on cache migration or the next note save.
  Future real-SQLite work must retain the v7 event evidence and SQL NULL
  distinction; preserve trail-source separation from note candidates.
- Existing focus sessions (`brwr-ued`), curated worklog sharing (`brwr-ppn`)
  and deliberate restoration of recorded capture rules (`brwr-cua`) remain
  stronger product opportunities after assigned work than another internal
  diagnostics panel. Reuse those beads rather than filing duplicates. Explicit
  preserved-note viewing remains the copy-only route for ambiguous records.
- Required and README gates pass, including scripted-dialog tests and both
  Clippy builds. IPC guards explicitly skip 43 ordinary and 53 scripted cases
  in this sandbox. The macOS smoke runner refuses Linux; no browser acceptance
  is claimed. No live external API, remote Codex review, credential, deployment,
  service, systemd or git remote was touched; independent review belongs to the
  external driver under the unattended-run instructions.

- The single commit attempt refused `.git/index.lock` on the read-only Git
  mount. The complete green diff, closed/exported queue and excluded
  `.bosun_commit_msg` remain for the driver; no retry or remote operation was
  attempted. Final queue/current-state consistency and whitespace checks pass.


### Historical UUID recovery handoff (2026-10-28)

- Completed assigned `brwr-smu` from a clean opening gate. Both new regression
  matrices failed before the fix, then passed with 27 restart/recovery cases
  and six reindex cases. Complete source/test/doc diff reviewed for hash and
  tail integrity, UUID identity, cache deduplication, legacy renderer reuse,
  hostile payloads and bounds; no further defect found. The original queued
  UUID spelling remains necessary for a hash-only intent; never guess an
  arbitrary spelling or rewrite an authoritative log to recover it.
- The required and README gates are green: 972 workspace Rust outcomes with
  43 explicit IPC skips, 835 scripted-dialog outcomes with 53 IPC skips and
  one ignored doctest, both Clippy configurations, 15 extension files, two
  script files and 10,696 inert Markdown renderings. The macOS smoke runner
  refused Linux before browser setup; its three picker responses are unchanged.
  No live harness, external API or remote Codex review ran; the driver provides
  independent review and unrestricted IPC verification.
- Remaining assigned work has external blockers: `brwr-7vf` needs `brwr-b2e`,
  which needs human-owned `brwr-mff` to add/cache bundled rusqlite; `brwr-iqx`
  needs operator reconciliation because VaultCheckFinisher no longer exists.
  Neither assigned item nor any human gate was closed. A later real-SQLite
  implementation must preserve canonical event IDs and unknown legacy URL
  provenance when moving away from snapshots.
- Product direction: existing `brwr-ued` focus sessions and `brwr-ppn` worklog
  curation remain worthwhile larger steps after assigned/ordered work clears.
  Show known missing capture alongside worklog sharing (`brwr-9r7`) so a
  person can distinguish an incomplete record from an uneventful day. Reuse
  those proposals; another internal recovery/status surface is lower value.
- The single authorized commit attempt stopped at `git add` because
  `.git/index.lock` is on a read-only filesystem. No retry or remote command
  ran. The complete green diff, closed/exported bead and excluded
  `.bosun_commit_msg` are ready for the driver to commit as Cookie.

### Re-learning preview handoff (2026-10-28, starting at 5fb15210)

- The opening tree was clean, with no gate-failure file. All required opening
  checks passed: 972 reported Rust passes (including guarded IPC skips), 15
  extension files, both Clippy configurations, generation, build, fmt, offline
  npm install, typecheck and lint. README checks also passed: current-state,
  queue consistency, two script files and 10,696 inert Markdown renderings.
- During this run the coordinator closed obsolete `brwr-iqx` and removed the
  assigned label from dependency-blocked `brwr-7vf`. Cookie preserved those
  external decisions in the export and reconciled this markdown. Bundled
  rusqlite is still absent; real SQLite and concurrent index work still need
  human `brwr-mff`, followed by `brwr-b2e`. No human gate was bypassed.
- With no assigned work left, plain `bd ready` selected P2 `brwr-6x2` ahead of
  the P3 proposals. The full feature is deliberately split into coherent
  stages; alias rehearsal is an inspectable developer tool, not a claim that
  the panel can re-learn or that any stored note identity has changed.
- Next implement a revision-bound migration impact planner, explicitly handling
  splits, simultaneous key reuse/swaps, existing notes on both sides and
  stripped-query evidence. A sequential alias chain is not sufficient proof
  that a simultaneous policy replacement should merge those records. Preserve
  canonical-address versus autosave-identity separation and local note recovery.
- The existing larger product proposals (focus sessions `brwr-ued` and worklog
  curation `brwr-ppn`) remain useful after the plan work. Do not add duplicate
  proposals or another internal status view to fill the queue.
- Final gate passed with 987 reported Rust passes (+15), all 15 extension
  files, both Clippy configurations and README extras. The full diff was read
  fresh, including queue-field changes and new files; the documented JSON
  example also ran through the real binary. No remote/API review ran; the
  driver provides independent review. Evidence: `target/cookie-relearn-gate.log`.
  The next ready prerequisite is `brwr-hq8`, and parent `brwr-6x2` stays in
  progress. No macOS/browser acceptance was claimed on Linux.
- The single commit attempt failed at `git add`: `.git/index.lock` is
  read-only. No retry, push or merge ran. The complete green working tree,
  exported queue and excluded `.bosun_commit_msg` are ready for the driver.

### Worklog capture-gap handoff (2026-10-28)

- Opening health was clean: no gate-failure file, no incomplete working tree,
  and every required command passed before feature work. The existing P2
  assigned items `brwr-bzx` and `brwr-9xg` still require external platform
  evidence; their notes preserve prior operator history and state the Linux
  limitation. No remote was contacted and neither item was closed.
- Completed assigned P3 `brwr-9r7` ahead of self-directed work. Capture gaps
  now appear where someone reviews and copies a worklog. The additive v8
  report returns work and notices together; original worklog clients keep
  their existing message shape. A checked-by-default inclusion choice keeps
  the preview identical to Copy, while notices remain visible when excluded.
- Real capture tests caught the difference between standalone overflow and
  markers attached to a normal batch (URL/provenance and anchor time). Both
  now read correctly, including a long anchor URL and a different loss time.
  Missing records remain system evidence: no invented focus or artifact rows,
  no summed overlapping counts, and no guessed allocation across days.
- Format-8 caches use separate gap checkpoints. Upgrades preserve the event
  checkpoints needed to detect deleted/truncated logs, and rescan once under
  the existing read bound. Reindex reproduces notices without rewriting logs.
  Partial passes remain unavailable; the following query resumes them.
- The complete code/generated/doc diff was read fresh. Reused the existing
  checkbox style and date/range authorization path. Full gate evidence is
  `target/cookie-worklog-gaps-gate.log`: 990 reported workspace outcomes
  (43 explicit IPC skips), 853 scripted outcomes (53 explicit IPC skips),
  all 15 extension files, six new composed-panel cases, both Clippy builds,
  two script files and 10,696 inert renderings. The macOS smoke refuses Linux;
  CI must supply unrestricted IPC and actual Chrome acceptance. No real
  harness/API review ran; the driver supplies independent review.
- Next assigned product work is restoring recorded site rules (`brwr-cua`).
  Keep the host's original scope and custom identity visible in its review;
  the existing recording preflight is not itself a restore implementation.
  The prior Re-learn parent `brwr-6x2` remains in progress without enabling
  migrations. Do not bypass human gates to make these items look complete.
- Product observation: recorded gaps describe known buffer loss, not all
  uncaptured browsing. Avoid a completeness percentage inferred from them.
  For further sharing value, use existing worklog curation proposal
  `brwr-ppn` to select what is shared while retaining these honest notices;
  another diagnostics/status surface would add less value. The current
  proposal list already includes focus sessions and rule restoration, so no
  duplicate proposal or speculative robustness bead was added this cycle.
- The one commit attempt failed creating `.git/index.lock` on the
  read-only Git metadata. No retry or remote action ran. The green working
  tree, exported queue and excluded `.bosun_commit_msg` are ready for the
  driver to commit and independently review.

- 2026-10-28 Cookie cycle (`brwr-cua`): the assigned plan now restores usable
  recorded site recognition before offering new starters/drafts. Fresh diff
  review kept all overlapping history refused, including declared containers:
  note-only records can name multiple keys even when capture precedence was
  originally valid. `brwr-7lq` records the product follow-up and should use the
  existing `brwr-6x2` impact plan instead of another identity exception.
- Platform acceptance is the remaining bottleneck for `brwr-bzx` and
  `brwr-9xg`; they are blocked for external evidence. Future Linux cycles
  should report that once, then advance runnable assigned work. Focus-session
  grouping (`brwr-ued`) and curated worklogs (`brwr-ppn`) remain useful product
  directions after assigned/ordered work; another internal robustness layer
  does not outrank those user-visible capabilities.

- Commit handoff (`brwr-cua`): the single authorized `git add -A` / commit
  attempt failed creating `.git/index.lock` because Git metadata is read-only.
  No retry, remote access or push occurred. The final required gate is green,
  the queue is explicitly exported, and excluded `.bosun_commit_msg` contains
  the exact subject/body for the driver's commit and independent review.

- Supplemental validation limitation (`brwr-wg5`): the optional 5,000-case
  adapter comparison was interrupted after several minutes without a result;
  a bounded one-case probe timed out after 30 seconds. The existing IPC probe
  reports `spawnSync /usr/bin/node EPERM`. This comparison is not counted as
  passed; required gates, real Markdown renderers and script tests are green.
  Establish the runner's exact wait before changing it; no engine was touched.


### Platform evidence handoff (2026-10-28, rechecked at 8916bfc6)

- Health began clean: no quarantine notice or working-tree changes. All
  required local commands passed: protocol generation check, workspace build,
  fmt, both Clippy configurations, npm installation, typecheck, lint,
  extension build, all 15 extension test files and workspace Rust tests.
  Rust reported 997 successful outcomes: 954 executed and 43 explicit
  sandbox IPC early skips; these do not establish platform execution.
  Extras passed: current-state/queue checks, both script test files and 10,696 inert
  Markdown renderings. The scripted-dialog test suite was not rerun this
  cycle; its Clippy build passed.
- Both assigned beads remain blocked. `brwr-bzx` needs current Windows/macOS
  CI results against a named commit. `brwr-9xg` needs actual isolated macOS
  Chrome execution: `npm run smoke:macos:headless` again refused Linux before
  setup, and no Chrome/Chromium executable was found on PATH. The expected
  dialog count stays 14. The operator/driver must supply these results;
  implementation and skip-reason audit are already complete.
- No feature or source change shipped. Existing bead notes were preserved,
  current results appended and the queue exported. No assigned label, status
  or human gate changed. Review covers the complete documentation/queue diff;
  an API-based Codex review is prohibited by this run's no-live-API rule.
- Scheduling observation: repeated Linux-only cycles cannot remove either
  blocker. Obtain platform results or assign runnable work before launching
  again. Existing focus sessions (`brwr-ued`) and curated worklogs (`brwr-ppn`)
  remain useful later product work; no duplicate proposal or unrelated
  implementation was added ahead of this queue. This handoff updates the
  previous platform-evidence summary instead of adding another duplicate.
