# Cookie Feature Notes

## 2026-10-16 — Host-validated adapter configuration in protocol v5

Brauser's adapter-template engine is no longer an isolated library. Protocol v5 carries a required format-versioned adapter document, and the native host serializes it as `[adapters] format = 1` plus `[[artifacts]]` tables in `config.toml`. Every adapter is parsed by the existing Rust authority, must use an origin already enabled for capture, and inherits the engine's shared-origin scope protections. The host also reports conservative overlap pairs so settings can warn that an earlier rule wins.

The failure behavior is intentionally narrow and recoverable. A newer or malformed adapter format is preserved without rewriting the user's file and disables only automatic capture; page notes and AI settings continue to work. The side panel, settings page, and autonomous service-worker renewal all suspend their policy consistently, while the host independently suppresses a stale queued visit before vault I/O. Adapter additions, edits, and precedence-changing reorders enter the native confirmation flow, with a plain every-account warning and separate-work-profile recommendation for unscoped shared origins. Existing adapter ID templates are conservatively frozen until alias migrations exist, so a config edit cannot silently orphan future artifact record keys.

This is the safe configuration foundation, not a claim that artifact logging is complete. The confirmed templates still need to travel in the worker lease; the TypeScript engine must prefilter navigation before buffering, and the host must re-evaluate every delivered event before writing it.

## 2026-10-16 — Portable vault and harness verification repair

The full Rust suite exposed two assumptions that made otherwise valid builds look catastrophically broken outside the macOS CI environment. On Linux, `cap-std` can hold a directory capability as an `O_PATH` descriptor, which supports confined relative lookups but cannot itself be synced. Brauser now opens the capability's own `.` entry as a readable directory before `fsync`, preserving the same confinement while restoring the durability sequence used by page notes and visit logs.

The harness process fixtures also depended on the shell's umask: a cooperative `0002` umask made fake install directories group-writable, so the host correctly rejected the fixtures before the tests reached their intended assertions. Those fixtures now set private permissions explicitly, including the separate symlink target directory, and the work-directory check chooses BSD or GNU `stat` syntax by platform. This does not expand Brauser's supported runtime platforms; it makes the repository's security and filesystem verification reproducible and leaves the deliberate unsafe-install rejection coverage intact.

## 2026-10-16 — Adapter-only capture at both trust boundaries

Configured adapter templates now decide what automatic capture means instead of sitting inert in `config.toml`. Every revisioned policy lease carries the exact format-1 adapter document confirmed by the host. The worker validates and compiles that document with the TypeScript adapter engine, checks that Chrome still grants the origin, and evaluates the original navigation URL before it reads tab metadata or places any page-derived content in durable extension storage. Pages that miss every rule or fall outside a configured scope simply never enter the queue. When an origin grant disappears, the worker narrows both the site list and the adapter document, preserving a valid fail-closed lease across later worker wakes.

Upgrade behavior is fail-closed without getting stuck. A cached lease written by the previous extension has no adapter document, so the worker treats that present-but-invalid value as a one-time reason to re-read the host before deciding whether to capture the first navigation. A genuinely absent lease and the explicit `null` used to suspend an invalid configuration do not start the host. This preserves panel-independent capture across the upgrade without turning an unconfigured browser into a periodic native-host wakeup.

The worker remains only a privacy prefilter. On every delivery, the native host holds the configuration lock, refreshes the current policy, and independently evaluates the original URL and title with the Rust engine before it opens the notes folder. An unrecognized or out-of-scope event injected by a stale or compromised worker receives terminal `suppressed` / `not_an_artifact` and writes nothing. Recognized events temporarily continue through the proven M1 visit writer; the next activity-events slice will replace those rows with one measured artifact total per local hour. The macOS smoke flow now installs its fixture adapter through the real native confirmation and update exchange before checking that the recognized route is logged and a sibling route is absent.

## 2026-10-16 — Adapter review and match preview in settings

Artifact capture is now configurable from the product instead of requiring a hand edit to `config.toml`. The settings page presents every adapter as a readable policy card: its ordered match rules, durable ID template, tenant scope or every-account acknowledgment, title behavior, and planned Tier 1 fields. Overlap warnings name both adapter types and the earlier rule that wins, and a persistent caution explains that URLs, titles, and configured fields can leave the machine when the chosen notes folder is synced.

An add/edit workspace provides the advanced adapter definition as JSON while translating its consequences live. It validates the single adapter and the complete resulting format-1 document with the same TypeScript grammar used by the capture prefilter, requires the exact origin to be enabled already, and shows what an example URL and optional tab title become: winning type and rule, canonical artifact ID, extracted title, and mode. It also evaluates the design's fixed non-artifact probes for roots, search and filter pages, login and dashboard pages, and auth, sharing, and capability routes, highlighting any match for review. User-written probe matches remain warnings rather than hidden policy changes. Saving, reordering, and widening still go through the host's revision-bound native confirmation and authoritative Rust validation.

The workspace also closes the lifecycle around origins. Removing an origin's final site now removes its adapters in the same host update, including when settings reconciles a Chrome grant revoked elsewhere, so the host never receives a dangling adapter for a disabled origin. URL-shape samples and counts of already-recorded artifacts that a rule would re-map remain explicitly deferred to the M1.5b Create adapter flow; that feature should extend this workspace rather than add a competing review screen.

## 2026-10-16 — Reviewable starter drafts for the first work tools

Brauser settings now turns a compatible enabled site into a useful starting point instead of asking every user to author adapter JSON. Root-scoped Jira Cloud tenants get an issue draft that joins canonical pages and board modals by Jira key; SharePoint and OneDrive for work tenants get document rules keyed by the stable `sourcedoc` UUID; and GitHub gets separate pull-request and issue drafts. Each generated definition resolves the recorded template to the site's exact origin and opens in the existing editor, where the user can read its rules, exercise the match and safety-probe preview, edit it, and then pass through the native host's normal confirmation.

The GitHub path is deliberately narrower than a generic template picker. GitHub hosts personal and work accounts on one origin, so both buttons stay disabled until the user enters valid organization owners. The gallery lowercases and deduplicates those owners, writes them only as `scope.owner`, and never offers an unscoped shortcut. Likewise, a site configured with a narrow path prefix gets no multi-route starter suggestion; Brauser leaves that deliberate boundary custom rather than implying it will work or asking to widen it.

The generated objects are regression-compared with the existing shared fixtures that both the Rust and TypeScript adapter engines run, and focused tests cover canonical IDs, route near-misses, GitHub modes, scope exclusion, and the full preview/save flow. Four adapter documentation pages record example routes, frozen ID contracts, scope behavior, and known limits. Because this isolated run was forbidden from contacting live services, the settings copy and docs call them development drafts: Jira, SharePoint/OneDrive, and GitHub still need a dated live-tool UI check before Brauser can call them release-certified starter adapters.

## 2026-10-16 — Measured hourly artifact activity

Brauser now remembers time spent with work artifacts instead of writing a new log row for every browser navigation. The service worker keeps one durable, offset-qualified total per artifact and local hour, recording cold opens, focused time while Chrome is frontmost, separate background time when the active Chrome tab remains selected while focus moves elsewhere, adapter-declared modes, title references, and capped trails showing how one recognized artifact led to another. Tab switches, focus changes, idle and lock events, SPA route changes, reloads, restored or discarded tabs, and new-tab targets all update the same coarse record. A worker waking after the hour boundary atomically moves completed totals into the existing durable delivery queue; the panel distinguishes current live totals from records awaiting the host. A restart rebuilds missing tab mappings from committed main frames without inventing opens, and existing M1 visits remain deliverable during upgrade.

The worker is still only a privacy prefilter. Every activity delivered to the native host is matched again against the current adapter configuration while the configuration lock is held. The host checks the primary artifact ID and type, replays each claimed mode against its evidence URL, verifies every trail source ID against its URL, and accepts only title references the adapter itself extracts. Only after those checks does it normalize allowed URLs, strip configured parameters, and append a concise measured line. The owned marker carries a bounded hex-encoded canonical event payload, so the forthcoming index can recover exact modes, trails, references, and `why` text without parsing prose; old visit markers continue to work. Dedicated tests cover repeated daylight-saving hours, focus/background/idle accounting, worker restart and map repair, SPA title ordering, reload and tab replacement behavior, legacy-queue draining, hostile identity and evidence claims, URL stripping, replay, and the panel-wake hour handoff. Actual machine idle/lock and cross-app focus behavior still need the already-documented interactive macOS verification.

## 2026-10-16 — Correct same-type adapter removal

Brauser no longer refuses a legitimate adapter removal when two adapters share an origin and artifact type. Previously, transition validation inspected each old adapter against the first surviving same-type sibling; deleting a `/tickets` rule could therefore make the surviving `/issues` rule look like an illegal ID-template edit. That left users unable to narrow capture through the adapter editor or through a site-removal flow even though removal is supposed to take effect immediately.

The host now compares counted ID-template sets within each origin and type. A pure subset is a removal, a pure superset is an addition, and reordered equal sets are unchanged; only a set that replaces one template with another triggers the existing migration-required refusal. Counting, instead of comparing only unique strings, also prevents a duplicate template from disguising a replacement. The regression was demonstrated failing before the fix and covers both the allowed removal and the still-refused sibling replacement.

## 2026-10-16 — Durable adapter ID contracts across removal

Brauser now remembers an adapter's record-key contract even after the user removes that adapter. Previously, the frozen-ID check compared only the two active documents in one update. A user could therefore delete an adapter, save, and later put the same route back under a different ID; changing the adapter type at the same time created a second one-step escape. Either path could leave existing activity, notes, and future index records stranded under a key the current policy would never produce.

The native host now writes a compact `adapter_id_history` beside the active adapter tables in the same atomic `config.toml` replacement. Each entry retains only the origin, recognition rules, scope behavior, and durable ID inputs needed to compare identity—never page content, titles, or captured activity—and the extension cannot view or edit it. A newly introduced contract is checked against retired overlapping routes with the adapter engine's conservative overlap analysis. A different type or ID is refused until alias migrations exist, but restoring the same ID and adding a truly disjoint route remain available. Restoring an exact contract removes its redundant history entry, keeping ordinary remove/undo use bounded; recognition edits and disjoint same-ID contracts retain the older route so it cannot later be remapped. The regressions exercise a direct swap, persistence through a host restart, the remove/re-add bypass, a disjoint route, recognition edits, same-ID reuse, and restoration of the original contract.

## 2026-10-16 — Usable settings during adapter-policy repair

An unsupported or malformed adapter document still suspends automatic capture, but it no longer traps the rest of Brauser's settings behind that fault. The native host now distinguishes the base configuration from adapter policy during a revisioned write. It accepts an already-invalid document only when the caller carries that exact inert document forward, which lets folder repair, AI privacy changes, optional harness setup, and Pause succeed; changing the adapter document still requires a fully valid replacement. Consent remains conservative if a base-policy change would make dormant adapters valid, and the on-disk adapter issue survives unrelated writes instead of being accidentally cleared.

Settings now keeps those unrelated controls available and shows a dedicated “Remove all adapter rules” recovery action. That action replaces the bad document with a valid empty format-1 document through the normal revision and host-validation path. After every config or harness save, the page re-reads the host's derived adapter status before it can publish a worker lease; an error, newer competing revision, or still-invalid document leaves capture suspended. Regressions cover the original host refusal, persistence across an unrelated write, harness setup, pausing capture, visible recovery controls, and the reset itself. A later P3 follow-up tracks copy-out and full-document replacement for users who want to salvage rules instead of discarding the document.

## 2026-10-16 — Conflict-safe adapter editing across settings pages

An adapter edit can now survive another settings page changing the configuration without silently jumping to a different rule. The editor previously remembered only the adapter's array index. If an external revision reordered or removed an earlier adapter, the refreshed page adopted that newer revision and Save replaced whatever adapter happened to occupy the old position; the native revision check could not help because the page was no longer stale.

Brauser now records the opened adapter's origin, type, ID, and a canonical structural snapshot. After a lease-driven refresh, it locates that exact unchanged target in the current document, so a harmless reorder preserves both the user's draft and the intended adapter. If the target was edited, removed, or became ambiguous elsewhere, the match preview turns into a clear conflict, Save is disabled, and the current configuration remains untouched until the user closes the stale editor and reviews the new version. The regression harness now emits real policy-storage changes and proves both the safe reorder and conflict paths end to end through the settings session.

## 2026-10-16 — Immediate adapter removal without survivor re-confirmation

Removing an adapter or an enabled site now narrows capture immediately, even when the removed entry appeared before many other adapters. Native consent previously compared each adapter with the same array position in the old document. Deleting the first rule shifted every survivor, so the host described all later rules as newly enabled; a sufficiently large set of unscoped adapters could exceed the confirmation-text cap and make removal impossible.

The host now finds each survivor by its stable origin, artifact type, and ID, then compares its complete definition. It tracks relative prior positions separately: deletion-driven shifts remain free, while swapping two rules still enters native confirmation because config order controls overlap precedence. A missing or duplicate identity is never guessed and remains confirmation-required. A failing-first 31-adapter regression covers the former refusal, and a companion test proves the reorder boundary remains intact.

## 2026-10-16 — Retryable capture during adapter-policy repair

Brauser no longer throws away recognized work activity merely because the adapter document becomes temporarily unreadable or comes from a newer format. Previously, the three delivery entry points disagreed: a legacy visit was terminally suppressed as an invalid policy, a single hourly activity looked like capture had been disabled, and the current mixed batch could discard activity as though the page were not an artifact. In every case the worker interpreted the answer as final and removed data that had been recognized under an earlier valid lease.

The native host now makes policy state explicit before it evaluates or writes any capture payload. An intentional Pause is still terminal and a valid policy miss is still suppressed as defense in depth, but an adapter-policy fault returns `retryable` for legacy visits, compatibility activity messages, and every event in a mixed batch. Batched overflow evidence also remains unacknowledged. Once settings repairs or replaces the adapter document, the worker can replay the exact queued events through the current policy instead of silently losing the interrupted hour. Three failing-first scenarios cover all delivery generations so extension upgrades cannot reopen the gap.

## 2026-10-16 — Host policy follows Chrome revocation during adapter repair

Brauser no longer leaves an origin enabled in native-host policy when Chrome revokes its permission while the adapter document needs repair. The settings session previously suspended the worker and stopped as soon as it saw `adapter_issue`; that was fail-closed in Chrome, but it left the host allowlist wider than the browser's current grant and retried the same unresolved revocation on every load.

Revocation reconciliation now commits the narrower site list under the shared configuration lock and disables capture when the last site disappears. A malformed or future adapter document is carried forward without structural changes, using the host's deliberately narrow inert-document exception, and remains suspended for explicit user repair; Brauser never tries to filter or reinterpret fields it does not understand. Reconciliation rechecks the worker marker after acquiring the lock, so an old snapshot cannot remove an origin another settings page just re-enabled. Only after the revisioned host update succeeds does the worker forget the revocation. If the host had already removed that origin, the stale worker marker is acknowledged while the bad policy remains suspended instead of being reinstalled. Three failing-first settings regressions exercise those exchanges and prove the site disappears, capture turns off, the unsupported document survives unchanged, stale cleanup cannot republish it, and a concurrent re-enable wins.

## 2026-10-16 — Revision-cached adapter recognition

Brauser's service worker no longer rebuilds the entire adapter-template grammar while handling ordinary browser events. A host configuration revision now owns one prepared TypeScript adapter document: the worker compiles it when a lease is installed, or once when that revision is first recovered from trusted storage after the worker starts, and every later URL match reuses it. This removes both the full validation pass on each policy read and the JSON serialization that previously built a cache key for every matching navigation, which matters most as a user adds more tools and rules.

The optimization preserves the trust boundary rather than making the cache an alternate source of policy. Every non-expired lease received through an install message is structurally checked and compiled again even if it claims a cached revision, while an expired lease is rejected before it can change the active revision's matcher. Malformed cold-start storage remains inert, and URL matching still begins with the lease's enabled-site and expiry checks before it touches the compiled adapter. Failing-first cache regressions count compiles across cloned storage reads, prove that repeated matching navigations serialize no adapter document, and keep invalid and expired install rejection explicit.

## 2026-10-16 — Revision-owned native adapter recognition

Brauser's native host now prepares an adapter document once when it adopts a new on-disk configuration revision and keeps that validated Rust object beside the snapshot. Visits, hourly activity, mode evidence, trail sources, and `why` records all reuse the prepared matcher after the host refreshes policy under its configuration lock. An unchanged refresh—whether triggered by Hello, a note operation, settings, or capture—therefore does not rebuild the grammar, and one activity containing several evidence URLs no longer recompiles the same policy for every URL.

The cache does not weaken the privileged boundary. A changed revision must compile and pass enabled-origin validation before it becomes the active document; a malformed or future adapter format leaves no matcher and continues to make capture retryable until repair. Conservative rule-overlap analysis is now separate from validity checking: settings computes warnings on demand, and a successful config commit computes them for the new snapshot, while ordinary native messages avoid the quadratic pair scan. A failing-first instrumented regression demonstrated three redundant compiles and one overlap scan for a refresh plus two matches before the change, then proves repeated matching and unchanged refreshes perform neither and that settings computes its warning set only once.

## 2026-10-16 — Contributor status reflects artifact capture as built

Brauser's contributor guide no longer describes its central M1.5a capture path as future work. The current-state overview now follows an artifact from the confirmed adapter document in the revisioned worker lease, through TypeScript prefiltering, to the native host's independent Rust recheck and the measured hourly activity record. It also names the pieces that really are unfinished—delivery compaction, indexing and artifact files, record keys, and the artifact-facing panel views—so a contributor can choose work from the actual product boundary instead of an obsolete one.

README's shorter product status and DESIGN.md's top-level status now agree with that boundary. A small CI check extracts all three status summaries, normalizes prose wrapping, rejects the stale claims that caused this blocker, and requires the shipped lease, prefilter, host-recognition, and activity facts to stay visible. Future milestone work can reword the prose, but it must update the protected fact set when the boundary advances.

## 2026-10-16 — Remaining adapter review repairs

Brauser's starter gallery now separates an offer's stable artifact type from the scope-dependent draft it can create. That lets settings recognize an already-installed GitHub pull-request or issue adapter before the user types an organization, while still keeping creation disabled until a valid explicit scope exists. The button state now also requires a real offer and adapter object, closing the JavaScript edge where a missing offer's `undefined` value compared unequal to `null` and made the button appear available.

The native host now isolates forward-compatibility failures inside `[adapters]` as carefully as it already isolated an unsupported format. Unknown future keys are captured in host-only TOML state, automatic capture remains suspended, and page notes plus unrelated settings stay usable. Those unrelated saves preserve the values structurally instead of silently dropping fields a newer Brauser may need. The explicit remove-all repair can still clear an otherwise empty future document: an exact no-op wire update is interpreted as that deliberate narrowing only while host-only adapter extensions exist. Failing-first regressions cover both starter failures and the full load, preserve, and repair lifecycle for an unknown adapter key.

## 2026-10-16 — Bounded, crash-recoverable activity delivery

Brauser now turns its durable worker queue into day-bounded native deliveries instead of paying the filesystem cost of one message and one intent file for every captured hour. Up to 64 ready visits or activity records share a stable batch ID; the native host rechecks each one against current adapter policy, then writes every surviving entry through a single compact in-flight intent, combined append, and log sync. If the host or browser stops mid-write, replay either finishes the exact byte prefix in the original selected folder or recognizes complete event markers. The old single-record path stays available only so a pre-upgrade queue can drain.

The user-facing pressure behavior is complete as well. At 80% of the count or byte cap the toolbar shows a warning badge, compatible undispatched totals can coalesce without crossing the hard byte limit, and any hours that still cannot fit remain visible in the panel as a count and time range until an overflow marker is durably acknowledged. The precise overflow snapshot travels with the batch across retries, so a lost acknowledgement cannot accidentally clear newer drops. M1's configurable near-repeat suppression is retired: older config files are accepted without applying it, and deliberate `/log` actions for the same page remain separate.

## 2026-10-16 — Rebuildable artifact index and files

Brauser now turns its append-only activity log into something the product can browse without making the log any less authoritative. After an activity survives the current native policy checks and reaches the daily log durably, the host updates a standard SQLite `index.db` outside the notes folder. Its artifact, visit, edge, and dirty-artifact tables retain measured totals, latest presentation, trails, title references, and pending regeneration state. A bounded atomically replaced ingestion cache lets each delivery update that snapshot without rereading years of logs, but neither cache is precious: `brauser reindex` validates owned machine markers and recreates the database and generated files from scratch. The SQLite regression opens both small and multi-page snapshots through Python's standard SQLite implementation rather than trusting Brauser's own encoder.

Artifact Markdown remains deliberately quiet. Capture only marks records dirty; it does not create a synced file for every busy hour. When the native connection closes, a best-effort pass writes each dirty record through the selected-folder capability, checks an existing file's `brauser.kind` and exact artifact identity before replacement, and leaves unrelated occupants untouched. Identical output keeps its original mtime. Filenames include a hash of the full artifact ID so case-sensitive IDs stay distinct on case-insensitive filesystems, while titles and `why` text are YAML-escaped or Markdown-neutralized. The Timeline keeps the latest 14 active days, then weekly rows through 26 weeks and monthly rows beyond that, so a year of daily opens stays to a few dozen lines. Older configs gain the first non-overlapping generated directory name (`artifacts` normally, or a `brauser-artifacts` variant if that location is already in use), and changing it follows the same non-overlap validation and native consent rules as every other content location. The artifact card, Recent/worklog views, conflict UI, record keys, and manual settings trigger remain the next plan step.

## 2026-10-17 — Artifact history becomes a daily workspace

Brauser's derived artifact history is now visible where a person can use it. On a recognized page, the side panel shows the artifact's stable identity, current mode, measured focus and background time, cold opens, active days, linked-artifact count, and the latest optional “Why I'm here” line. The new Recent tab combines artifact-day rows across every configured tool, groups them by local day, filters by artifact type or title, and opens the last known URL. It also turns the same measurements into a copyable worklog for today, yesterday, or the current week without invoking an AI harness. Settings completes the file lifecycle with an explicit regeneration action that reports written and unchanged files and names bounded unowned-path conflicts instead of overwriting them.

Notes now follow the artifact rather than whichever route happened to be open. The native host alone derives a URL or artifact record key; Google Docs `/edit` and `/view`, Jira board modals and `/browse/`, and other adapter-equivalent routes therefore load one note. Every save echoes the key it loaded as well as the file revision, so an adapter edit made while someone is typing produces a `key_changed` refusal and preserves their text for copy-out. Existing M1 and pre-adapter URL notes are not silently rewritten: the panel lists them, and **Use as artifact note** publishes a verified copy under the artifact key without clobbering an existing artifact note, rechecks that the source did not change, and only then removes the old URL-keyed file. All list, note-metadata, conflict-path, and worklog responses are bounded to fit one native message.

## 2026-10-27 — Keep navigation trails factual without dropping history

Brauser now refuses activity that claims an invented navigation type. Previously, an extension message could supply any short string as its trail transition, including “mentions,” and that text would reach the relationship index. The native host now accepts only Chrome’s known transition types and the existing back/forward encoding. Both single-event and batch delivery return a terminal rejection before changing the log or index, so a bad event does not keep retrying. The regression first demonstrated the old behavior by persisting an invented transition.

Existing history stays usable. The log reader retains the original bounds and source-pair checks, and keeps old activity totals and exact payloads even if a former version accepted an unknown transition. The index derives edges only from known transitions, whether it is reading an existing ingestion cache or rebuilding from owned log markers. Three new tests cover native rejection, browser-value compatibility, and real SQLite output before and after reindex; all 314 Rust tests and eight extension suites pass. No macOS browser run is claimed from this Linux VM.


## 2026-10-27 — Keep generated history readable and literal

Brauser now filters invisible formatting out of the text it generates for daily logs, artifact files, and copyable worklogs. A hostile page title could previously carry right-to-left overrides or zero-width characters into a row and visually disguise its contents; artifact titles in YAML and explicit context had the same exposure. One shared host sanitizer now replaces Unicode format characters, controls, and Unicode line separators with spaces. Generated Markdown also escapes image syntax and Obsidian comment, highlight, and tag delimiters, so captured titles and context remain literal. Ordinary non-ASCII letters, accents, and visible emoji remain readable.

The filter runs only when presenting text. Canonical activity and context payloads keep their exact encoded bytes, and reindex can regenerate safe owned artifact files while leaving the source log intact. Two regressions first demonstrated the old failures; four new tests cover single and batch activity/legacy delivery, exact markers, rebuilding, worklogs, input limits, and multilingual text. The full gate passed with 319 Rust tests and eight extension suites. Independent review then reproduced and repaired interrupted legacy-entry recovery for this sanitizer upgrade: the older rendering must match both the durable hash and the exact partial bytes, through single or batch delivery. Two further regressions cover edited tails, changed payloads, complete-marker deduplication, activity, and context. The broader renderer-independent intent representation remains tracked as brwr-3cc. No macOS browser or Markdown-viewer acceptance is claimed from this Linux run.


## 2026-10-27 — Keep single and batch activity authorization together

Brauser now uses one native-host policy checker for both compatibility single-event activity delivery and the current batched path. The source previously repeated recognition, artifact identity, mode evidence, trail-source, title-reference and collection-bound checks in two places. They agreed in the baseline, so this cycle does not claim to fix an existing bypass; it removes the opportunity for a later policy change to update one delivery path and miss the other. Existing config locking, unavailable-policy retries and intentional Pause behavior remain in place.

The new parity regression sends the same activity through both handlers in separate stores across 13 scenarios. It checks expected outcomes and reasons rather than mere agreement, verifies refused events create no log or index, and decodes the actual durable marker to prove accepted activity retains its payload, including valid mode, trail and reference evidence. Early failures were mistakes in the test's marker assumptions, corrected before establishing the passing baseline. All 322 Rust tests and eight extension test files pass, along with the full required build and quality checks. macOS browser and dialog acceptance remain separate; the smoke dialog count stays 14.


## 2026-10-27 — Recover artifact history after interrupted index updates

Brauser now recovers activity that reached the durable daily log but missed the disposable index. Previously, restoring a valid older cache after such an interruption left that activity absent from Recent and worklogs even when another event arrived; only manual reindex restored it. A failing native-host regression reproduced the problem before the fix. The cache now records each log file's length and modification time, checks those checkpoints before updates or history reads, and reparses changed files to recover every owned event marker. The next query or single/batched delivery catches up after restart without replaying the missing event, and unchanged logs are not reread. Re-reading a changed file also recovers a formerly incomplete trailing marker and preserves the preceding prose used by older visit markers.

SQLite publishes before the cache checkpoint, and reads that can publish repairs use the same configuration lock as capture and settings writes. Cache failure never changes a durable capture acknowledgement: fault tests confirm the log stays intact while artifact, Recent, and worklog requests explicitly report unavailable history. Once the fault is removed, a query repairs both caches and duplicate event replay still appends nothing. Four new tests also cover old-cache migration, dirty tracking, real SQLite rows, and bounded ingestion without partial publication. The provisional index still has hard file, read, and state-size limits; the existing real-SQLite task owns resumable ingestion beyond those limits. All 326 Rust tests, eight extension test files, and required build/quality checks pass. The macOS-only smoke runner refused this Linux VM; browser acceptance is not claimed and its 14-dialog expectation is unchanged.


## 2026-10-27 — Keep notes accessible when capture rules change

Removing a tool from capture should not make its notes look empty. Brauser already had partial historical-key recovery, but two native regressions exposed gaps: separate older URL notes disappeared from alternate-route lists after removal, and a route remembered only by “Why I’m here” lost access to the shared note. The host now uses the same durable URL evidence in both directions, including an activity’s primary URL, its mode URLs and explicit context before any measured activity. Recovered artifact notes retain the older-note list and read-only viewing action. A separate URL note stays separate, explicit migration cannot overwrite an occupied artifact note, and stale keys or versions still return the current content for the panel’s copy-out workflow.

The acceptance matrix sends framed native requests and reopens the host store between them, covering adapter removal, narrowed recognition and site removal, with existing caches, deleted caches and an explicit reindex. It checks exact note revisions and unchanged log bytes, proves subsequent capture is suppressed, and confirms notes remain editable even without delivered activity. A dialog callback replaces only the native confirmation drawing for rule edits; real consent tokens and the revisioned update handler still run. Five new Rust tests and expanded activity/context ambiguity coverage bring the full passing gate to 332 Rust tests plus eight extension test files. No browser or real macOS dialog acceptance is claimed; the Linux smoke runner refused execution and its 14-dialog expectation stays unchanged.

*Note (2026-10-26, protocol v6):* open access dropped the consent tokens and native confirmation described above; rule edits are plain revisioned `update_config` requests, and the macOS smoke no longer expects a fixed dialog total. Its answered-dialog counter must equal the `shown.jsonl` line count (see TESTING.md). The 14-dialog expectations in this and the previous entry are superseded.

## 2026-10-27 — Refuse an autosave target when note recovery is incomplete

Reviewing the removal path exposed a related safety problem: the optional older-note scanner deliberately skipped files it could not read and truncated large directories, but primary historical-key recovery treated that partial list as complete. A conflicting artifact note beyond the cap could disappear from consideration, leaving the host apparently certain about the wrong autosave target. The new strict scan shares the existing filesystem capability and limits while refusing a partial result: more than 4,096 Markdown entries, a regular file above 4 MiB, more than 16 MiB of read candidates, or an unreadable regular file makes historical identity unavailable. Symlinks remain skipped. Ordinary optional older-note discovery keeps its forgiving behavior on currently recognized pages.

A native regression first reproduced the partial-scan decision, then verified file-count, per-file, aggregate and non-UTF-8 faults through both load and save. Existing text stays unchanged, the host returns a correlated unavailable-identity error, and clearing the fault restores access. This discovered work is tracked and completed as brwr-t61 alongside brwr-ezb. A separate P3 proposal, brwr-r88, would let a person explicitly read preserved notes when multiple historical artifacts share one URL; this cycle retains conservative URL-key fallback and does not choose between them. The full required gate passes with 332 Rust tests and eight extension test files.

## 2026-10-27 — Capture known work sites directly from the page

Brauser now has a normal first-use path from the work a person is already reading. After choosing a notes folder, open the side panel on Jira Cloud, SharePoint/OneDrive, or GitHub and click Capture Site. Jira and SharePoint resolve the existing development starter from the site's origin. GitHub presents separate pull-request and issue choices, each with its own work-organization list; either kind can be left out. The inline review explains what Brauser remembers, checks the actual current page, labels illustrative examples honestly, and exposes all 27 root, search, dashboard, login, sharing and auth probes before the user saves. The existing starter definitions and their frozen IDs stay unchanged, and their live-tool certification is still pending.

The save uses one protocol-v6 configuration update under the same revision and mutation lock as settings. It rechecks the host and active tab, refuses an obsolete review, and preserves other sites and global pauses. It leaves existing site definitions in settings rather than attempting a re-learn without migrations. Notes finish saving before the host starts resolving artifact identities, preserve unsaved text as a copy-out if necessary, and reload afterward; a title refresh cannot reopen the editor in the middle of that save. Installing policy from a page also seeds recognized tabs already open in Chrome, so measurement starts immediately without counting an invented navigation. Background renewal keeps its real navigation accounting. If configuration commits but status refresh fails, the panel tells the user the site was saved instead of submitting it again.

The complete required check sequence passed with 332 Rust tests and all 10 extension test files, including 26 new scenarios. The composed panel uses scripted Chrome/native/worker boundaries, and the worker test proves immediate measurement and private/non-artifact exclusions. The macOS smoke command explicitly refused to run on this Linux VM, so no browser layout, live-tool certification or Windows acceptance is claimed. Unknown-site shaping, samples, link extraction and host structural drafting remain the next Capture Site slice (`brwr-t40`); the parent bead remains open.

## 2026-10-27 — Inspect private page patterns before learning a site

Brauser now offers a collapsed Page patterns section on public HTTP(S) tabs, including tools it cannot yet capture. Clicking Review page patterns shows the current address as a pattern and inspects up to 500 same-page anchors in Chrome's isolated top frame, returning at most 50 distinct same-origin link patterns. One shared shaper replaces numbers, issue keys, UUIDs, hex IDs, tokens, encoded names and title-like segments with typed placeholders. Query values never survive, fragments disappear, and a route literal occurring in just one distinct link shape is generalized even if that link was planted many times. Short lowercase route words remain readable. Exact page URL/origin checks and a final active-tab check prevent a navigation from publishing a stale review, while closing, collapsing or leaving it discards the link shapes.

While capture is on, configured sites also keep up to 20 visited patterns for seven days in trusted extension storage, including patterns of pages their artifact rules skip. These contain only the shape, a bounded navigation count and the last-seen time; they never enter activity delivery, native messages or the notes folder. Settings can inspect or clear each site's patterns without changing its capture rules or contacting the host. Pause, removal, policy suspension and expiry prune samples; corrupt data is discarded, and a sampling failure cannot interrupt recognized artifact capture. Review found and fixed a privacy-control edge case so clearing still works when local removal policy is unreadable.

This is the complete private input/review slice, brwr-b39, of unknown-site Capture Site. It deliberately offers no enable action for unknown sites: brwr-t40 retains host structural drafting, per-rule checks/selection, shared-origin scope and identity cases. The named bare-Notion success case conflicts with the leading-literal draft rule; brwr-gf1 records that decision for the next drafting work. Twenty-one new mocked/serialized-function scenarios pass alongside all 11 extension test files and 332 Rust tests, with both Clippy configurations and the full required gate green. The extension was built and exercised with mocked Chrome/DOM APIs; real Chrome injection and layout remain unverified on this Linux VM, whose macOS smoke runner refused to run. No dialog or smoke step changed.


2026-10-27 — Complete the sandbox-aware service verification repair (`brwr-elx`). The opening health gate still failed five tests because they connected to Unix sockets without the existing permission-denial guard. An extra scripted-dialogs run found a sixth missing-service test with the same omission. These now follow the established local-skip/CI-fail policy, including connections to absent listeners, without changing service behavior or weakening assertions. All six skip on simulated denial and fail when CI's required-IPC flag is set. The full required gate and scripted-dialogs tests pass locally with explicit IPC skips; this also verifies the already-merged dialog request-reader repair and closes its stale `brwr-4vb` queue entry. Socket behavior must still be exercised by the driver's unrestricted gate and CI.

2026-10-27 — Make Capture Site's attention test deterministic (`brwr-647`). A 15-second test interval seeded from the wall clock intermittently crossed an hour boundary, where the worker correctly moved the completed hour into delivery. The test looked only at live totals and reported missing time. Pinning the start to local 10:59:50 reproduced the failure as 5 seconds live instead of the expected 15. The repaired test checks fixed mid-hour and boundary intervals, counts queued seconds alongside live milliseconds, and confirms that all 15 seconds survive with zero invented opens and the same artifact identity. All 77 worker cases pass in 20 repeated runs and in UTC, Los Angeles and Kathmandu time zones; all 15 extension files pass three complete runs. No activity-accounting implementation or design contract changed.

## 2026-10-27 — Keep note edits when the panel closes

Brauser now keeps unsaved page-note text in this browser profile as you type. Closing the side panel while autosave is waiting, or while an earlier save is still running, no longer relies on that final native request finishing before Chrome tears the panel down. Reopening loads the current saved note and shows the recovery text separately, with Copy and Discard recovery copies. Nothing is pasted into the editor or saved automatically, so an edit from before a revision or artifact-key change cannot silently overwrite the current note. Conflicts and even an attempted empty deletion stay recoverable until explicitly discarded.

This restores the recovery portion of the earlier, unmerged PR #67 (`8ae533c`) rather than inventing another store. Every snapshot has a fresh immutable key. A new copy is published before the preceding one is retired, and a successful native save removes only the exact snapshot it acknowledged; late saves and stale discards from another panel cannot erase newer typing. Browser storage failures produce a visible warning while ordinary native saves continue. If quota prevents writing the latest edit, the previous durable copy and newest in-memory copy-out are kept, but closing still cannot guarantee that unwritten edit. Copies contain note text and page addresses, remain outside the home folder and are not synced by Brauser; clearing extension data or uninstalling removes them.

The built extension passes all 15 test files, with 15 new recovery regressions (29 editor and 60 composed-panel cases in those files). The full required gate, README script/documentation checks and both Clippy configurations pass; Rust reports 610 passes, including 43 explicit sandbox IPC skips, for 567 exercised tests. The macOS headless runner refused this Linux VM, so real browser layout, storage and teardown acceptance remain with the existing browser gate. `brwr-3nq` is complete; Capture Site's remaining Lane A work and the macOS interaction gate remain open.

## 2026-10-27 — Show brief activity accurately in worklogs

A short ticket review or document check no longer reads as zero minutes of work. Brauser now shows positive focus or background time shorter than a minute as `<1 min` in generated worklog prose and artifact Timelines. Zero time is still omitted, and longer periods retain their whole-minute display. The numeric frontmatter remains integer minutes so existing consumers keep the same format. The regression tests reproduced the previous zero-minute output before the fix, then checked both generated worklog text and a real regenerated artifact file, including its unchanged numeric fields. This completes `brwr-d3m`.

## 2026-10-27 — Open Recent while the native host starts

Recent now works when someone opens it immediately after opening the side panel. Its list and worklog show “Waiting for the host…” until the native host has supplied the configuration, and Copy stays disabled while there is no worklog to copy. The visible view then loads automatically, without switching tabs again. Requests started after configuration arrived are not repeated when policy installation finishes, and history loading does not delay the page-note startup. Three composed panel cases cover opening before hello, during policy installation, and after startup, checking the waiting state, rendered results, and exactly one request for each result. This completes `brwr-p9r`. Full checks pass with 569 exercised Rust tests, 43 explicit sandbox IPC skips, and all 15 extension and 2 script test files; actual macOS browser smoke remains outside this Linux VM.


## 2026-10-28 — Keep captured work after rebuilding the index

Brauser now writes a single, canonical UUID spelling into activity and Why log payloads, matching the ID used for their marker hashes. Previously, an uppercase or unhyphenated UUID was accepted and acknowledged, but rebuilding the index silently skipped its activity or context. Single activity, batch activity and Why writes now survive that round trip. Immediate index updates normalize the same ID, so indexing the original request after reading its marker cannot double the recorded work. Batch acknowledgements retain the caller’s spelling so the reply still matches the request. Normal browser-generated lowercase UUIDs keep their existing representation.

Three regression tests cover the three delivery paths with lowercase, uppercase and unhyphenated uppercase IDs. They first reproduced disappearing activity/context, then checked actual marker payloads, rebuilt measurements, live-update deduplication and continued rejection of a different payload ID. Review added a failing batch-reply assertion before the small correlation fix. This completes assigned `brwr-q7e` for new writes. Older noncanonical markers and interrupted v1 intents need a separate recovery change, recorded as `brwr-smu`; existing logs are not rewritten.

## 2026-10-28 — Refuse URLs the rebuilt history cannot retain

A short Unicode address can become much longer when its characters are percent-encoded. Brauser previously checked only the incoming address length, accepted the event, then lost it when the index rejected its oversized stored URL. Capture now checks the normalized spelling after authorization and tracking-parameter removal, rejecting an address above 8192 bytes as `url_too_long` before creating a log or intent. The same check covers page URLs, activity mode URLs and trail source URLs across single and batch writes, along with the shared Why and summary writer.

Two new regression tests reproduced oversized page and nested URLs being persisted before the fix, then verified refusal through all relevant writers. A third accepts an exactly 8192-byte encoded address in each activity URL field and rebuilds the index from the real log with its measurements intact. This completes assigned `brwr-u8n`. The final cycle gate reports 798 Rust outcomes: 755 exercised tests and 43 explicit sandbox IPC skips, including six new Rust test functions across both fixes. All 15 extension and two script test files, schema generation, build, formatting, both Clippy configurations and Markdown checks pass. The macOS headless runner refuses this Linux VM, so no browser smoke pass is claimed.


## 2026-10-28 — Offer useful actions for older notes

Older notes now always offer read-only View, while Use as artifact note appears only when the shared artifact note does not exist. Previously the panel offered migration even when the host could only refuse it. The editor now passes the host's existence flag to the view and updates the actions when a first save creates the shared note, a stale save discovers another writer's note, or a conflict reports that it was deleted. Viewing keeps the current editor text intact and never triggers a save or migration. Five new extension scenarios cover the action sets, viewing, saves and conflicts; the existing recovery suite still passes. This completes assigned brwr-v5t.

## 2026-10-28 — Keep an older note's creation URL during migration

Moving an older URL note into a shared artifact note now keeps the older note's recorded URL. Previously it replaced that address with the route where the user clicked the migration action, which could erase the only durable evidence connecting the note to its original page after capture rules changed. The host takes the URL from the ownership-checked source note, keeps the no-clobber move, and preserves it on later saves from other routes. The regression migrates a board-modal note from an item page, inspects the resulting file, confirms the source was moved and checks another save. This completes assigned brwr-w4k. The full local gate passes: 882 exercised Rust tests plus 43 explicit IPC skips, all 15 extension and two script files; macOS browser acceptance remains separate.


## 2026-10-28 — Capture tests that stay green across midnight

The two Windows CI failures reported as assigned brwr-mdn (also filed as brwr-mdt) came from event fixtures whose timestamps could select different daily logs while a test was running. Capture and native unit tests now share a timestamp at the preceding day's UTC noon, frozen for each test thread. It remains recent enough for the host's real delivery window and leaves room for fixture offsets, while repeated helper calls cannot drift onto a new day. Production clock handling is unchanged. A regression first reproduced the compact batch's calendar-day refusal, then passed the full batch/recovery and oversized-log scenarios at 23:59:59 and just after midnight in UTC, UTC-12 and UTC+14. The full local gate passed with 883 exercised Rust tests and 43 explicitly guarded IPC skips.

## 2026-10-28 — Summary titles follow the current page's heading

Assigned brwr-sti reported a summary of an individual GitHub pull request that still carried the pull-request list's title. The URL and body can update during in-app navigation before Chrome's tab title, document.title, and the metadata Readability uses. The extractor now prefers a single clear H1 in the current page's main or article region, ignoring headings marked hidden and those inside navigation or sidebars. Equal duplicate headings are fine; missing or conflicting headings retain the existing Readability, document and tab title fallbacks. The chosen title still goes through Unicode repair and the existing title cap. Four new extension regressions cover these choices and a list-to-item navigation through the real summarize entry point, including refusal if the caller still holds the old URL. The full local gate passed; actual Chrome/GitHub navigation remains a macOS acceptance check.

## 2026-10-28 — Explain capture without judging unreadable tabs

Brauser's enabled-capture status now uses the same plain wording as settings: capturing on sites with adapters. It no longer refers to a vanished list of sites, even when capture is enabled with an empty adapter document. When the active tab becomes a Chrome page or Chrome withholds its address after site access is withdrawn, the panel clears both the previous artifact card and its verdict. It still says Not an artifact for a readable page the host actually checked and did not recognize. Four panel regressions failed before the fix and now verify both capture configurations and both unreadable-tab transitions. This completes assigned `brwr-09c`.

## 2026-10-28 — Recover the artifact card when the host returns

A note autosave that reconnects to the native host now refreshes the artifact card as well. Previously, the note could say Saved while the same page's card still claimed the host was disconnected and showed old focused time. The panel observes the connection transition in its shared session callback, asks for the current artifact again, and keeps any unsaved Why I'm here text for that artifact. A composed regression first reproduced the stale status, then verified fresh totals, the retained draft, and exactly one lookup; a second ordinary autosave sends none. This completes assigned `brwr-b50` without changing note recovery or save sequencing.

## 2026-10-28 — Keep delivered activity visible after capture controls

Background delivery updates are now noticed by every panel status reader. Dismiss notices, Pause capture and Discard pending could previously read the new delivery counter before the periodic poll, leaving the poll with equal counters and the card with stale totals. The shared render callback now tracks the last observed generation and uses the existing coalescing refresher to reload the current page once when it changes. Repeated polls with unchanged counters send nothing. All three action cases failed before the fix while the normal poll control passed; all four now pass. This completes assigned `brwr-mm0`. The complete local gate passed for this batch: 883 exercised Rust tests with 43 explicit sandbox IPC skips, all 15 extension and two script files, and 10,696 inert Markdown renderings. macOS browser smoke remains separate because its runner refuses Linux.


## 2026-10-28 — Keep adapter repair reachable

Settings can now read a configuration whose adapter rules the host preserved for repair, including format 0 and documents with 65 adapters. Previously the extension rejected the entire reply and hid the repair path behind a protocol error. The incoming check now validates the document's JSON shape, allowing the host's issue and revision through to the existing reset control. It still rejects non-object artifacts, and the adapter engine retains the supported-format and 64-adapter limits before any new capture policy can be enabled. Shared fixtures come from the real host and are checked by both the Rust native path and settings tests (`brwr-5nq`).


## 2026-10-28 — Explain a pending pause during a host outage

A locally paused capture session can still be waiting for the native host to save the setting. The panel now explains when the host is unavailable and asks the user to reopen the panel, instead of directing them to a disabled Pause capture button. While another panel task is running or config needs repair, the hint explains that wait; once the host answers and the control is usable, the normal finishing instruction returns. The longer-lived reload status states that the host setting is pending without retaining a button pointer after the connection changes. An outage/recovery regression checks the composed panel behavior (`brwr-gob`).


## 2026-10-28 — Keep Why drafts private during pending removal

The artifact card's Save why action now respects the worker's pending site-removal list. On the origin being removed, the button is disabled and the handler independently refuses a save, tells the user to finish removal in settings, and retains the typed draft. This closes the upgrade window where the worker had already turned a site off locally but the host still had its adapter and would accept a manual Why entry. A pending removal on another origin does not stop saving context on this page; both paths are covered (`brwr-ufa`).


## 2026-10-28 — Keep activity references tied to the retained title

When queue pressure combines two undispatched activity totals for the same artifact and hour, the resulting refs now come from the event whose title is retained. Previously an older title's issue keys could survive after a newer title replaced it, causing the host to reject the whole combined activity event as unverifiable. If the newer event has no title, the existing title and its refs remain together. Refs are still deduplicated and capped, and existing duration, trail, dispatch and byte-bound behavior stays intact. Four worker cases exercise replacement, repeated refs, missing titles and the first title (`brwr-rft`).


## 2026-10-28 — Follow external changes to a page's note identity

When settings or another panel installs capture rules while a page is open,
Brauser now reloads that page's note after reading the changed configuration
revision. Previously the artifact card could recognize the page while the
note editor still held its old URL key, so the next edit was refused and sent
to copy-out. The editor now gets the host's current note and key before the
next edit. It uses the existing serialized save/load queue: drafts pending
or already saving retain their old expected key, and a key-change refusal
keeps all typed text in copy-out and durable local recovery. No recovered
text is inserted into the new note automatically, and a later successful
save cannot clear the earlier conflict copy. Refreshes that read the same
configuration revision leave normal typing and debounce alone.

Four new composed panel cases cover the clean transition, pending and
in-flight drafts, and the unchanged-revision control. The full local gate
passed, including 159 panel/editor cases, all 15 extension files and 927 Rust
outcomes (884 exercised, 43 explicit IPC sandbox skips). macOS Chrome smoke
remains unverified on this Linux VM. Completes assigned brwr-jdi.


## 2026-10-28 — Check a removed site's identity before offering Save

Capture Site can now tell someone during review when its starter would
conflict with capture rules they used earlier. Removing a site keeps those
rules' identity contracts so old activity and notes stay connected; previously
the panel could promise to enable a starter and only reveal the conflict after
the person clicked Save. The review now asks the native host to check the
proposed rules first. A conflict names the recorded page kind and ID template
and points to settings, while a fresh or compatible site still saves with one
click after the check completes.

The host checks both normal retired history and contracts removed from the
config file by hand. That history stays on the host, and the check changes no
capture policy. Each selection has its own verdict: editing an organization,
unchecking a kind, canceling or reopening cannot reuse an older reply. A host
that lacks the new additive protocol message leaves Save disabled. The actual
save still independently validates its revision and history. Seven new panel
cases, a native removal/restoration regression and a protocol round trip pass
with the full local gate: 134 panel cases and 929 Rust outcomes (886 exercised,
43 explicit IPC sandbox skips). macOS Chrome smoke remains unverified on this
Linux VM. Completes assigned `brwr-t8m`; `brwr-iqx` remains blocked because its
acceptance criteria name a deleted finisher whose replacement already has the
correct notes-folder wording.


## 2026-10-28 — Finish interrupted captures after upgrading URL limits

Brauser can now finish an older interrupted capture even when percent encoding
makes its stored URL exceed the newer 8192-byte limit. Previously, the new
check rejected the retry before reaching the older host's recovery journal.
The worker could then drop the event while an unfinished line remained in the
daily log. The host now checks that journal first, under the existing capture
lock, for both individual deliveries and batches. This covers the page URL
and activity mode and incoming-trail URLs.

Recovery still needs the exact entry hash, an unchanged partial prefix, an
owned log and the same selected folder. A changed retry remains pending for
repair; it cannot write different content. New events still obey the URL
limit, and a rejected event cannot block a valid batch neighbor by choosing
the wrong daily log. Two failing-first regressions cover eight recovery
combinations and both mixed-batch orders. The full local gate passes with
931 Rust outcomes (888 exercised, 43 explicit IPC sandbox skips), all 15
extension files and two script files. macOS smoke remains unverified on this
Linux VM. Completes assigned `brwr-u8r`.


## Deliver capture-loss evidence after the last batch (2026-10-28)

Brauser now finishes recording a queue-overflow notice even when there is no
later page visit to carry it. Previously, hours dropped while the final native
batch was awaiting acknowledgement stayed visible in the panel but could wait
indefinitely for their durable log marker. The worker now follows that final
batch with a standalone delivery containing only the dropped count and time
range. It stores the delivery ID and exact snapshot before sending; lost
replies and worker restarts reuse them, while newer losses remain counted for
a separate marker. Failed deliveries back off and remain visible, and Pause
still prevents dispatch.

The host takes no page URL or path for this notice. It validates the count,
timestamps and chronological range, rechecks capture policy and the selected
home folder, and derives the daily log from the last timestamp. The same owned
log, compact intent, append/sync and duplicate checks protect this write;
existing batches keep their historical recovery rendering. Tests reproduce two
actual dropped hours during the last batch, exercise lost replies and Dismiss
races, and recover a torn log append exactly once after restart. All required
checks pass with 936 Rust outcomes including existing sandbox IPC skips, all
15 extension files and two script files, plus 10,696 inert Markdown renderings.
Real macOS smoke remains unverified here. Completes assigned `brwr-m64`.


## 2026-10-28 — Keep retired adapter history from crowding out settings

Brauser remembers removed adapters so their pages keep the same notes and cannot silently return under a different artifact identity. That history shares config.toml's 64 KiB limit with ordinary settings. Assigned brwr-2iz now gives growing history its own 32 KiB budget, measured as the pretty TOML actually stored. A legacy history already larger than that can stay unchanged or shrink: pausing capture, changing tracking-parameter settings and adding a disjoint site still work while the whole file fits. A refused retirement explains how to pause without removing adapters or restore exact original rules in Settings, rather than returning a generic save failure.

Pruning removes only exact duplicate history entries or contracts restored as active adapters. A distinct older route remains protected even when another adapter uses the same ID. Restoring moves a contract within the file, so it frees history budget rather than total file space; whole-file pressure instead names history's byte cost and asks to reduce optional metadata or summary vocabulary. The usual revision checks and ID freeze still apply, and no note or vault path changes. Four new Rust regressions cover filled histories, native error replies, rejected-write preservation, restoration and identities after restart. The complete local gate passes with 940 Rust outcomes, including 43 explicit sandbox IPC skips, all extension and script files, and the Markdown renderer corpus. Interactive macOS Chrome checks remain unverified on this Linux VM.


## 2026-10-28 — Reopen the recorded source of an artifact

A ticket's artifact card can now show which recorded pull request, document or other artifact brought you there. Under Recorded navigation it lists up to five incoming source/transition pairs, each with the source's title, an explicit link opening in a new tab and the latest recorded day. Wording such as “Link navigation from” and “Form submission navigation from” reports the browser observation without claiming why the person followed it. Repeated observations stay one row, while different transition kinds can name the same source. Days determine ordering; the card does not invent an exact within-hour sequence or click count.

The host derives the list from existing trail edges and known source artifacts. Missing sources, self links, back/forward and unknown historical transitions produce no explanation, and mentions, containment and co-focus cannot become navigation evidence. An empty list says that no known incoming source was recorded. Source titles render only as text, links accept HTTP(S) only and use noopener/noreferrer, and following another page clears the old list. Protocol v8 adds a required incoming_trails field, so rebuild the host and extension together; logs, cache formats and collection behavior stay the same.

Completes assigned brwr-1g2. Four new Rust regressions and two composed-panel cases cover native validation, the list cap and ordering, reindex, unknown legacy transitions, hostile titles, unsafe schemes and activating a local source fixture. All 1,680 extension cases pass individually, and the full required and README gate passes with 944 Rust outcomes (901 exercised and 43 explicit sandbox IPC skips), both script files and 10,696 inert Markdown renderings. The first feature gate found old protocol-version fixtures; those were corrected and the entire gate rerun successfully. The fake panel DOM simulates source-tab activation; actual Chrome anchor opening remains unverified on this Linux VM.


## 2026-10-28 — Recover interrupted log entries across presentation upgrades

Brauser can now finish a partially written single-event log entry after a new
version changes how titles or context are displayed. Previously those journals
kept only a hash of the rendered line, so recovery depended on reconstructing
exactly the older renderer. New version-2 journals retain the host's prepared
append, its hash and a separate identity hash of the normalized event fields.
After a restart, the same event can finish those original bytes through either
the older single-event delivery or the current worker's batch delivery. The
shared path covers visits, activity, explicit Why context and summary events.

Recovery checks the payload and exact torn tail before appending and preserves
unrelated log rows. A different selected folder, changed payload, edited tail,
corrupt journal or exceeded size limit refuses the write. Completed visit
markers still deduplicate; summaries also keep their exact path/key and folder
checks. Prepared entries are capped at 512 KiB and their serialized journals
at 1 MiB, with a maximum-field activity fixture checking those budgets. Older
v1 journals retain the frozen-renderer compatibility, and unknown historical
renderings still need manual repair rather than a guessed append. Four new
regressions pass alongside the full local gate. No live service was touched.

## 2026-10-28 — Read preserved artifact notes separately

When capture rules disappear or several historical artifacts share the same
stored address, Brauser keeps the notes but cannot safely choose one for
editing. The panel now offers **Preserved notes at this address** below the
editor. Expanding it searches the notes folder and lists matching titles
with their artifact IDs, so notes with similar names remain distinguishable.
**View read only** opens one in a separate text-only viewer with Copy and
Close. The current editable note keeps its original key and revision, even
when that address already has its own URL note. Viewing never migrates a
file or restores capture.

The host checks both Brauser ownership and the exact recorded creation
address at the key's actual file path, then repeats those checks when the
person opens a result. It rejects unowned or malformed files and reuses the
existing limited scan, showing at most 24 notes. The operation does not need
index.db or recorded adapter contracts; a broken history cache therefore
cannot hide otherwise readable matching notes or choose an autosave target.
The search can omit files beyond its budget and cannot find a creation
address that today's stripping rules no longer produce. Collapse and
navigation clear the viewer, and late replies cannot bring its text back.

Six native regressions, one wire test and eight panel cases cover ambiguity,
an occupied URL note, missing history, ownership changes, response bounds,
copying, autosave identity and asynchronous navigation. The full required and
README gates pass, including all 144 panel cases. macOS Chrome interaction
remains a separate human check; no live service or external API was touched.

## 2026-10-28 — Isolate scripted host configuration on Windows and Unix

The host lifecycle tests now choose a disposable configuration file explicitly
instead of relying on HOME alone. Windows obtains application directories
through Known Folders, so changing HOME did not prevent those tests from
reading a person's real Brauser settings. The new BRAUSER_TEST_CONFIG_PATH
seam exists only in debug scripted-dialogs builds; ordinary builds ignore it,
and the existing release guard prevents shipping that feature. Empty or
relative overrides fail rather than silently using a real configuration.

Both host-level picker fixtures now set this path, including the test that
kills the host while a picker is open. Configuration side files and the
rebuildable cache stay beside the temporary configuration. Process-level
regressions check the printed path, the exact revision of the file actually
loaded, disposable cache placement and invalid-path refusal. The ordinary
build also proves it ignores the override. No dialog count or macOS smoke
configuration convention changed; Windows execution remains with CI.

## 2026-10-28 — Explain why the current page is outside capture

The panel now gives a readable explanation when automatic capture does not
apply to the current page. It distinguishes a paused configuration from a
pause still pending in this browser, a pending site removal, a page outside
the chosen workspaces or accounts, and a page that matches no rule. It also
points out missing setup, configuration repair and unavailable browser
settings. An existing site's button opens settings; a site without rules
opens the existing Capture Site review and still requires an explicit save.

The explanation uses the same ordered URL evaluator and revision-cached
compiler as capture, with local pauses and removals taking precedence. It
does not inspect links, infer visits from link counts or change capture,
note identity, or the host's history verdict. Navigation, changed scope and
local status refresh the explanation; unreadable and private tabs clear it.
Repeated control refreshes retain unchanged status text to avoid repeating
screen-reader announcements. Fourteen new composed-panel cases cover these
states and their actions, bringing the individually exercised panel suite to
158 passing cases. Real macOS Chrome interaction remains human verification.

## 2026-10-28 — Keep daily-log page titles and Why context as text

A page title or the context someone adds with Why can contain dollar signs,
backticks or Markdown-like text. New daily-log rows now run those fields
through the same inline escaping used by summaries, so a Markdown viewer
shows them as text instead of interpreting math or code. This applies to
older queued visit events and current hourly activity, delivered individually
or in a batch, as well as Why context. The structured event payload retains
its original text for indexing and regeneration.

Changing the written bytes also needed care for an interrupted older write.
Newer journals already store their prepared bytes, but v1 journals only
stored a hash. Brauser now retains two immutable historical renderers: the
original text escaper and the later Unicode-aware sanitizer before math/code
escaping. Recovery completes a historical row only when its whole-entry hash
and exact torn bytes agree with the journal. It never rewrites completed rows.
Two regressions cover fifteen new-write/recovery scenarios, including restart,
a cut inside a multibyte title, refused changed text and edited tails, retained
unrelated rows and duplicate retries. All required local checks pass; the
macOS browser smoke runner cannot run on this Linux VM (`brwr-2za`).


## 2026-10-28 — Review recovered note copies one at a time

Returning to unsaved notes from several pages now presents a separate recovery
card for each copy instead of combining everything into one text box. Each
card shows the page address, why the text was kept, a read-only preview, and
its own Copy and Discard buttons. Copy preserves exactly that note's text,
including whitespace; an empty edit is explicitly described as clearing the
note and copies an empty string. Clipboard errors stay beside the relevant
copy so the person can retry without losing it.

Discard selects the copy's immutable snapshot ID and removes only that copy.
This also works for conflicts kept only in memory when local storage is
unavailable. Other recovered notes, current typing and newer snapshots from
another panel remain intact, and a storage refusal leaves the chosen copy
visible and retryable. Recovery still never fills the editor or sends a save
automatically. Four failing-first built-panel tests and two additional editor
regressions cover the interaction; the existing teardown, conflict and
acknowledgement tests now exercise the individual controls. The smoke check
uses the new card selector, with the same three expected folder-picker
responses; a real Chrome/macOS pass remains separate verification.


## 2026-10-28 — Remember what was removed from a saved page address

Brauser now records which query parameter names it removed when saving an
address, without keeping their values or a second copy of the original URL.
This helps distinguish a clean address from one that became ambiguous after
tracking or identity parameters disappeared. If a later capture rule would
interpret a stripped parameter differently — including treating its absence
as meaningful — Brauser refuses to offer that older note for migration into
another artifact. Its text stays on disk, and explicit preserved-note viewing
remains a read-only way to inspect copies. Ordinary tracking names do not
prevent discovery when no recorded rule reads them; primary autosave identity
continues to come from the existing host contracts.

The evidence travels with daily-log page, mode and trail URLs, the rebuilt
index and its queryable SQLite table, and note and summary front matter.
Notes preserve their creation address and evidence through later edits and
explicit re-keying. Older records remain readable with unknown evidence;
upgrading or saving them never invents proof that nothing was stripped.
New interrupted-write intents bind the names as well as the normalized
content, while the old v1 renderers and v2 hashes still recover only their
exact original bytes. Ten new regression tests cover these paths, including
a later-adapter collision and a review-discovered error-classification bug.
The full required and README gates pass; real macOS browser checks remain
outside this Linux run. Completes assigned `brwr-srp`.


### Recover historical activity and context (2026-10-28)

Brauser can now rebuild older work history that used uppercase or
unhyphenated UUIDs without silently losing measured activity or Why context.
Those older log entries stored the original UUID spelling in their payload
but hashed its canonical form in the marker. The reader now validates and
caches the canonical UUID, keeping retries from double-counting the same
work. It leaves the daily log exactly as it was written, and a payload naming
a different UUID still fails validation.

An interrupted append from that older build can also finish after an upgrade.
For v1 hash-only intents, recovery reconstructs the original queued UUID
spelling alongside all three frozen historical text renderings. It selects
bytes only when the complete intent hash and existing torn tail both match;
changed payloads, mismatched IDs and edited logs stay untouched. Prepared-byte
v2/v3 recovery continues unchanged. Two failing-first regression matrices
exercise 33 cases through reindex, restart, single delivery and activity batch
delivery, including duplicate retries and refusal paths. The full required
and README gates pass; macOS browser acceptance remains outside this Linux
run. Completes assigned `brwr-smu`.


## 2026-10-28 — Rehearse artifact identity corrections before migration

Brauser now has an offline developer command, `brauser adapters aliases-preview`,
that shows what an artifact's identities would become under a proposed capture
policy. Given old and proposed adapter documents and URL evidence, it derives
the endpoint IDs itself and prints the resulting canonical groups and pending
suggestions. A chain of moves stays one group, a move back makes the old key
canonical again, and undoing a pair removes only that connection. Suggested
links never merge records. Exact logical event retries are idempotent, while
conflicting reuse refuses the preview.

This gives developers and agents an inspectable first step toward Re-learn
capture without risking existing notes. The command reads stdin only, creates
no config or vault files, checks adapter scope and types, and refuses URL
provenance that cannot establish a query-based identity. Its output describes
only the supplied operations and cannot authorize a migration. The ID freeze
remains until the remaining impact planner, durable migration and panel review
are complete. Fifteen new tests include real binary isolation checks; the full
required and README gates pass with 987 reported Rust outcomes and all 15
extension files. `brwr-sq9` is complete; parent `brwr-6x2` continues with
`brwr-hq8` under the four-stage plan in `docs/superpowers/plans/`.

## 2026-10-28 — Review capture gaps before sharing a worklog

Brauser now shows known capture gaps where a person prepares a daily or weekly
worklog. If a full browser buffer dropped completed activity-hour records, the
review lists the reported count and its original time range instead of making
the worklog look silently complete. A checked-by-default “Include capture gaps
in copied worklog” choice adds a plain incompleteness note to the preview.
Clearing it leaves the notices visible for review and copies only the measured
work. Copy always uses exactly the preview, and the choice remains in place
when the period changes or new activity refreshes the view.

The host derives notices from existing owned daily-log markers alongside the
measured report. A range that intersects the chosen local dates is shown in
full: the log does not say which missing records belong to which day, so the
count is never prorated or turned into focus time. Retry copies appear once;
different notices with overlapping ranges stay separate without a combined
count. Notices work even when no artifact was recorded. Reindex restores the
same evidence, and an older cache makes one bounded rescan to find overflow it
previously skipped. Results are capped at 32 notices with explicit truncation.
Loading, failed or stale replies cannot leave an earlier report available to Copy.

Assigned `brwr-9r7` is complete. Real local capture/reindex fixtures cover both
standalone and attached notices, including long anchor URLs, and six new panel
cases use mocked transport and a private clipboard stub. The full required and
README gates pass: 990 workspace Rust outcomes, all 15 extension test files,
both Clippy builds, the scripted-dialogs suite, script checks and 10,696 inert
Markdown renderings. IPC sandbox skips remain explicit. Actual macOS Chrome
and current Windows/macOS CI acceptance remain with the existing platform gates;
no real account, model, live service or user clipboard was used.


## 2026-10-28 — Restore a removed site's recorded capture rules

Capture Site now recognizes when Brauser has used rules on a site before,
including custom rules removed through settings or deleted from config.toml.
An explicit click offers the recorded page kinds, readable route patterns and
exact account scopes for review, instead of proposing a stock starter with a
different artifact identity. The review sends only the origin and config
revision to the host. It does not need page content or link inspection, and
nothing starts capturing until the user saves. Restoring the only site also
respects a global pause rather than silently resuming capture.

The host checks that the recorded contracts are usable and unambiguous; the
panel re-reads the same set before the normal revision-checked save, and
refuses changed records, stale settings, navigation and pending local pauses
or removals. Existing note edits use the established save/recovery boundary.
Real local native tests exercise custom Jira browse and issue-modal routes
through both removal paths and verify unchanged note keys and exact file
bytes. Recorded title cleanup, title aliases and reference extraction are not
reconstructed; settings can add those again. Damaged records retain repair
guidance. Overlapping historical rules, including repository/PR containers
that can name several note identities, remain refused; follow-up brwr-7lq
covers an explicit identity review through the existing migration plan.

Validation adds seven Rust regressions and eleven composed-panel cases. The
required Linux gate passes 997 Rust outcomes (43 documented IPC sandbox
skips), all 15 extension files, formatting, builds, typecheck, lint and both
Clippy configurations; the scripted-dialog suite has 859 passing outcomes.
The Windows skip-reason audit is complete, while brwr-bzx and brwr-9xg remain
blocked for current Windows/macOS CI and actual macOS Chrome evidence. This
cycle makes no platform acceptance claim and uses no real account or model.

The optional 5,000-case engine comparison is not counted as passed: its
one-case probe timed out, and Node child pipes are denied by this sandbox.
The run was stopped and recorded as brwr-wg5. Both adapter engines are
unchanged; their ordinary vector tests passed in the required gate.

## 2026-10-28 — Review the impact of re-learning capture rules

Brauser can now show what a replacement capture policy would do to history
that already exists. `brauser adapters relearn-plan` inspects the selected
home folder's owned log markers, notes and summaries and returns a JSON
review with unchanged and changed identities, splits, merges and unresolved
cases. It also includes a repository whose history comes entirely from work
inside it. This completes brwr-hq8, the second stage of capture re-learning;
the panel action and actual migrations remain under the parent brwr-6x2.

The report keeps existing notes and summaries on both sides visible without
choosing one or overwriting either. It flags simultaneous swaps and reused
keys that sequential aliases would incorrectly collapse, and refuses to
infer identity when a query parameter may have been stripped. A URL note
sharing an artifact's address remains a separate copy requiring an explicit
choice. Config, recorded contracts, inspected evidence and proposal hashes
bind the review so a later migration can reject stale decisions. The command
writes only coordination locks; it leaves the cache, content and ID freeze
unchanged. An optional absolute config path supports disposable fixtures.

Twenty-one new tests cover this boundary, including actual capture markers,
both-side files, provenance and the real CLI's rejection paths. A successful
CLI fixture also preserved exact config/note bytes. The full required and
README checks passed with 1,018 workspace Rust outcomes (43 explicit sandbox
IPC skips), all 15 extension files, both Clippy builds, 880 scripted-dialog
outcomes (53 IPC skips) and 10,696 inert Markdown renderings. Windows/macOS
execution remains separate. The next concrete stage, brwr-wvi, records
reviewed decisions in a durable journal with recovery before file/index or
panel integration can enable migrations.

## 2026-10-28 — Keep capture-correction reviews across interrupted writes

Brauser can now keep a durable record of a reviewed capture correction before
any migration is enabled. The developer command `adapters relearn-record`
rebuilds the impact plan under the configuration and capture locks, checks that
all reviewed inputs still match, and records an approval or rejection with its
full proposal, mappings, file revisions and selected home-folder identity.
`adapters relearn-journal` shows those records and completes interrupted
appends. This finishes brwr-wvi, the journal prerequisite of capture re-learning.

Each operation has a stable UUID. An exact retry returns the original receipt;
a different proposal or decision cannot reuse that ID. The journal saves the
exact bytes it intends to append before writing them, so recovery can finish a
torn write without guessing or flattening a simultaneous mapping into alias
links. Splits, merges and reused keys cannot be approved in this stage; a
rejection preserves their full impact report. Reviews remain inspectable even
if the former home folder is unavailable. All receipts say `applied: false`:
notes, summaries, activity, config and index identities are still untouched,
and ordinary settings saves still enforce the ID freeze.

Eighteen new regressions cover crash checkpoints, a torn UTF-8 append,
concurrent writers, exact retries, stale log/note/config/contract evidence,
conflicting identities, damaged state and separate real CLI processes. The
complete Linux gate passed with 1,037 workspace Rust outcomes (43 explicit
sandbox IPC skips), 899 scripted-build outcomes (53 IPC skips), all 15 extension
files, both Clippy builds, two script files and 10,696 inert Markdown renderings.
Fresh diff review also fixed parent-directory syncing on retries. Windows
power-loss durability and real browser acceptance still need platform evidence.
Next, brwr-dbp integrates actual config/file/index migration and authoritative
rebuild; panel Re-learn and parent brwr-6x2 acceptance follow that work.

## 2026-11-01 — Repair the exported work queue before feature work

This cycle repaired a failing queue check discovered during the opening
health gate. The committed queue named two Re-learn prerequisites that were
absent from the export, while the local database still held their stale rows
from an unmerged attempt. One row claimed a completed journal whose source
is absent from this checkout. After retaining a local backup, Cookie removed
only those database-only rows and dangling links, exported the queue, and
regenerated the backlog from its authoritative data. All 173 tracked issues
and their statuses and labels remain intact; human review gates stay in
place. The hydration follow-up now records the evidence. This is queue
maintenance; assigned product fixes remain open for the next healthy cycle.

Validation after the repair passed the complete required and README gates:
1,207 reported Rust passes, all 37 extension test files, both Clippy builds,
74 Python script tests, six Node script test files, and 10,696 inert Markdown
renderings. Current-state and queue checks pass. Full diff reviewed locally;
independent review and publication remain with the driver. Logs are in
`target/overnight-health/2026-11-01-queue-repair/`.

## 2026-11-01 — Verify summary restoration after AI-draft bursts

The Capture Site restore follow-up is now covered across the service and the built panel. The existing per-kind list implementation filters before its 20-run limit; the new regression matrix verifies that more than a window of newer AI drafts cannot hide retained summaries or tasks. A built panel test also reopens with an older finished summary behind 21 newer site drafts and checks that the summary card actually returns. This closes a stale follow-up left after task history supplied the implementation, rather than adding a second listing mechanism.

## 2026-11-01 — Keep summary restoration compatible across list versions

Summary restoration continues to ask for its own run kind, keeping recent tasks from displacing summaries. The obsolete test that documented that displacement now checks both directions: an older task survives a summary burst, and an older summary survives a task burst. If an older host explicitly rejects the new filter field, the panel retries the original unfiltered request once and displays only summaries from that answer. That fallback retains the older host's shared window until it is rebuilt; the Tasks page still needs filtered, paged listing for complete history and retains its existing rebuild instruction. Other errors are reported directly rather than retried.

## 2026-11-01 — Cancel AI drafts when their review becomes stale

Changing settings elsewhere can make an open Capture Site review unusable. Brauser now removes and cancels its queued or running AI draft as soon as it observes that change, freeing the service slot instead of waiting for a result it cannot use. The review retains its settings-changed message and disabled save button. If the service has not yet answered the start request, the panel waits for its run identifier, then cancels without mounting progress, polling, or judging the result, including a rejoined run while that stale review remains open. Repeated updates and closing do not send duplicate cancels; a failed cancel keeps the existing service-timeout bound.

Validation for this batch passed the required and README gates: 1,207 reported Rust passes (92 use the documented sandbox IPC guard), all 37 extension test files, both Clippy builds, 74 Python script tests, six Node script files, and 10,696 inert Markdown renderings. Five extension test cases were added. The full diff was reviewed locally; independent review and publication remain with the driver. No live API, service, or real harness was used.


## 2026-11-01 — Keep PR hygiene tests running after handoff edits

Editing a PR's owner or handoff text used to cancel its running hygiene workflow, then skip the Dev tools job in the replacement workflow. The replacement now runs the Python checks on body edits too. This keeps a quick ownership update from leaving the current revision without those checks, while PR owner validation continues to read the updated body. The fix uses the existing workflow concurrency and accepts the cost of another short test run. Hosted event cancellation was reviewed from the workflow; no GitHub API was called from this workspace.

## 2026-11-01 — Finish cleanup when its starting worktree is removed

The worktree cleanup command now selects the repository's main worktree for its control commands before removing anything. A caller can point --repo at a finished linked worktree and still remove the remaining candidates and check stale locks after that directory disappears. Existing checks for local changes, ignored state, running processes, live-install references and locks still decide what may be removed; branches remain in place. A new throwaway-repository regression reproduced the old failure, then verified two worktree removals followed by clearing a stale common index lock. All 75 Python dev-tool tests pass, including that regression.

Validation for this batch passed the complete required and README gates: 1,207 reported Rust passes, all 37 extension test files, both Clippy builds, 75 Python script tests (+1), six Node script files and 10,696 inert Markdown renderings. Queue and current-state checks pass. The full diff was reviewed locally; independent review and publication remain with the driver. Logs: `target/overnight-health/2026-11-01-dev-tools/full-gate.log`. No live API, service or real harness was used.


## 2026-11-01 — Keep hidden text out of boxless-body extraction

A page can show its children while its body has no layout box, as with
`display: contents`. Brauser's previous fallback walked that body's nodes
and could include hidden panels, sometimes enough to choose
the wrong frame for a summary or task. The fallback now reads `innerText`
from the rendered document root, letting the browser apply visibility
filtering. A hidden body contributes no fallback text, and a non-rendered
frame still contributes neither text nor selection. Local extraction and
summary-entry regressions cover these cases; actual CSS rendering remains
browser acceptance work.

## 2026-11-01 — Pin task tracking behavior with deterministic unit tests

The panel's task count and completion announcements now have direct unit
coverage in a dedicated test file included in the extension suite. Seven
tests use deferred host replies and mocked timers to verify that concurrent
refresh requests merge into one follow-up, repeated refreshes leave one
poll timer, and tracked tasks announce completion once. They also check
off-page completion fetches, visibility changes and disposal during a
request. This adds a focused test seam for the remaining assigned tracker
fixes without changing its runtime behavior.

## 2026-11-01 — Remove the keyboard-focus test's polling race

The Tasks page focus regression now triggers the page's existing hide/show
refresh handler explicitly. It waits for a changed status pill before
checking that an existing row retains focus, then adds a row and waits for
the rebuilt list before checking that focus returns to the selected task.
Both checks exercise the real refresh and rendering code without depending
on when the two-second poll fires. Ten isolated repeat runs passed after
the extension build completed; this is local evidence, with hosted macOS
CI left to the driver's publication gate.

Validation for this batch passed the complete required and README gates:
1,207 reported Rust passes, all 38 extension test files, both Clippy builds,
76 Python script tests, six Node script files and 10,696 inert Markdown
renderings. Nine extension cases were added overall. Queue and
current-state checks pass. The full diff was reviewed locally; the driver
owns independent review and publication. Logs: `target/cookie-health/`.
No live API, deployed service or real harness was used.

## 2026-11-01 — Reserve tracked issue IDs before creating beads

Brauser now has a repository command, `scripts/dev/create-bead`, that loads
the committed issue history into the local beads database before creating
a new issue. This prevents a stale database from treating an already-shipped
issue ID as available. The helper first validates the JSONL, then uses the
existing importer's timestamp guard and proceeds only after import succeeds.
It retains local-only work and newer local scalar fields rather than
rebuilding the database; labels, comments and dependencies may still merge
under the importer's rules.

The helper forwards issue text as literal process arguments and sends import
progress to stderr so `--json` creation output remains usable. It refuses
linked worktrees, redirected databases and routing overrides; worktrees keep
the documented scratch-database workflow and copy back only new rows. Eight
mocked-command regressions cover ordering and refusal behavior, and a ninth
regression proves the existing queue checker rejects a closed/open ID
collision even when the backlog itself is current. Contributor instructions
and the beads quick start now point to this workflow.

The final review caught a missing `--repo` routing refusal. Both argument
forms were reproduced in the regression before the fix; the helper now
rejects them before hydration as well. Importer storage semantics remain
covered by beads itself; this repository tests the command boundary.

Validation after the review fix passed every required and README check:
1,207 reported Rust passes, all 38 extension test files, 85 Python tests
(+9), six Node script files, both Clippy configurations and 10,696 inert
Markdown renderings. Logs are in `target/overnight-checks/review-fix-*.log`.


## 2026-11-01 — Finish adapter comparisons promptly in restricted sandboxes

Assigned follow-up `brwr-wg5` traced the optional adapter differential check's
stall to the Rust engine's synchronous child-process call with piped input
and output. The case generator completed normally; a bounded probe killed
that call after three seconds, while the existing Node child-pipe probe
reported EPERM. The comparison command now runs that shared probe before
starting either engine. A confirmed sandbox permission denial prints an
explicit skip with “no cases compared”; required CI mode fails instead, and
other probe failures remain errors. This makes an unattended check finish
with an honest result instead of waiting indefinitely.

The probe itself now has a three-second timeout, and each engine batch has
a 60-second timeout with a forceful kill. Failed batches identify the
command, seed and case range. Result-count checks also refuse incomplete
engine output. The engines, generator, comparisons and usefulness checks
retain their existing behavior. Fifteen new regression cases cover the
runner and probe, including required-mode denial, both engines' failures,
multiple batches, mismatches, escaping and incomplete output. A supplemental
regular-file transport run compared 5,000 real Rust and TypeScript cases
(seed 7) without differences; the normal piped CLI correctly skips in this
sandbox and remains a required CI check where child pipes are permitted.

Final required and README gates passed: 1,207 reported Rust passes (sandbox
IPC guards apply), all 38 extension test files, both Clippy configurations,
85 Python tests, seven Node script files and 10,696 inert Markdown renderings.
Logs are in `target/cookie-wg5-final/`; independent review and publication
remain with the driver.


### 2026-11-01 — Announce each watched task completion once (brwr-jlz)

The panel remembers which task run IDs it has announced. A delayed start reply
or stale running view can no longer make the same finished task announce again.
Different tasks still receive separate announcements even when their instructions
are identical; the memory lasts for the current panel instance.

### 2026-11-01 — Preserve refreshes queued during a failed task list (brwr-2p0)

A request to refresh while another list is pending now receives its promised
follow-up even if the pending list fails. Overlapping requests still merge into
one follow-up, and all callers receive the final attempt's outcome. Tests hold
and reject host replies to verify both recovery and a second failure, plus
disposal while a refresh is pending.

### 2026-11-01 — Pause task tracking after repeated host failures (brwr-7u4)

The panel keeps checking active tasks every two seconds while the host works.
Consecutive failed passes now retry after two, four, eight and sixteen seconds
and pause after the fifth failure, including failures fetching tasks absent
from the first history page. Showing the panel again or explicitly refreshing
restarts the budget, and a successful pass restores the normal interval.
Mocked timers verify the delay boundaries, pause, and recovery without waiting
for those delays in real time.

### 2026-11-01 — Keep task-list failures visible across summary restores (brwr-4r1)

A summary started through the shortcut or context menu can restore its card
without making an outstanding task-list error disappear. Error restoration now
lives in the shared summary-card restore function, so startup, live worker
notifications and stored notifications use the same rule. Tests cover live
delivery and a stored notice consumed when the panel follows its tab.

### 2026-11-01 — Remove the tracker's unused clock dependency (brwr-ug2)

TaskTracker no longer asks callers for a clock it never reads. Its only timing
mechanism is the polling timer; the panel and direct tests now construct it
without an unused timestamp callback. Type checking verifies the updated API.


### 2026-11-01 — Keep supervisor tests independent of disk flush speed (brwr-pnti)

The opening health check failed six supervisor tests. Focused runs reproduced
the task-capacity and long-report failures with their runs still progressing.
A local probe measured 5.6 seconds for a single file flush, enough for a few durable
status writes to exhaust a ten-second state wait. Supervisor and retention
unit fixtures now explicitly choose temporary record stores that omit those
flush waits while retaining actual filesystem writes, atomic replacement,
private permissions and write failures. Their existing timeouts and state
assertions remain intact. Normal stores, the record storage tests, and
all production and scripted-dialogs builds retain file and directory syncing;
the temporary-store switch exists only in unit-test builds. An added storage
regression protects the temporary path's permissions, refusal to overwrite
an existing result directory, cleanup and preservation of existing content.

The full run also exposed a separate race in a resumed-run test that combined
a one-second timeout with capturing the prompt through a shell. Those checks
are now independent: the recorded limit still has to time out after one
second, while the existing in-process recording runner checks the complete
prompt built from the recorded vocabulary. This preserves both expectations
without making prompt verification depend on subprocess startup speed.

### 2026-11-01 — Keep Tasks navigation current during refreshes

A newly started task could be missed when its Tasks-page hash arrived while
an older list was still loading. The page now remembers overlapping refresh
requests and runs one follow-up after the current read, even if that read
fails. Selection follows the latest URL hash, so a delayed callback cannot
undo a newer navigation. Closing the tab stops queued work and ignores
pending replies. Deferred-response tests cover success, failure, repeated
refresh requests and tab closure (`brwr-um4`).

### 2026-11-01 — Test Tasks polling without wall-clock waits

The Tasks-page tests now advance a mocked polling clock and drain host
replies separately. Hidden tabs and completed histories are checked across
multiple polling intervals without sleeping for 2.3 seconds. New held-reply
cases verify that overlapping requests produce one follow-up and keep the
latest selection, making the actual race reproducible (`brwr-jzs`).

### 2026-11-01 — Preserve dismissals from different Brauser views

Dismissing a summary in the panel and a task in the Tasks tab at the same
time now serializes the shared storage update with a Chrome Web Lock.
Each document retains its own queue as a fallback, and a failed lock or
write no longer prevents later attempts. The existing 1,000-item cap and
deduplication stay in place. Tests load independent copies of the module
against shared storage and locks to exercise the original lost-update race
(`brwr-90b`).

### 2026-11-01 — Check unknown run kinds against the schema

The protocol regression for an unsupported list_runs kind now checks the
JSON schema directly, alongside the existing Rust deserializer rejection.
This ensures a future schema change cannot silently accept a kind that the
wire contract excludes (`brwr-3wh`).

### 2026-11-01 — Verify Reuse's fallback across windows

The Reuse fallback test now begins in a known, nondefault Tasks window and
asserts that Brauser attempts to open that window's panel. When that open
fails, the saved instruction is untargeted and a panel in a different
window actually consumes it and fills its task bar. This catches the older
behavior that simply handed off without trying to open the panel (`brwr-3pv`).

### 2026-11-01 — Refresh off-page task states concurrently

A task that finishes outside the newest history page still needs get_run
to update its row. Those lookups now start together, so one slow read does
not prevent the others from starting. Results merge in candidate order;
a failed read leaves its row for retry while a successful finish or prune
still takes effect. Tests hold one reply, fail another, prune a third and
verify both recovery and teardown (`brwr-sm8`).

### 2026-11-01 — Align history cursor timestamp lengths

History cursors now use the same 20–64 character timestamp bounds as run
views, replacing the one-character minimum. Wire tests cover both ends of
the range and assert that the two definitions stay aligned. Regenerating
the protocol leaves its Rust and TypeScript types unchanged because this
change affects schema validation rather than field types; length bounds
alone do not validate RFC 3339 syntax (`brwr-0at`).

### 2026-11-02 — Gate keyboard access and accessibility (browser acceptance pending)

Brauser now has a dedicated real-browser accessibility suite for its built
panel, settings and Tasks pages. It checks setup, configured, disconnected
and error states in both light and dark themes, reports incomplete axe
checks for human review, and deliberately inserts an unnamed button to
prove that the gate rejects a serious violation. The keyboard flows use
real Tab and key events to open reviews, save capture rules and adapters,
switch views, start tasks and summaries, and operate task history. Every
browser resource is fulfilled locally and the Chrome/native boundaries are
mocked, so these checks never run a real agent or touch user folders.

The accompanying UI changes give This page/Recent proper tab semantics and
arrow navigation, let Escape close idle reviews with focus returned to the
entry control, preserve focus through run polls and task dismissal, and
adjust muted/status/focus colors for light and dark mode. The npm command
is required by macOS CI and the external commit gate. This is implementation
progress on `brwr-e7b0`, not completed accessibility acceptance: this sandbox
cannot start Chromium, so its 33 browser cases explicitly skip and the
required-mode run fails rather than pretending they passed. The bead stays
in progress for external browser validation, with manual checks still needed
for incomplete axe results and screen-reader behavior.

### 2026-11-02 — Recover the accessibility gate's dark-theme warnings

The driver's real-browser run caught a serious contrast failure in the
Tasks page's disconnected and error messages, so the previous cycle was
quarantined. This repair recovers that implementation and explicitly paints
the shared page background and text with the browser's Canvas/CanvasText
colors. This gives the theme-aware warning text an explicit background;
axe otherwise blends translucent warning surfaces over white when the
document background is implicit, even in dark mode.

Two added regressions switch those warning states from light to dark and
back without reloading. They require opaque computed backgrounds and a
measured contrast pass for the warning itself, rather than accepting an
incomplete check. Failure reports now include axe's node explanations to
help diagnose any further gate failures. The suite has 37 cases: two
transport/assertion checks run locally, while 35 browser cases remain
explicit sandbox skips. The strict required-browser command still fails
inside this sandbox, and `brwr-e7b0` stays open for the external driver to
validate the repair. This cycle adds no unrelated feature work.

### 2026-11-02 — Repair Tasks listbox semantics after gate rejection

The driver's next browser run found a critical listbox defect in the
recovered accessibility work: the day labels were headings inside a widget
that must own groups and options. The visible labels now remain the
groups' accessible names through `aria-labelledby`, with presentation roles
that remove the extra heading semantics. When no tasks match a search, a
message beside the hidden listbox replaces the empty widget while keeping
the selected task's detail visible. Review caught that presentation alone
would still leave a listbox without required options, so the empty state
uses ordinary prose outside the composite. Task grouping, selection and
keyboard navigation stay available.

Two controller regressions fail on the recovered build and pass
with the repair. A new browser case verifies Running, Today and Yesterday
as named groups, requires a measured listbox child-role pass, and audits
searching to no matches and clearing the filter. The suite now has 38 cases:
two local fixture checks and 36 browser cases. Browser execution remains
blocked inside this sandbox, so the required external gate must validate
the repair before `brwr-e7b0` closes. This red-start cycle recovers the
quarantined work and repairs it without starting unrelated product work.


### 2026-11-02 — Complete the accessibility recovery handoff

The cycle-4 rejection notice describes an adapter preview that keyboard users
could not scroll. The current checkout already includes the repair from
PR #212: the preview has a keyboard tab stop and an accessible label. That
PR also waits for the asynchronous diagnostics copy in the keyboard test
and preserves scroll position when restoring run-action focus. The
coordinator recorded three passing required 38-case runs on macOS Chrome
and closed `brwr-e7b0`; that supersedes the pending-acceptance entries above.

This repair-only cycle reconciles the testing guide, history and generated
Shipped entry with that recorded acceptance, preserving all newer code
instead of reapplying an obsolete quarantine patch. All eleven non-browser
health commands pass locally. Required browser mode still fails at the
explicit sandbox guard, so this run adds no new browser acceptance evidence.
Incomplete axe results and screen-reader behavior still require human review.

### 2026-11-02 — Refuse incomplete Re-learn history

Re-learning review now stops when an owned daily log contains a recognizable
but damaged visit marker. Previously a truncated marker could silently vanish
from the impact report and let the host record approval of an empty plan.
The strict scan reuses the existing event and overflow parsers and reports the
file and line that need inspection. Legacy visits, activity, saved context,
summary markers and both overflow forms remain readable; normal indexing
continues to tolerate incomplete entries for capture recovery. Regressions
exercise malformed payloads, provenance and joined markers, plus the real
`relearn-plan` and `relearn-record` commands with a torn why marker. Neither
command changes the config, log, cache or decision journal on refusal.

### 2026-11-02 — Include other sites when checking Re-learn key collisions

A site's proposed keys are now checked against recorded identities from every
origin, including owned notes, summaries and containers that have never been
opened directly. HTTP and HTTPS pages can share a hostname and therefore an
artifact ID; previously reviewing just one scheme could miss a merge with the
other scheme's history. A foreign-occupied destination now reports a merge and
blocks approval. A source already shared with another origin is also blocked,
even when its new key would otherwise be free. Other sites remain outside the
migration rows, and unrelated foreign history still permits a clean approval.
All recorded contracts used to derive occupancy bind the review token. These
two assigned prerequisites add seven regression tests; the full local gate
passes with 1,314 Rust-reported outcomes (92 explicit IPC skips), 40 extension
test files and both Clippy builds. Browser execution still skips 36 cases in
this sandbox. Durable apply (`brwr-dbp`) and panel Re-learn remain unfinished.


### 2026-11-02 — Keep recorded incoming navigation with a summary

Summaries can now retain the recorded path into an artifact, so a reader
returning later can see the planning ticket, document or other known artifact
from which it was opened. The service looks up the existing recorded trail
for the page's current host-derived identity and freezes up to five source and
transition pairs when the summary is requested. The saved file's Incoming
list gives each source's title and safe link, the browser transition and the
last recorded date. It describes observed navigation; the agent's outgoing
References still mean sources the artifact itself cites.

The same bounded snapshot goes into a host-written section of page.md and
stays with the private run record through queueing, restart and Retry save.
The agent may use it as attributed assessment context, but may not open the
source links or treat navigation as a dependency or endorsement. The host
renders the final list itself, so it cannot be invented by the answer. Unsafe
addresses and unattributed transitions are omitted, titles stay inert, and a
run with no known incoming sources adds no section or prompt paragraph. Older
runs keep their original behavior. A service rollback cannot read new run
records containing the optional snapshot; finish queued/running runs first.

Ten new offline tests cover actual recorded trails, cache loss and lookup
failure, bounds and hostile text, Office/Miro boundaries, legacy records,
queue/restart persistence and byte-identical Retry save. The full required
and README gate passes with 1,331 reported Rust outcomes (92 explicit IPC
guard skips), 40 extension test files, both Clippy builds and 10,696 inert
Markdown renderings. Accessibility passes two fixtures and explicitly skips
36 browser cases inside the sandbox. This completes brwr-sm1; no live
harness, external API or deployed runtime was used.

## 2026-11-02 — Read the summary before reopening it (brwr-sm3)

Saved summaries now tell you what a page is about directly in the panel and
Recent: their one-sentence overview appears alongside the document type. A
finished summary run leads its result with that same overview while keeping
its type, category and importance details. Previously the overview reached
only the summary file, so deciding what to reopen required opening that file.
The host now carries the validated sentence through the daily-log event and
run result to each view. Older summaries retain their familiar wording, and
new and old overviews survive reopening the cache and rebuilding it from
logs. Agent text stays literal, including markup-like strings. This additive
protocol-v8 change needs the host and extension deployed together; older
hosts cannot replay newly written overview fields.

## 2026-11-02 — Keep dismissed summaries reachable (brwr-tsh1)

Removing the harness no longer makes retained summary history disappear
after a panel restore. Brauser keeps its browser-local history hint when a
retained summary exists, even if its card was dismissed or aged out of the
panel's one-day restore window. Tasks still lists those records and its Show
dismissed control reveals them. An end-to-end controller regression follows
a dismissed summary through panel restore, harness removal and Tasks, and
additional cases cover older summaries and mixed legacy history.


## 2026-11-02 — Recognize padded IDs in tab titles (brwr-tp1)

Custom capture rules can now remove an identifier from a tab title even when
it ends with base64 padding, such as `[uXjVM0uuBLo=] Roadmap`. The same ID
already worked in URLs; the title matcher stopped before its `=` and left the
whole title in place. Both the native host and browser now recognize the
padded title in either direction, and tab-title alias suggestions retain the
complete identifier. They still reject empty bodies, internal or excessive
padding, and values whose padding exceeds the configured length limit.

The fix reuses the existing longest-run matcher, with strict validation after
scanning, following a separate parser design challenge. A custom template
that uses `=` as the delimiter beside a token capture now fails validation;
use a delimiter outside the token alphabet, such as brackets or spaces.
Shipped starter rules do not use token title captures. The 26 shared regression
vectors reproduced ten failures in each engine before the fix and now pass.
The full local gate passed with 1,337 Rust-reported test outcomes, all 40
extension files and both Clippy builds; 1,265 individual adapter JavaScript
tests pass. A regular-file comparison also verified agreement on 2,024 cases.
IPC and real Chromium checks explicitly skip in this sandbox; the ordinary
differential runner skipped its blocked child pipes and fuzz compilation
could not find the uncached `libfuzzer-sys` dependency. Those checks remain for
the external gate/CI. No live service, harness or external API was used.


## 2026-11-02 — Keep automatic summaries within current page rules

Brauser now checks the site's current capture rules when an automatic summary
reaches the native host. If the site was removed or its rules narrowed while
the browser was reading the page, the host skips the automatic run. Earlier
rules still preserve the shared page note, and a person can explicitly choose
Summarize & log as before. The eligibility settings, note key and current-rule
check share one refreshed configuration lock; it is released before talking
to the service. An offline regression saves a real note, removes or narrows
the rules, and verifies both the skip and the preserved note.

## 2026-11-02 — Retry viewer-only summaries across local days

A browser that stays open overnight can now revisit a summary made from only
the document viewer. The worker remembers already-summarized artifacts for the
current local day rather than the whole browser session. On the next day's
activity synchronization, an open tab can become eligible again; the host
still refuses a new automatic summary when its existing whole or partial
summary is sufficient. Same-day duplicate tabs remain suppressed. Older
undated browser caches are cleared once, while dwell progress and retry state
stay intact. Pure clock-controlled tests cover midnight and a backward date
change, and a worker test verifies a second host request without restarting
the browser.

A delayed already-summarized reply that crosses midnight does not mark the
new day: the worker retains the local day of the actual request timestamp,
and lets the next activity update ask the host for a current-day decision.
A failing-first worker regression covers the delayed reply.

Final validation for both changes: 1,365 Rust-reported outcomes (94 explicit
IPC sandbox skips), all 41 extension test files, both Clippy configurations,
14 pure-state cases in UTC and Pacific time, and 102 worker cases. Five new
regressions and one enhanced case cover the changed behavior. The browser
accessibility command passes its two fixtures and explicitly skips 36 real
browser checks in this sandbox; external driver/CI evidence is still needed.


## 2026-11-02 — Tell agents when browser capture is only an excerpt

Brauser now preserves the browser extractor's signal that page text or a
highlight hit its size limit. Previously that information disappeared before
the agent saw the page, allowing an excerpt to look like the complete artifact.
Manual summaries, automatic summaries and tasks now carry the same optional
flag. The service adds a short completeness notice to the page file and tells
the summary agent to retrieve the full artifact or report a partial/viewer-only
read. Task agents get an incomplete-context warning within the user's request.

The notice also survives a queued-run restart. Its authority comes from stored
run metadata, so a web page cannot activate it just by copying its words.
Frame tests cover the chosen document iframe, discarded page chrome and borrowed
highlights; Office, Miro and Incoming navigation keep their bounded sections.
The existing combined flag is conservative about which text was cut, and it
does not claim that a capture with no reported cut contains the full artifact.
This restores the earlier isolated work and covers the automatic path it
missed. Host and extension must deploy together; downgrade only while no run
is queued/running, because older services cannot read the new true status flag.

Validation: 1,374 Rust-reported outcomes (94 explicit IPC sandbox skips), all 41 extension test files, both Clippy configurations, 2 accessibility fixture passes with 36 explicit browser sandbox skips. Twelve new regressions cover this change. The supplementary README checks and fresh full-diff review passed; real browser, IPC and platform acceptance remain external.

## 2026-11-02 — Reach the task bar from Chrome

Brauser's task bar now has its own Chrome shortcut, Ctrl+Shift+8 (Command+Shift+8
on macOS), and an **Ask Brauser…** entry in the page and selection context menu.
The shortcut can be changed in Chrome's extension shortcut settings. Inside the
panel, pressing `/` outside a text field focuses the task bar. Existing drafts
stay in place; these gestures never copy the page or selection into an instruction
and never start an agent. Enter or Run remains the deliberate start action.

The worker opens the panel while Chrome still recognizes the user gesture, then
leaves a short-lived focus notice so a panel still loading can honor it. The
panel rejects stale, future and wrong-tab notices and waits if a task start has
made the field read-only. Text fields, editable page controls and input-method
composition keep their normal keys. Worker and panel tests cover the transport
and startup paths; actual Chrome shortcut binding and native menu behavior still
need an interactive platform check.

## 2026-11-02 — Recall complete task instructions without losing a draft

ArrowUp at the start of the task field now brings back earlier task instructions.
Repeated presses walk older instructions; ArrowDown at the end walks forward and
returns to the original unsent draft. Edits to a recalled instruction survive the
round trip too. Recall reads the complete user instruction from the service's
stored run, rather than the shortened preview shown in history lists, and keeps
its browsing state only in this panel's memory. It pages lazily through up to
200 tasks or 20 list pages; the Tasks page remains available for older history.

A delayed history response cannot replace text typed while it was loading, a
Reuse handoff, or a new task submission. Missing retained records are skipped;
other failures leave the draft untouched for retry, and a successful retry clears
its previous error. Recall applies the instruction to the current page named
under the field. Tasks-page Reuse remains the choice that also reopens the
original page; a follow-up proposes making that distinction visible beside each
recalled entry. Fifty-one focused recall/bar cases pass, with a separate worker case
for the new gestures. The local browser keyboard journey also uses a shared
helper that enters the selected panel tab before choosing its sibling; real
Chromium acceptance is left explicit for the driver.

Final cycle validation: all required and README checks passed, including 44
extension files, 51 focused recall/bar cases and 1,374 Rust-reported outcomes
(94 explicit IPC sandbox skips). Accessibility has 2 fixture passes and 39
explicit browser skips; external driver/CI acceptance remains required. Sixteen
new offline regressions and one new browser journey cover the cycle. The two
feature beads are closed; the shared keyboard-entry helper's bead stays blocked
for its explicit real-Chromium criterion.

## 2026-11-02 — Run a task without attaching a browser page

Brauser can now take a task such as “Organize my notes” from a new tab, browser
settings or another non-web tab. On an ordinary web page, the new **Include this
page** checkbox lets the user leave that page out while keeping their draft.
The context line says “No page · Runs in your knowledge base”, so the instruction
and its scope can be checked before Run. Ordinary page tasks keep their current
behavior, including visible failures when a page cannot be read or changes
while being extracted.

This is an explicit task mode, rather than an empty page capture. The extension
sends no page title, address or content; the host and service both check the
shape, and the service creates no page.md. The agent still works in the
configured knowledge-base folder under the existing task limits and file
protections, but its prompt contains no page-reading exception or document
retrieval section. History labels the task **No page** and leaves out Open page.
**Reuse** carries that choice back to the bar without opening a tab or silently
attaching the web page that happens to be active. Keyboard recall continues to
reuse only instruction text under the currently visible scope.

The complete local required and README gates pass, with eight new Rust/wire
checks and seven extension regressions plus a new keyboard/axe journey. The
Rust total is 1,382 reported outcomes (94 explicit IPC sandbox skips); all 44
extension files pass. Real Chromium cannot launch here: two fixture checks pass
and 40 browser cases explicitly skip, so the driver and platform CI still need
to establish that acceptance before publication. No live service or API changed.

## 2026-11-02 — Repair capture rules while keeping the original

A damaged capture-rule document no longer leaves removing every rule as the
only settings-page recovery. **Repair adapter document…** opens the preserved
adapter values, including settings from a future format, in a copyable TOML
view. Beside it is an explicitly authored complete format-1 JSON replacement
with immediate validation and a match preview across all its sites. Nothing
converts a future format automatically, and the original reset-all action stays
available. This lets someone repair one broken rule while retaining the useful
ones instead of rebuilding their entire capture setup.

Saving sends only the reviewed configuration revision and replacement rules.
The native host keeps the capture pause, chosen folders and agent settings,
checks recorded identities and the complete replacement, and backs up the exact
original file and comments beside config.toml before its atomic write. Another
settings change makes the open draft stale, preserving both texts for copying
and requiring a fresh review. The extension verifies the new host policy before
publishing it to capture. Focused Rust, wire and offline settings tests cover
future formats, malformed entries, exact backups, conflicts, identity refusal,
preview and cancellation; the real-browser journey also checks keyboard focus
return after saving. The driver's external gate remains the source of real
Chrome and platform acceptance.
