# Pending AMOS Documentation Updates

- **Brauser — protocol v5 adapter configuration:** Brauser now persists format-versioned artifact adapters, validates their grammar, origins, scope, and overlaps in the native host, and makes policy faults and precedence warnings visible without disabling page notes or AI settings. Adapter changes require native consent, unscoped shared origins explicitly disclose every-account capture, and unsupported future formats fail closed while remaining intact for repair. This establishes the trusted configuration boundary for the upcoming worker prefilter and host recognition path.
- **Brauser — portable vault and harness verification:** Brauser now syncs capability-scoped directories through a readable self-handle, fixing Linux `O_PATH` failures without relaxing vault confinement, while harness fixtures use explicit secure modes and platform-correct permission checks. The complete Rust security, note, capture, and harness suite is reproducible under cooperative umasks again; macOS remains the only supported product platform.
- **Brauser — adapter-only capture is active end to end:** Confirmed adapter templates now travel in the worker's revisioned lease, where rule, scope, site, and live Chrome-grant checks keep unrecognized browsing out of durable extension storage. The native host independently re-evaluates every delivered original URL and title and suppresses stale or compromised-worker widening before vault access, making automatic capture artifact-only in practice rather than configuration alone.
- **Brauser — adapter review and match preview:** Brauser's settings page now makes artifact adapters understandable and editable without touching `config.toml`: readable policy cards cover rules, IDs, scope, titles, fields, overlap precedence, and folder-sync implications, while a native-confirmed JSON workspace validates the complete document and previews canonical IDs against an example plus fixed non-artifact safety probes. Add, edit, reorder, remove, and origin-revocation flows keep sites and adapters consistent across the worker and host.
- **Brauser — first-tool starter draft gallery:** Brauser settings now recognizes enabled Jira Cloud, SharePoint/OneDrive, and GitHub sites and opens exact-origin starter definitions in the existing match-preview and native-confirmation flow. GitHub requires explicit organization scope and never offers every-account capture, narrow site policies remain custom, and per-type docs clearly separate cross-engine recorded-fixture confidence from the live-tool checks still required for release certification.
- **Brauser — measured hourly artifact activity:** Brauser now replaces per-navigation capture rows with durable local-hour totals for cold opens, focused and background seconds, adapter modes, references, and trails, including restart map repair and clear live-versus-pending panel status. The native host revalidates artifact, mode, reference, and trail evidence before an idempotent append and stores a canonical stripped machine payload beside readable Markdown, giving the next index phase exact history without trusting worker claims or parsing prose.
- **Brauser — same-type adapter removal works:** Narrowing capture no longer fails when two adapters on one origin share an artifact type. The host compares counted ID-template sets so a deletion is distinguished from an identity-changing replacement, preserving both the user's ability to remove policy and the frozen-record-key safeguard.
- **Brauser — adapter IDs stay durable after removal or recognition edits:** The native host now retains compact, extension-invisible recognition and ID contracts for removed or superseded adapter routes, closing remove/re-add, recognition-edit, same-ID-reuse, and type/ID swap escape paths that could orphan artifact history. Exact-contract restoration and genuinely disjoint new routes remain valid until M1.5b alias migrations replace this conservative freeze.
- **Brauser — adapter faults no longer lock unrelated settings:** An invalid or future adapter document still suspends capture, but the host can now preserve that exact inert document while folder repair, AI privacy, harness setup, and Pause continue to work. Settings offers an explicit remove-all recovery action and verifies the host's post-save adapter status before publishing a worker lease, so recovery stays usable without weakening the fail-closed boundary.
- **Brauser — adapter edits stay attached across config refreshes:** The settings editor now keys an open edit by the chosen adapter's identity and canonical snapshot instead of its array position. External reordering follows the intended target, while a changed, removed, or ambiguous target disables Save and preserves the newer configuration for explicit review rather than overwriting a sibling adapter.
- **Brauser — adapter removals no longer re-confirm surviving rules:** Native consent now matches adapter deltas by stable origin, type, and ID, so removing an adapter or site takes effect immediately instead of treating every shifted survivor as newly enabled and potentially overflowing the confirmation dialog. Relative order remains a separate checked policy dimension, so genuine precedence changes still require confirmation and ambiguous identities fail closed.
- **Brauser — adapter-policy repair no longer loses queued activity:** Invalid or future adapter documents now produce retryable native acknowledgements for legacy visits, single hourly activities, and current mixed batches, retaining recognized events and overflow evidence until repair. Intentional Pause and valid rule misses stay terminal, so recovery preserves data without weakening user control or host-side filtering.
- **Brauser — Chrome revocations narrow host policy during adapter repair:** When Chrome removes an origin grant, settings now commits the narrower native-host site policy even if an invalid or future adapter document has suspended capture. The unsupported document remains structurally unchanged and inert for explicit repair, capture turns off when its last site disappears, and the worker acknowledges the revocation only after the host commit succeeds. Revocations are rechecked under the shared configuration lock so a stale snapshot cannot undo a concurrent explicit re-enable.
- **Brauser — adapter recognition is cached by host revision:** The service worker now compiles a stored adapter document once per host configuration revision and reuses the prepared matcher without serializing the full policy on every navigation. Non-expired untrusted install messages still compile independently, expired installs cannot replace the active matcher, and malformed restored state remains inert, preserving the fail-closed boundary while removing policy-size work from the browsing hot path.
- **Brauser — native adapter recognition is cached by configuration revision:** The native host now retains one validated Rust adapter document per adopted on-disk revision and reuses it across visits, hourly activities, mode and trail evidence, and why records. Unchanged native refreshes do not recompile policy, and pairwise overlap warnings run only for settings reads or after a successful commit; invalid revisions still suspend capture fail-closed.
- **Brauser — project status now reflects active artifact capture:** Contributor, product, and design summaries now describe the shipped revisioned worker lease, prefilter, independent native-host recognition, and measured hourly activity, while clearly retaining delivery compaction, indexing, record keys, and artifact-facing views as unfinished. CI protects that milestone boundary from returning to the obsolete pre-recognition description.
- **Brauser — remaining adapter review repairs:** GitHub starter cards now recognize installed PR and issue adapters without requiring scope text and never enable a missing draft. Unknown future keys inside `[adapters]` suspend only adapter capture while notes and other settings remain usable; the host preserves those values across unrelated saves and the explicit remove-all action repairs them without treating the whole configuration as unreadable.
- **Brauser — bounded durable activity delivery:** The worker now sends replay-stable, same-day capture batches with a frozen overflow snapshot, and the host commits validated survivors through one compact crash-recovery intent, one combined append, and one log sync. Pressure coalescing respects the hard byte cap, the toolbar warns at 80%, dropped-hour evidence reaches the log without being lost across acknowledgement retries, and deliberate `/log` actions are no longer near-repeat suppressed.
- **Brauser — rebuildable artifact index and bounded files:** Durable activity now updates disposable SQLite artifact, visit, edge, and dirty-set tables outside the notes folder, while lazy capability-scoped regeneration writes only owned, changed artifact Markdown with case-safe paths, hostile-text neutralization, and bounded daily/weekly/monthly Timelines. `brauser reindex` reconstructs both database and files from the append-only log, giving the upcoming artifact card, Recent, and worklog views a local source without introducing a service or making derived data authoritative.
- **Brauser — artifact history is now a daily workspace:** Recognized pages now expose measured artifact context and an append-only one-liner, while a filtered cross-tool Recent view and copyable daily or weekly worklog turn the local index into a visible product without an agent. Host-derived record keys join alternate routes to one note, guarded migration preserves older URL-keyed notes without clobbering, mid-edit identity changes keep copy-out text, and settings names artifact-file conflicts it refuses to overwrite.

- **Brauser — navigation trails accept only known browser transitions:** The native host terminally rejects invented transition values through both single and batch activity delivery, while retaining the worker’s back/forward encoding. Older bounded log/cache payloads keep their measured history and exact replay identity, but unknown transitions cannot become derived relationship edges; reindex preserves source logs. Three new regressions and the full 314-test Rust suite plus eight extension suites pass.

- **Brauser — Keep generated history readable and literal (2026-10-27).** Shared host presentation filtering removes invisible Unicode formatting from daily row titles/context, generated artifact titles/context, and worklogs, while generated Markdown escapes Obsidian comments, highlights, tags, and embed syntax. Exact encoded activity/context payloads remain authoritative and reindex leaves source logs unchanged. Two failing-first regressions and four new tests bring the passing Rust total to 319, alongside eight extension suites and the complete build/quality gate. Independent review added exact-hash recovery of interrupted entries from before this sanitizer upgrade; future renderer-independent intent storage remains tracked as brwr-3cc; macOS and Markdown-viewer acceptance remain separate.

- **Brauser — Share activity authorization across delivery paths (2026-10-27).** Single and batch activity now use one privileged host policy checker, preventing future drift in identity, mode, trail, reference and shape validation. A 13-scenario test covers 26 deliveries with explicit outcomes, no writes on refusal and decoded durable payloads on acceptance; the baseline already agreed, so no current bypass is claimed. The complete required gate passes with 322 Rust tests and eight extension test files; macOS acceptance remains separate.

- **Brauser — Recover missing artifact history from durable logs (2026-10-27).** Log ingestion checkpoints now repair valid older caches before history queries or subsequent delivery, including after restart without replaying the missing event. Durable capture acknowledgement remains independent of cache publication; artifact, Recent, and worklog queries explicitly report unavailable history when repair fails, then recover after the fault clears. Four new regressions cover real append/cache interruption, single and batch delivery, SQLite publication, duplicate replay, checkpoint migration, partial markers and bounds. All 326 Rust tests and eight extension files pass; macOS browser acceptance remains separate.


- **Brauser — Preserve shared and older notes after capture-policy removal (2026-10-27).** Adapter removal, recognition narrowing and site removal now retain shared notes and their separate older URL-note list across recorded primary, mode and explicit-context routes. Native request coverage proves capture stays suppressed, migration never overwrites an occupied note, stale keys preserve copy-out behavior and cache deletion/reindex retains identity. Five new tests and expanded ambiguity coverage pass with all 332 Rust tests and eight extension test files; macOS acceptance remains separate.
- **Brauser — Refuse incomplete historical identity scans before autosave (2026-10-27).** Primary note recovery now refuses file-count/byte-limit truncation and unreadable regular candidates instead of trusting a partial candidate set that can hide a conflicting artifact. Optional older-note discovery remains best effort. Four native scan-fault cases preserve text and recover after cleanup; the full build/quality gate is green. The discovered fix is brwr-t61; explicit read-only viewing of ambiguous preserved notes remains the P3 follow-up brwr-r88.

- **2026-10-27 — Capture Site for known work tools:** Brauser's side panel can now review and enable Jira Cloud, SharePoint/OneDrive, and independently scoped GitHub pull requests/issues from the current page, without a JSON editing step. The review checks the actual URL and fixed safety probes, and the revisioned save preserves pauses, other site rules and note edits while starting measurement on already-open recognized pages without inventing opens. Full checks passed (332 Rust tests, 10 extension files, 26 new scenarios). This is the known-site slice; unknown-site structural drafting and human live-tool/browser certification remain open, and no service or live deployment was changed.

- **Brauser — private page-pattern review:** The panel can now inspect the current page and same-origin link shapes on an explicit click, including on unsupported tools, with identifiers/query values replaced by placeholders, singleton link literals generalized and stale-page results discarded. Configured sites keep only 20 visited shapes for seven days in trusted local extension storage, with settings show/clear controls and pause/removal cleanup; samples never enter delivery or the notes folder. This ships the private input/review layer of unknown-site Capture Site, while host drafting, scope and rule enablement remain queued. Full gates pass with 332 Rust tests and 11 extension test files, including 21 new scenarios; real Chrome injection still needs macOS validation.


- **Brauser — complete sandbox-aware service tests (2026-10-27):** Six previously unguarded missing-service, launch and forwarding tests now use the existing IPC-denial guard. Local skips remain visible and CI requires execution; all six denial/required-failure paths were verified. Full required and scripted-dialogs gates pass with explicit IPC skips, and the existing 60-exchange dialog fixture is verified and closed in the queue. Production service behavior is unchanged.
- **Brauser — deterministic Capture Site attention verification (2026-10-27):** A reproduced hour-boundary test flake now checks both live attention and completed events queued for delivery, using fixed local clocks. Mid-hour and crossing-hour cases retain all 15 seconds without inventing opens. All 77 worker cases, 20 repeats, three time zones and three full extension-suite runs pass; no product accounting contract changed.

- **Brauser — recover unsaved panel notes (2026-10-27):** Closing the panel during debounce or an in-flight save now preserves the newest note edit in synchronous browser-local recovery. Reopening shows the current saved note plus separate Copy/Discard text, including conflicts and empty deletions; recovered text never saves automatically. Immutable per-snapshot keys prevent late acknowledgements and another panel's stale discard from erasing newer edits, and quota/storage failures warn while native saves continue. This restores the note-recovery slice of unmerged PR #67 and completes `brwr-3nq`. Full checks pass: 567 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and 2 script files, and 15 new regression scenarios. macOS Chrome validation remains open.

- **Brauser — show brief activity in shared worklogs (2026-10-27):** Worklog prose and artifact Timelines now display positive durations under a minute as `<1 min`, so short reviews no longer look like zero activity. Zero durations remain omitted and numeric frontmatter stays integer minutes. Completes `brwr-d3m`, with two regressions covering formatter boundaries and generated worklog/artifact output.
- **Brauser — load Recent during panel startup (2026-10-27):** Opening Recent before the native host connects now shows waiting text and then loads its list and worklog automatically, keeping Copy disabled until text is available. Three startup timing cases verify results and avoid duplicate requests while the worker installs policy. Completes `brwr-p9r`; all required local gates pass (569 exercised Rust tests, 43 explicit IPC skips, 15 extension and 2 script files). macOS browser smoke remains unexercised on Linux.


- **Brauser — retain captured work through index rebuilds (2026-10-28):** Activity and Why writes now store the same canonical UUID as their marker hash and live index, preserving accepted uppercase/unhyphenated IDs after a rebuild without double-counting immediate updates. Batch replies retain the original request ID for correlation. Completes assigned `brwr-q7e`; historical noncanonical marker/intent recovery remains `brwr-smu`. Three regression matrices first reproduced the loss and now verify stored payloads, rebuilt context/measurements and hash mismatch refusal.
- **Brauser — reject encoded URLs beyond the history limit (2026-10-28):** Capture now enforces the 8192-byte stored URL limit after authorization and normalization for page, mode and trail addresses, refusing oversize as `url_too_long` before writing a log or intent. Exactly-at-limit Unicode URLs remain accepted and survive reindex. Completes assigned `brwr-u8n`. Full local gate green: 755 exercised Rust tests, 43 explicit IPC skips, six new test functions across both fixes, all 15 extension and two script files; macOS browser smoke unavailable on Linux.


- **Brauser — offer older-note migration only when useful (2026-10-28):** Older notes retain read-only View, and Use as artifact note appears only while the shared artifact note is absent. Saves and stale-save responses update the action immediately, preserving current editor and recovery text. Completes assigned `brwr-v5t`; five new extension scenarios across the older-note batch cover the rendered actions, viewing and save transitions.
- **Brauser — preserve the source URL of migrated notes (2026-10-28):** Use as artifact note now records the verified older note's creation URL rather than the route where migration was clicked, retaining durable identity evidence after capture rules change. A failing-first host regression verifies migration and a subsequent save from the destination route. Completes assigned `brwr-w4k`; full gate green with 882 exercised Rust tests, 43 explicit IPC skips, 15 extension and two script files. macOS smoke remains unavailable on Linux.


- **Brauser — make capture tests independent of midnight (2026-10-28):** Capture and native unit-test fixtures now freeze a recent UTC noon per test thread so batches and full-log assertions keep one calendar day. A failing-first regression verifies both reported scenarios at 23:59:59 and midnight in UTC-12, UTC and UTC+14 without changing production delivery-window validation. Completes assigned brwr-mdn and duplicate brwr-mdt; full gate green with 883 exercised Rust tests and 43 explicit IPC skips.
- **Brauser — title summaries from the current item after in-app navigation (2026-10-28):** A unique current main-page heading now takes precedence over stale tab/document/Readability metadata, preventing a pull-request list title from naming its item's summary. Missing or ambiguous headings retain the existing fallbacks; title caps and Unicode repair remain. Completes assigned brwr-sti with four new extension regressions, including the real summarize entry point and URL-change refusal. All required local checks pass; real macOS browser acceptance remains separate.

- **Brauser — describe capture and unreadable tabs accurately (2026-10-28):** Enabled capture now describes sites with adapters using settings' shared wording, and unreadable Chrome/site-access tabs clear the artifact verdict instead of claiming Not an artifact. Readable unrecognized pages retain that verdict. Completes assigned `brwr-09c`, with four failing-first panel regressions.
- **Brauser — refresh artifact history after host recovery (2026-10-28):** A successful note autosave after a host outage now refreshes the card's status and measured totals, preserving an unsaved context draft. A later ordinary save sends no extra lookup. Completes assigned `brwr-b50`, with a failing-first composed reconnect regression.
- **Brauser — reflect background delivery after capture controls (2026-10-28):** Every session status update now observes the delivery counter, so Dismiss, Pause and Discard cannot hide an update from the poll and strand stale artifact totals. Changed generations refresh once; unchanged polls send nothing. Completes assigned `brwr-mm0`, with three failing-first action cases and the normal-poll control. Full local gate passed for the batch; macOS smoke unavailable on this Linux VM.

- **Brauser — Keep adapter repair reachable (2026-10-28).** Settings can now read a configuration whose adapter rules the host preserved for repair, including format 0 and documents with 65 adapters. Verified with failing-first regressions and the full local gate; host policy and file ownership protections remain unchanged.

- **Brauser — Explain a pending pause during a host outage (2026-10-28).** The panel now explains host outages and asks to reopen instead of pointing to a disabled Pause capture button. Normal finishing guidance returns once the host answers and the control is available. Verified with a failing-first outage/recovery regression and the full local gate (`brwr-gob`).

- **Brauser — Keep Why drafts private during pending removal (2026-10-28).** The artifact card's Save why action now respects the worker's pending site-removal list. Verified with failing-first regressions and the full local gate; host policy and file ownership protections remain unchanged.

- **Brauser — Keep activity references tied to the retained title (2026-10-28).** When queue pressure combines two undispatched activity totals for the same artifact and hour, the resulting refs now come from the event whose title is retained. Verified with failing-first regressions and the full local gate; host policy and file ownership protections remain unchanged.

- **Brauser — Follow a changed note identity before the next edit (2026-10-28).**
  An external settings or Capture Site revision now reloads the current note
  through the editor's serialized save/load queue. The next autosave uses the
  host's current artifact key; pending or in-flight drafts still receive
  conflict copy-out and durable recovery instead of being applied to another
  note. Four new panel cases and the full local gate pass (`brwr-jdi`);
  real Chrome timing remains macOS acceptance work.


- **Brauser — Check recorded identities before Capture Site offers Save (2026-10-28).**
  Starter review now checks the host's retired and hand-removed capture
  contracts before enabling Save, names any conflicting recorded identity,
  and discards verdicts for edited or closed reviews. Fresh compatible sites
  still save with one click, and the host independently validates every save.
  Seven panel cases plus native and wire regressions pass with the full local
  gate (`brwr-t8m`); macOS Chrome acceptance remains human work.


- **Brauser — Complete interrupted captures after the URL-limit upgrade (2026-10-28).**
  Existing v1 capture journals now get exact-byte recovery before the new
  normalized URL bound can reject a retry, including activity mode and trail
  URLs through single and batch delivery. Changed payloads remain retryable,
  new oversized events remain rejected, and valid batch neighbors still save.
  Two failing-first regressions and the full local gate pass (`brwr-u8r`);
  real macOS smoke remains unverified on this Linux VM.


- **Brauser — Deliver capture-loss evidence after the last batch (2026-10-28).**
  Overflow that arrives during the final native delivery now gets its own
  durable count/range marker without waiting for another captured page. Frozen
  IDs and snapshots make lost replies retryable without duplication; Pause,
  folder identity and owned-log checks still apply, and the extension supplies
  no URL or path. Three worker cases and four Rust regressions pass with the
  full local gate (`brwr-m64`); macOS Chrome smoke remains human/CI work.


- **Brauser — Reserve config space for ordinary settings (2026-10-28).**
  Retired adapter history now has a 32 KiB growth budget within config.toml's
  64 KiB cap. Older larger histories remain usable for unchanged or shrinking
  writes, and exact restoration or duplicate pruning preserves note identity.
  Settings receives specific budget/whole-file recovery guidance instead of a
  generic failure. Four new Rust regressions and the full local gate pass
  (`brwr-2iz`); no interactive macOS verification was performed on Linux.


- **Brauser — Show where a recorded artifact was opened from (2026-10-28).**
  Artifact cards now show up to five known incoming source/transition pairs,
  with safe explicit links, browser-navigation wording and the last recorded
  day. The host uses existing validated trails; missing, self, back/forward
  and unknown historical sources never invent provenance. Protocol v8 adds
  the required list; collection, logs and cache formats stay unchanged.
  Four new Rust and two panel regressions pass with the full gate and 1,680
  individually exercised extension tests (`brwr-1g2`). Actual Chrome anchor
  opening still needs macOS verification.

- **Brauser — interrupted log recovery survives presentation changes:** New single-event journals retain bounded prepared append bytes and a separate normalized-payload identity, allowing visits, activity, Why context and summaries to finish after renderer upgrades through single or batch delivery. Edited log tails, changed home folders and mismatched payloads remain refused; complete markers deduplicate and existing v1 frozen-renderer recovery remains supported. Four new regressions and the complete local gate pass; no live service or external API was used.

- **Brauser — read preserved notes without changing the editable page note:** The panel now lists up to 24 verified historical artifact notes sharing a stored address and opens an explicit choice in a separate read-only, copyable viewer. This makes ambiguous or forgotten-rule notes accessible without restoring capture, migrating files, or guessing an autosave key. Ownership and address are rechecked on every open; the bounded lookup works without index.db or recorded contracts. Seven new Rust and eight panel regressions pass with the full local gate; actual macOS Chrome interaction remains human verification (`brwr-r88`).

- **Brauser — isolate scripted host lifecycle tests across platforms:** Debug scripted-dialogs builds can use an explicit temporary config path, keeping configuration and cache state away from real user settings even when Windows Known Folders ignores HOME. Host-kill and knowledge-base picker fixtures use the seam; ordinary builds ignore it and release builds cannot enable it. Four process-level regressions cover loading, path refusal and cache placement (`brwr-3rt`); macOS smoke conventions and dialog counts are unchanged.

- **Brauser — explain skipped capture from the current page:** The panel now distinguishes pauses, pending site removals, excluded workspaces/accounts and unmatched page rules, with an existing settings or Capture Site review action. Explanations use the current compiled policy without inspecting links, inventing visits or changing capture. Fourteen new composed-panel cases cover policy changes, navigation and local status updates (`brwr-vny`); actual macOS Chrome interaction remains human verification.

- **Brauser — keep daily-log titles and Why context as literal text:** New
  visit/activity titles and Why text use the shared inline escaper so math
  and code syntax cannot activate a Markdown viewer's secondary engines.
  Both historical v1 renderer generations remain frozen for exact interrupted
  write recovery, while machine payloads and completed rows stay unchanged.
  Two regressions cover fifteen delivery/recovery scenarios, and the full
  required/README gate passes (`brwr-2za`). macOS browser verification remains
  outside this Linux run.

- **Brauser — individual recovered-note review:** The panel now shows each
  recovered copy with its page address, read-only text and separate Copy and
  Discard controls, so recovering one page never requires handling every
  unsaved note. Empty edits are clearly labelled, Copy keeps exact text, and
  Discard targets only the immutable snapshot selected. Live typing, other
  copies and newer snapshots from another panel stay intact; recovery never
  automatically fills or saves a note, and clipboard/storage failures remain
  visible and retryable. Completes assigned `brwr-kws`.

- **Brauser — names-only URL provenance:** Saved canonical addresses now carry
  the actual removed query names, never their values, through daily logs,
  cache format 7, a queryable SQLite table, notes and summaries. Older-note
  discovery and migration refuse evidence that lost a key read by current or
  recorded capture rules; primary autosave identity stays contract-derived.
  Legacy records remain readable with unknown evidence, note edits/re-keying
  preserve creation evidence, and frozen v1/v2 interrupted-write recovery is
  retained alongside new v3 intents. Ten new regression tests and the complete
  required/README gate pass (`brwr-srp`); macOS acceptance remains separate.

- **Brauser — historical activity/context recovery:** Older uppercase and
  unhyphenated UUID payloads now survive history rebuilds under a canonical
  cache identity, retaining measurements and Why context without rewriting
  daily logs. Pending v1 appends reconstruct their original queued UUID
  spelling across all three historical renderers, and finish only with exact
  intent hashes and tail bytes; edited or mismatched data remains untouched.
  Two failing-first regression matrices cover 33 cases, and the full required
  and README gates pass (`brwr-smu`). macOS browser acceptance stays separate.

- **Brauser — offline identity-correction preview:** Developers and agents can
  now rehearse alias chains, moves back and pairwise corrections with
  `brauser adapters aliases-preview`. The host derives endpoint identities
  from supplied adapter policies, checks scope and stripped-query evidence,
  and shows canonical groups separately from suggestions. It reads no live
  settings or vault and cannot apply a migration. Fifteen new regressions
  and the complete required/README gate pass (`brwr-sq9`). This is the first
  Re-learn prerequisite; impact planning (`brwr-hq8`), durable migration and
  panel review remain under parent `brwr-6x2`.

- **Brauser — capture gaps in shared worklogs:** The daily/week worklog now
  reviews recorded buffer-loss counts and full timestamp ranges, with an
  explicit, checked-by-default choice to include a clear incompleteness note
  in copied text. Copy matches the preview; notices remain visible when
  excluded. Retries deduplicate, overlapping ranges are never summed, and
  missing records never become measured work. Bounded cache upgrades and
  reindex recover the same notices. Assigned `brwr-9r7` is complete with real
  local log/reindex fixtures, six new panel cases and the full required/README
  gate green (990 workspace Rust outcomes, all 15 extension files). Actual
  platform acceptance remains separate.


- **Brauser — restore removed-site capture rules:** Capture Site now reviews
  usable host-recorded page kinds and account scope before proposing fresh
  rules, preserving custom artifact identities and existing notes. Only an
  explicit revision-checked save restores them; global pauses stay set and
  stale or changed reviews refuse. No page content or agent is needed.
  Damaged and overlapping history remains refused with repair guidance, and
  historical title/ref extraction is not invented. Assigned brwr-cua ships
  with seven Rust regressions, eleven panel cases, the full required Linux
  gate green (997 Rust outcomes, all 15 extension files) and 859 scripted-build
  outcomes. Platform evidence for brwr-bzx/brwr-9xg remains blocked externally.

- **Brauser — review capture re-learning impact:** The host's new
  `adapters relearn-plan` CLI reviews real owned log/note/summary evidence,
  including containers, before any rule correction can change identity.
  It lists splits, merges, swaps, reused keys, insufficient provenance and
  existing copies on both sides, bound to config/contract/evidence/proposal
  revisions. No content is moved and URL notes never choose an autosave key.
  Completes brwr-hq8 with 21 new tests and the full local gate green (1,018
  workspace Rust outcomes, all 15 extension files, 880 scripted outcomes;
  explicit IPC sandbox skips included). Durable journal/recovery brwr-wvi,
  file/index integration and panel Re-learn remain before parent acceptance.

- **Brauser — Task bar:** The side panel now has a free-text task bar under
  the page: the user types any instruction, presses Enter, and the configured
  full-access Codex or Claude Code carries it out through the Brauser service,
  with the open page as untrusted context it reads from a file. The typed
  request is the only directive; page steps run only when the request asks
  for them, and the prompt names Brauser's own folders as off limits. A task
  card streams progress and ends with a plain-text report, Copy report and
  Reuse; Brauser writes no record for a task. One task runs at a time beside
  summaries, at most four are active, the instruction and report never reach
  diagnostics logs, and the change is additive within protocol v8 (host and
  extension deploy together). Real-harness and hostile-page canary checks
  remain [human] work; notifications and task history are follow-ups.

- **Brauser — retain reviewed capture corrections across crashes:** The host's
  `adapters relearn-record` command revalidates and durably records a complete
  correction review; `adapters relearn-journal` inspects decisions and recovers
  exact interrupted appends. Stable UUIDs make retries idempotent, while stale
  reviews and conflicting payloads refuse. Rejected splits/merges/key reuse
  retain their simultaneous mapping. Completes brwr-wvi with 18 regressions
  and the full local gate green (1,037 workspace Rust outcomes, 899 scripted
  outcomes, all 15 extension files; explicit IPC skips included). Decisions
  always report `applied: false`; brwr-dbp still owns actual vault/index/config
  migration, with panel Re-learn remaining under parent brwr-6x2.

- **Brauser — Queue health repair (2026-11-01):** Reconciled stale local
  database-only Re-learn records and dangling exported dependency links,
  then regenerated the backlog. Preserved all 173 tracked issues and their
  statuses and labels. Assigned product work and human review gates remain
  open; this maintenance changes no product behavior or acceptance claim.

- **Brauser — Verify summary restoration after AI-draft bursts (2026-11-01):** Added service and built-panel regressions proving that per-kind listing preserves retained summary cards after more than 20 newer Capture Site AI runs. This completes an older follow-up whose implementation arrived with task history.

- **Brauser — Keep summary restoration compatible across list versions (2026-11-01):** Pinned summary/task restore isolation in both directions and added a single legacy summary-list retry when an older host explicitly rejects the filter field. Current hosts protect each kind before limiting; older hosts retain their shared-window limitation until rebuilt. Complete task history still requires the current listing protocol.

- **Brauser — Cancel stale Capture Site AI drafts (2026-11-01):** A settings change that invalidates an open review now detaches and cancels its queued or running AI draft. Pending starts are canceled when their identifier arrives, before polling or judgment, and the review retains its stale message and disabled save.


- **Brauser — Keep PR hygiene tests running after handoff edits (2026-11-01):** PR body edits now run Dev tools as well as PR owner validation, so canceling an earlier hygiene workflow cannot replace its Python test job with a skip. Uses the existing cancellation group; hosted event behavior was reviewed locally without a GitHub API call.

- **Brauser — Complete cleanup from a removable linked worktree (2026-11-01):** Worktree hygiene now runs control commands from the main worktree, allowing --repo itself to be removed without breaking later removals or stale-lock checks. A failing-then-passing throwaway Git regression verifies both removals, preserved branches and common-lock cleanup; all 75 Python dev-tool tests pass.


- **Brauser — Keep hidden text out of boxless-body extraction (2026-11-01):** No-article fallback now uses the rendered document root's `innerText` when the body has no box, preserving visibility filtering for `display: contents` and excluding hidden-body text. Extraction and mocked summary-entry regressions ensure hidden text cannot displace the visible document frame.

- **Brauser — Verify task tracking directly (2026-11-01):** Seven dedicated unit tests pin merged refreshes, one poll timer, completion announcements, off-page completion fetches, visibility and disposal through deferred host replies and mocked timers. Included in the normal extension suite; runtime behavior is unchanged.

- **Brauser — Remove the Tasks focus test's polling race (2026-11-01):** The regression now drives the existing visibility refresh and waits for rendered effects before checking retained-row and rebuilt-list focus. Ten isolated local repeat runs pass without depending on the two-second polling interval.

- **Brauser — Reserve tracked IDs before creating issues (2026-11-01):** Added `scripts/dev/create-bead` to validate and import committed issue history before creation, including closed IDs. Uses the existing timestamp guard, retains local-only work, preserves JSON output, and stops after invalid input or a failed import. Contributor guidance keeps linked worktrees on isolated scratch databases; nine regressions cover the workflow and duplicate-ID rejection.

- **Brauser — Bound adapter comparisons in restricted sandboxes (2026-11-01):** The differential test command now probes child-pipe access before starting either engine and reports “no cases compared” on a permitted sandbox skip; required CI mode fails on denial. A three-second probe bound and 60-second engine-batch bound stop stalls with actionable diagnostics, and incomplete engine output fails. Fifteen new regression cases preserve comparison and generator checks; supplemental regular-file transport matched 5,000 real Rust/TypeScript cases.


- **Brauser — Announce each watched task completion once (2026-11-01):** Brauser announces each watched task completion once per panel instance, even after stale running/start views (`brwr-jlz`).
- **Brauser — Preserve queued task refreshes after failure (2026-11-01):** Brauser preserves refresh requests queued behind failed task-list reads; merged callers receive the final follow-up outcome (`brwr-2p0`).
- **Brauser — Back off repeated task-tracking failures (2026-11-01):** Brauser backs off task tracking on consecutive list/off-page failures (2/4/8/16 seconds), pauses at five failures, and resumes on an explicit refresh or panel visibility refresh (`brwr-7u4`).
- **Brauser — Preserve task-list failure notices (2026-11-01):** Brauser keeps task-list errors visible when live or stored worker summary notices restore cards (`brwr-4r1`).
- **Brauser — Simplify tracker dependencies (2026-11-01):** Brauser removes TaskTracker's unused clock callback from its dependency contract (`brwr-ug2`).

- **Brauser — Stabilize supervisor timing tests on slow disks (2026-11-01):** Explicit temporary unit-test stores omit crash-durability flush waits while preserving real private writes, atomic replacements and filesystem failures. Production, scripted-dialogs builds and the normal storage tests retain durable writes. Supervisor deadlines remain unchanged; resumed-run vocabulary delivery is checked independently from its one-second timeout, avoiding a shell startup race (`brwr-pnti`).

- **Brauser — Keep Tasks navigation current during refreshes (2026-11-01):** The Tasks page coalesces overlapping refreshes, retries a queued refresh after failure, follows the latest hash and ignores pending replies after closure (`brwr-um4`).
- **Brauser — Drive Tasks polling tests with a mocked clock (2026-11-01):** Hidden and completed histories are checked across explicit polling intervals without real-time sleeps; deferred host replies reproduce refresh overlap and navigation races (`brwr-jzs`).
- **Brauser — Preserve simultaneous cross-view dismissals (2026-11-01):** An origin-wide Web Lock serializes dismissal storage updates across panels and Tasks tabs, with queue recovery after lock/write failure; independent module tests reproduce the lost-update race (`brwr-90b`).
- **Brauser — Test unknown run kinds at the schema layer (2026-11-01):** The wire regression now asserts schema rejection alongside Rust deserializer rejection (`brwr-3wh`).
- **Brauser — Verify Reuse fallback across windows (2026-11-01):** The regression asserts the source-window panel-open attempt and proves another window consumes the untargeted instruction when opening fails (`brwr-3pv`).
- **Brauser — Fetch off-page task states concurrently (2026-11-01):** Missing active rows now start get_run reads together, preserve partial successes and retry failed reads without rendering after closure (`brwr-sm8`).
- **Brauser — Align cursor timestamp lengths (2026-11-01):** RunCursor.created_at now shares RunView's 20–64 character schema bounds; boundary tests guard alignment without claiming timestamp syntax validation (`brwr-0at`).
- **Brauser — Add a built-UI accessibility gate (2026-11-02; validation pending):** Real-browser axe coverage spans 24 page/state/theme combinations, with keyboard flows and a seeded failure. Tab semantics, review focus return, polling/dismissal focus and theme contrast receive corresponding UI changes. macOS CI and Cookie's external gate require browser execution; the sandbox explicitly skips 33 browser cases. Keep `brwr-e7b0` in progress until that required run passes, and retain incomplete checks as manual review. This entry records implementation progress, not certified accessibility.
- **Brauser — Repair dark-theme accessibility gate failures (2026-11-02; external validation pending):** Recovered the quarantined accessibility implementation after the driver's browser run found serious Tasks warning contrast failures. The shared document now explicitly paints Canvas/CanvasText; two new theme-switch regressions require a measured warning contrast pass, and failures include axe's node explanations. There are now 35 browser cases plus two local fixture checks. Keep `brwr-e7b0` in progress for the required external rerun; this repair is not a claim of accessibility certification.
- **Brauser — Repair Tasks listbox child roles (2026-11-02; external validation pending):** Recovered cycle 3 after the browser gate rejected heading children in the Tasks listbox. Visible day labels still name their groups but have presentation roles. No-match searches hide the empty listbox and show a separate message, preserving the selected detail. Two controller regressions fail before the fix; a new browser case checks named groups and audits populated, filtered-empty and restored history. The 38-case suite includes 36 explicit browser skips in this sandbox. Keep `brwr-e7b0` open until the required external run passes.

- **Brauser — Complete accessibility recovery acceptance (2026-11-02):** The coordinator closed `brwr-e7b0` after PR #212 passed the required 38-case browser gate three times on macOS Chrome. The final repair makes the adapter match preview keyboard-focusable, waits for asynchronous diagnostics copy in the keyboard test and restores run-action focus without scrolling. This supersedes the pending-validation bullets above. The current repair cycle reconciles documentation and Shipped metadata; its sandbox browser guard still refuses required execution, and incomplete axe checks remain human work.

- **Brauser — Refuse damaged Re-learn log evidence (2026-11-02):** Strict review now rejects recognizable invalid owned markers with a path/line error, preventing a torn why marker from producing an approvable empty plan (`brwr-eo7`). Existing legacy and overflow forms remain valid, ordinary indexing retains tolerant recovery, and refused planning/recording writes no content or decision.
- **Brauser — Detect Re-learn collisions across origins (2026-11-02):** All-origin log, owned-note/summary and verified implicit-container keys now protect sources and destinations from hidden cross-site merges (`brwr-r62`). The report remains scoped to the reviewed site, all contracts bind the token, and fresh approval refuses conflicts. The assigned pair adds seven tests; the complete local gate passes, with explicit IPC/browser sandbox skips. Durable apply and the panel remain open.

- **Brauser — Preview corrected history before applying Re-learn (2026-11-02):** Host-only `adapters relearn-preview` shows corrected artifact totals, Recent/worklog rows, resolved links and container roll-ups from fresh owned evidence using one shared alias resolver. Original summary ownership/paths, logs, notes, config and caches stay untouched. Thirteen new offline regressions and the complete required/README gate pass (1,327 workspace Rust-reported outcomes; explicit IPC/browser sandbox guards). This completes prerequisite brwr-ruwj only; durable apply, note storage selection/stubs and panel Re-learn remain on assigned brwr-dbp/brwr-6x2.

- **Brauser — Preserve recorded incoming navigation in summaries (2026-11-02):** Summary runs freeze up to five host-derived incoming source/transition pairs, share that bounded untrusted context with the agent, and host-render a measured Incoming list with safe title links and last-recorded dates. The snapshot survives queueing, restart and Retry save; absent/legacy context adds nothing and grants no source retrieval. Ten new offline tests and the complete required/README gate pass (1,331 Rust-reported outcomes, including 92 IPC guard skips; 40 extension test files; both Clippy builds; 10,696 inert renderings). New nonempty run snapshots require the newer service. Assigned Re-learn work remains blocked on held PR #238, not included in this cycle.

- Brauser (2026-11-02, brwr-sm3): panel saved summaries and Recent now show
  the one-sentence overview with the document type, and summary run results
  lead with it. Older records retain their labels; the optional overview
  survives cache reopen/reindex. Deploy host and extension together; host
  downgrade after new summary events is unsupported.
- Brauser (2026-11-02, brwr-tsh1): retained summary history remains reachable
  after panel restore and harness removal, including dismissed summaries and
  summaries too old for panel cards. Tasks can still reveal dismissed runs.

- 2026-11-02 — Brauser (`brwr-tp1`): custom title capture rules and
  tab-title alias suggestions now retain up to two trailing base64 `=` in
  identifiers, matching URL behavior in both engines. Malformed padding and
  length overflow still reject the title entry; `=` is no longer a valid token
  delimiter. Added 26 shared vectors and verified 2,024 offline comparisons;
  full local gate passed with explicit sandbox skips. Fuzz compilation needs
  its uncached dependency and platform/browser evidence remains CI/human work.


- 2026-11-02 — Brauser (`brwr-sm7`): automatic summaries require the original
  page URL to match a current adapter, checked under the native configuration
  lock. Removed/narrowed rules still preserve existing notes and explicit
  summaries. An offline saved-note regression covers both changes.
- 2026-11-02 — Brauser (`brwr-sm9`): already-summarized suppression now expires
  by local calendar day, so viewer-only summaries can retry while Chrome stays
  open. Same-day suppression, pending dwell and retry state remain intact;
  the host continues deciding which saved summaries block another run.


- **Brauser — Honor capture pause when admitting automatic summaries (2026-11-02, brwr-q0i4).** The host rechecks capture_enabled under its refreshed configuration lock and skips paused automatic requests before service contact, including requests read under older settings. Manual summaries remain available; already admitted runs keep their lifecycle. An offline regression reproduces the old unwanted forwarding.
- **Brauser — Show summary reading scope beside the overview (2026-11-02, brwr-rr93).** Run cards and Tasks summary detail disclose the agent's whole-artifact, partial-artifact or browser-page-only reading through one text-only formatter. Legacy results omit the label and access walls retain their explanation. The optional protocol projection changes no stored answer or retrieval authority; deploy host and extension together.
- 2026-11-02 — Brauser (`brwr-sm5`): summary and task agents now learn when
  browser page text or selection was capped, including automatic summaries and
  queued recovery. Summaries must retrieve the full artifact or report partial/
  viewer-only reading; a page cannot forge the stored provenance. Host and
  extension deploy together, and service rollback requires no queued/running
  runs because older services reject the new true status flag.

- 2026-11-02 — Brauser (`brwr-2b5`): added the task-bar Chrome shortcut
  (Ctrl/Command+Shift+8), **Ask Brauser…** context menu and panel-local `/` focus.
  They preserve drafts and only focus the field; Enter/Run remains the start
  gesture. Short-lived tab-specific notices retain focus through panel startup.

- 2026-11-02 — Brauser (`brwr-18u`): ArrowUp/ArrowDown browse full retained task
  instructions and restore the unsent draft and recalled edits. History is read
  lazily from the service with no second persistent instruction store; late
  replies cannot replace typing, Reuse or a submitted task. Recall targets the
  current page; Tasks-page Reuse also reopens the original page.

- **Brauser — select captured work for a concise update (2026-11-02):** Recent offers explicit artifact-day checkboxes and a plain-text preview whose contents are copied exactly. Entries retain same-day titles, links and measured activity while excluding unselected container roll-ups. Same-period refresh preserves checked identities; period/home/host changes clear them. Recovered the unintegrated worklog capability onto the redesigned panel; no source history or notes change. Local browser/IPC sandbox skips remain subject to the driver's external acceptance gate; busy-day pagination stays queued as `brwr-p71k`.
- **Brauser — tasks without a page (2026-11-02, brwr-qin).** The task bar now works on non-web tabs, and Include this page lets a user run a knowledge-base instruction without attaching the active web page. Explicit scope is checked by host and service, omits page.md and page-reading permissions, and survives Tasks history/Reuse without navigation. Local required/README gates pass; real browser/IPC acceptance and publication remain driver/platform work. Brauser project documentation remains in this repository.

- **2026-11-02 — Brauser adapter repair (`brwr-jnj`):** Settings can copy
  preserved adapter values and preview a complete format-1 replacement while
  the original document is invalid. The host backs up the exact config before
  replacement, preserves pause/folders/agent settings, refuses stale or invalid
  submissions and retains recorded identity protection. Future formats remain
  opaque; remove-all recovery is still available. Project details remain in
  Brauser's README, DESIGN.md §7.2, TESTING.md and CookieFeature.md.

- **Brauser — preserve note ownership through re-keying (2026-11-03, brwr-c1bq):**
  Native note requests now distinguish logical artifact identity from the
  physical note owner, retaining original filenames and provenance while
  rejecting stale selection/history revisions. Twelve offline regressions
  cover original-path saves, canonical preference, ambiguous fallback,
  move-back/unalias, ownership/symlink refusal and logical native conflicts.
  Ordinary requests retain empty replay; durable apply and panel Re-learn
  remain assigned work. This is an internal prerequisite, not enabled
  migration. Brauser's design, staged plan and testing guidance stay here.

- **Brauser — keep finished task reports in their source notes:** Tasks now offers an explicit Save to note for successful page tasks. The host fetches the retained report, checks the home/settings revision, and appends Markdown-safe text through the ordinary note identity, ownership and version checks. Existing note text and later edits survive, exact report retries avoid duplicates, and task completion itself still writes no content. No-page tasks remain copy-only; report retention in a note is independent of service history.

- **Brauser named focus sessions (2026-11-03):** Explicit Start/Rename/End in the panel groups measured cross-tool activity in the worklog. Sessions survive restarts; pause and offline time stay unmeasured. Immutable name snapshots replay from logs without changing ordinary totals or capture authority. Names persist with captured activity and copied worklogs; empty sessions generate no record, agent run or network request.

- **Brauser personal summary style (2026-11-03, `brwr-sm4`):** Users can tailor
  summary language, emphasis and presentation through an optional home-root
  `SUMMARY_STYLE.md`. Each new summary snapshots bounded UTF-8 guidance before
  queuing, preserving it across restarts; compiled core instructions and answer
  validation remain. Settings, README and DESIGN.md describe defaults, repair
  and rollback. Offline tests use an in-process fake harness; no live model or
  deployed service was touched. Project documentation remains in this repo.

- **Brauser subprocess health checks (2026-11-04, `brwr-xkd1`):** Starter
  generation and PR-doctor integration tests now probe child IPC before
  creating fixtures or inspecting bd. Restricted workers run 23 pure tests
  and explicitly skip 13 subprocess cases; strict CI still rejects denial
  and retains the original integration assertions. The repair removes false
  health failures without changing application behavior or live services.

- **Brauser index contention budget (2026-11-04, `brwr-rx0s`):** Immediate
  SQLite busy replies now retry through one total ten-second lock-acquisition
  deadline instead of exhausting three attempts in about a quarter second.
  Each wait stays within the remaining budget, connection timeouts are
  restored, and transaction bodies run once. A real 400 ms writer reproduces
  the old failure. The separate Windows regression `brwr-epl1` remains open
  for Windows stress validation; project documentation stays in this repo.

- **Brauser — contradictory summary reading claims now surface:** A fresh
  answer reporting both an access wall and whole/partial artifact reading gets
  one correction turn, then a visible invalid-output failure if it remains
  contradictory. Valid page-only walls still save nothing and leave the page
  eligible for later summary; old stored answers remain readable. The summary
  instructions and validation now agree (`brwr-divh`).

- **Brauser — index bounds follow work rather than runner speed (2026-11-04):**
  Year-scale delivery tests retain row/event/transaction/log-byte bounds,
  ownership/file identity checks and full-rebuild equivalence while reporting
  elapsed time without fixed speed ceilings. The two reported held-lock tests
  retain exact one-wait and Busy/snapshot outcomes. Windows platform acceptance
  remains with CI (`brwr-gxag`).

- **Brauser — locate exhausted index contention (2026-11-04):** Structured
  diagnostics distinguish setup, schema/WAL, BEGIN, statement and commit
  failures with extended SQLite codes, elapsed milliseconds and attempts.
  Threaded and multi-process stress failures preserve the evidence; caller
  errors and retry behavior are unchanged. Seven new local regressions use
  real SQLite locks and isolated fixtures. The Windows root cause and repeated
  acceptance remain open on `brwr-epl1`; held PR #310 is untouched (`brwr-bszr`).
- **Brauser — share one recorded focus session:** Recent can narrow selected-work sharing to one explicitly chosen recorded session. The host filters activity before aggregation, preserving only that session's measured time and recorded page context; unchecked entries, exact preview/Copy, rename identity, explicit omission/unavailable states and immediate stale-preview invalidation keep sharing deliberate. Rebuild host and extension together. This is a standalone Brauser capability; no AMOS integration or live deployment is implied.

- **Brauser — saved summaries disclose reading coverage:** Recent and This page now show the agent's whole, partial or browser-page-only reading claim from validated summary log evidence, including summaries without an overview. Index reopen/rebuild preserves it, legacy scope stays unknown, and interrupted older appends recover exact bytes. Deploy host and extension together; older-host history reads cannot safely consume the enriched records.

- Brauser now runs reusable literal sidebar task commands from its task bar:
  local `[[commands]]` entries appear in Saved commands and `/name` invocation
  is resolved by the host under a revision check. Selection fills the draft;
  explicit Run starts the existing task workflow. Page context remains
  untrusted, stale choices preserve the draft, and Reuse keeps the instruction
  that actually ran. Built-in actions, templates and other outputs remain
  planned; users currently author commands in config.toml.


- **Brauser — edit saved task commands in Settings:** Users can create, edit
  and remove repeatable literal task instructions without editing config.toml.
  Revision-bound saves preserve unrelated raw settings and stale drafts;
  unsupported command registries have an explicit TOML repair view with an exact
  config backup. Saving starts nothing; Refresh commands and explicit Run retain
  the existing task flow. This is a standalone Brauser capability with offline
  host/UI coverage; deployment and browser/platform acceptance belong to the
  external driver and CI (`brwr-kqt4`).


- Brauser's Recent view can now find saved summaries by the agent's recorded
  reading coverage: limited, whole or unknown. It filters each day's latest
  displayed summary on the host before the result cap, making older limited
  readings easier to find without rerunning an agent. Unknown excludes days
  without summaries; existing history and content stay unchanged. Deploy the
  host and extension for the filter; All activity remains usable with older
  hosts. This is a Brauser feature, with source documentation kept in its repo.
