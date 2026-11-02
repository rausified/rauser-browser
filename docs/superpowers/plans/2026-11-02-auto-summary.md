# Auto-summary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the user opens a recognized artifact that has no summary yet, Brauser summarizes it automatically with `SUMMARY.md`, after an optional configurable dwell (default 0 s), through the service's existing first-in, first-out run queue.

**Architecture:** The worker already recognizes artifacts (the adapter lease) and tracks attention (`activity.ts`). A new pure module, `extension/auto-summary.ts`, keeps per-tab candidates and says which are due. The worker reads a due tab with `readTab` and sends a new `start_auto_summary` request. The host refuses that request when the feature is off, no harness is set up, the page is not an artifact, or the artifact already has a summary (`ArtifactIndex::summary_for`). Otherwise it forwards an ordinary summary `start_run`, which the service queues FIFO beside manual runs (`host/src/service/queue.rs`, unchanged). Manual **Summarize & log** is unchanged and still re-summarizes on demand.

**Tech Stack:** Rust (host, protocol), TypeScript MV3 extension, JSON Schema protocol (`protocol/schema.json` → `python3 protocol/generate.py`).

**Spec:** this file's "Design" section (John, 2026-11-02): auto-summarize every recognized artifact; skip one that already has a summary; dwell is configuration, default 0 (summarize every page); a plain FIFO queue with no other rules.

## Design

- **Trigger.** A tab's cold open of a recognized artifact (the same `match` that `recordNavigation` gets) makes a candidate. The candidate is due once its page has finished loading (`tabs.onUpdated` `status: "complete"`) and, when `dwell_secs > 0`, once the tab has been focused for `dwell_secs` in total while showing that artifact. With dwell 0, a loaded background tab is due too.
- **One attempt per artifact per browser session.** After the worker submits an artifact, it records the artifact ID in `submitted` (`chrome.storage.session`, capped at 500, oldest dropped), so reopening it does not call the host again. The host's "already summarized" check is the authority across restarts.
- **Queue.** No new queue. The service scheduler is already FIFO, runs `MAX_RUNNING = 2` at once, and joins a second request for the same record key. Auto and manual runs share it in arrival order.
- **Config.** `auto_summary = { enabled = true, dwell_secs = 0 }` in `config.toml`, with `dwell_secs` from 0 to 3600. It is shown and edited on the settings page and carried to the worker in the policy lease. `enabled` is the off switch for a feature that spends harness time on every open. Decision taken: it defaults to on, as John asked for all pages.
- **Sign-in and access walls (John, 2026-11-02).** A sign-in page that redirects to another URL is no longer a recognized artifact, so it never becomes a candidate. For a wall shown at the artifact's own URL there are two defences:
  1. *The agent reports access.* SUMMARY.md asks for a required `access` field: `ok`, `sign_in`, `denied` or `not_found`. When it is not `ok`, the service saves no summary file and no summary event. The run succeeds with no `summary_path`, and the run card says why. Nothing is written, so the artifact stays eligible. This applies to manual runs too: summarizing a sign-in page is never useful.
  2. *Only a real read counts as "already summarized".* For `start_auto_summary`, the latest summary blocks a new one unless its file records `read_scope: page_only` and it is from an earlier local day. So a viewer-only summary is retried at most once a day.
- **Out of scope:** related artifacts and incoming sources (brwr-sm1, brwr-sm2), showing the overview in the UI (brwr-sm3), run priorities and budgets.

## Global Constraints

- Protocol stays `protocol_version` 8. The new messages and the new required `ConfigSnapshot.auto_summary` field are additive in the schema, but an older extension rejects them (`deny_unknown_fields`, `protocol-shape.ts`): **deploy host and extension together** (AGENTS.md task-bar precedent). Record this in DESIGN.md and AGENTS.md invariants.
- `start_auto_summary` never carries `kind`, `record_key`, `harness` or `instruction`: the host chooses them, as `start_task` does (`host/src/native.rs` `start_task`).
- The host refuses before connecting to the service for every skip reason. No skip path forwards anything.
- Page text never goes into logs beyond what `diag` already allows. Every new request path carries `request_id` in its span (AGENTS.md diagnostics rule).
- Every new wire test goes in a new `protocol/tests/wire_auto_summary.rs`. Settings UI tests go in `extension/tests/options.test.mjs`. The new module's tests go in `extension/tests/auto-summary.test.mjs`, added to `test:extension` in `package.json`.
- Toolchain: cargo at `~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin`, Node 24 (`.nvmrc`). Work in a worktree off `origin/main`, never `checkout -b` in the shared checkout.
- Coordinate with the `feat/sqlite-index` lane, which is rewriting `host/src/index.rs`. This plan calls only `ArtifactIndex::summary_for` through `native::open_index`, from one place; rebase onto that lane if it merges first.

## Review Focus

1. **One artifact open in two tabs at dwell 0** → one summary run, not two. The service joins the second `start_run` for the same record key, and `submitted` stops the second call once the first is in. Test it in Task 4 (`twoTabsOneArtifactSubmitOnce`) and Task 3 (`a_second_auto_request_joins_the_queued_run`).
2. **Manual Summarize on an already-summarized artifact** must still run. Only `start_auto_summary` checks for an existing summary. Task 3 test: `manual_start_run_ignores_an_existing_summary`.
3. **Harness not set up, or capture paused** → no run and no error shown to the user, and no lasting `submitted` mark. Once the user sets up a harness, the next open summarizes. Task 4 test: `notSetUpLeavesTheArtifactEligible`. Task 5 test: a lease with `capture_enabled: false` produces no `start_auto_summary`.
4. **Opening 20 artifacts quickly** → 20 runs in FIFO order. A manual run requested afterwards waits behind them, and nothing is dropped. Task 3 test: `auto_runs_queue_in_arrival_order` (scheduler level).
5. **A wall at the artifact's own URL** (Jira "Log in to continue", SharePoint "You need permission"): no summary file or event is written, and the next open tries again. Task 3b tests: `a_sign_in_answer_writes_nothing` and `a_page_only_summary_from_yesterday_does_not_block`.
6. **Service worker restarts mid-dwell** → candidates and accumulated focus survive (`chrome.storage.session`), and the dwell completes on the next alarm. Task 5 test: `dwellSurvivesWorkerRestart`.

---

### Task 1: Protocol messages and config field

**Files:**
- Modify: `protocol/schema.json`, then regenerate `protocol/src/generated.rs` and `protocol/ts/generated.ts` (`python3 protocol/generate.py`)
- Modify: `protocol/README.md` (message list)
- Create: `protocol/tests/wire_auto_summary.rs`

**Interfaces:**
- Produces (Rust names from generation; TS twins in `protocol/ts/generated.ts`):
  - `AutoSummaryConfig { enabled: bool, dwell_secs: u32 }` (schema: `dwell_secs` integer 0–3600), added as a required field `auto_summary` of `ConfigSnapshot`.
  - `StartAutoSummaryRequest { protocol_version, request_id, url, title, content, requested_at }`, type `start_auto_summary`, with the same bounds as `start_run` and no `selection`, `harness`, `record_key`, `kind` or `instruction`.
  - `AutoSummarySkippedResponse { protocol_version, request_id, reason: AutoSummarySkipReason }`, type `auto_summary_skipped`.
  - `AutoSummarySkipReason` enum: `disabled`, `not_set_up`, `not_an_artifact`, `already_summarized`.
  - A started auto run is answered with the existing `run_started`.

- [ ] **Step 1: Write the failing wire tests** in `wire_auto_summary.rs`:
  - `start_auto_summary_round_trips`: a full request deserializes and re-serializes unchanged, and the schema validates it (`common::schema_subset`).
  - `start_auto_summary_rejects_host_only_fields`: adding `record_key`, `kind`, `harness` or `selection` fails deserialization.
  - `auto_summary_skipped_round_trips_every_reason`: one round trip each for the four reasons; `"reason":"later"` fails.
  - `config_snapshot_requires_auto_summary`: a snapshot without `auto_summary` fails, and `dwell_secs: 3601` fails schema validation.
- [ ] **Step 2: Run `cargo test -p brauser-protocol --test wire_auto_summary`.** Expected: FAIL (types missing).
- [ ] **Step 3: Add the definitions to `schema.json`, run `python3 protocol/generate.py`, and update every `ConfigSnapshot` construction the compiler flags** (host default: `enabled: true, dwell_secs: 0`; test fixtures in `protocol/tests/common/mod.rs`).
- [ ] **Step 4: Run `cargo test -p brauser-protocol` and `python3 protocol/generate.py --check`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(protocol): start_auto_summary, auto_summary_skipped, config auto_summary`.

### Task 2: Host config

**Files:**
- Modify: `host/src/config.rs` (TOML struct near `summary_vocabulary` at ~1392, snapshot conversion ~1549/1576, defaults ~1948, validation ~2004)
- Test: `host/src/config.rs` tests

**Interfaces:**
- Consumes: `AutoSummaryConfig` (Task 1).
- Produces: `pub(crate) fn default_auto_summary() -> AutoSummaryConfig` (`enabled: true, dwell_secs: 0`). The `[auto_summary]` table is serde-defaulted, so an existing `config.toml` without it loads. `update_config` validates `dwell_secs <= 3600` with the error "auto_summary.dwell_secs must be at most 3600".

- [ ] **Step 1: Write failing tests:**
  - `an_old_config_without_auto_summary_loads_with_the_default`: snapshot has `enabled: true, dwell_secs: 0`.
  - `auto_summary_round_trips_through_update_config`: set `{false, 45}`, reload, read it back.
  - `dwell_over_an_hour_is_refused`: 3601 is refused with the message above, and the file is unchanged.
- [ ] **Step 2: Run `cargo test --manifest-path host/Cargo.toml --lib config::`.** Expected: FAIL.
- [ ] **Step 3: Implement** the defaulted field, the snapshot mapping both ways and the validation, following `summary_vocabulary`.
- [ ] **Step 4: Run the same tests.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(host): auto_summary config with dwell`.

### Task 3: Host `start_auto_summary`

**Files:**
- Modify: `host/src/native.rs` (dispatch at ~774 beside `Request::StartRun`, the span kind table at ~346 and ~404, and a new `fn start_auto_summary` beside `start_task` at ~867)
- Test: `host/src/native.rs` tests (follow the existing `start_task` forwarding tests and their fake service); `host/src/service/queue.rs` tests

**Interfaces:**
- Consumes: Task 1 types; `default_auto_summary` (Task 2); `page_record_key(RunPage, &mut ConfigStore)`; `open_index(...)` → `ArtifactIndex::summary_for(&str) -> Result<Option<SummaryView>>`; `forward(&Request, service)`.
- Produces: `fn start_auto_summary(value: StartAutoSummaryRequest, config: &mut ConfigStore, service: Option<&ServiceLink>) -> Response`. Checks in this order, refusing with `auto_summary_skipped` and never connecting:
  1. `auto_summary.enabled == false` → `disabled`
  2. `config.agent` is `None` → `not_set_up`
  3. `page_record_key` errors → the same `error` that `start_run` returns
  4. key kind `Url` → `not_an_artifact`
  5. `summary_for(key.value)` is `Some` and `summary_blocks_auto(&view, &storage, today)` (Task 3b) is true → `already_summarized`; an index that cannot open → `error(Internal, "history is unavailable")`. Until Task 3b lands, use `true` in place of `summary_blocks_auto`.
  6. otherwise forward `StartRunRequest { kind: Some(RunKind::Summary), record_key: Some(key), harness: None, selection: None, instruction: None, .. }`

- [ ] **Step 1: Write failing tests:**
  - `auto_summary_skips_when_disabled`, `..._when_no_harness`, `..._for_a_url_keyed_page`, `..._when_the_artifact_has_a_summary`: each returns the reason, and the fake service saw no connection.
  - `auto_summary_forwards_a_summary_run_for_a_new_artifact`: the service got `start_run` with kind summary and the artifact key, and the answer is `run_started`.
  - `manual_start_run_ignores_an_existing_summary` (Review Focus 2).
  - `a_second_auto_request_joins_the_queued_run` (Review Focus 1): two requests give the same `run_id`, the second with `existing: true`.
  - In `queue.rs`: `auto_runs_queue_in_arrival_order` (Review Focus 4): 20 summary submits with distinct keys, then one more, come out of `next_ready` in submit order.
- [ ] **Step 2: Run `cargo test --manifest-path host/Cargo.toml --lib -- native::tests::auto_summary queue::`.** Expected: FAIL.
- [ ] **Step 3: Implement `start_auto_summary`** and the dispatch and span entries.
- [ ] **Step 4: Run the same tests, plus `native::` and `service::`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(host): start_auto_summary skips summarized artifacts and queues the rest`.

### Task 3b: Sign-in walls (the `access` answer field, and page-only retry)

**Files:**
- Modify: `SUMMARY.md` (JSON shape and field rules), `host/src/answer.rs` (`Access`, the parser and the schema), `host/src/service/finisher.rs` (`SummaryFinisher::save`), `host/src/service/record.rs` (`RunAnswer` mapping), `host/src/summary.rs` (`recorded_read_scope`), `host/src/native.rs` (`summary_blocks_auto`), `protocol/schema.json` (`RunAnswer.access`, optional), `extension/summary-text.ts` and `extension/run-card.ts` (the not-saved line)
- Test: those files' tests, `protocol/tests/wire_runs.rs`, `extension/tests/run-card.test.mjs`

**Interfaces:**
- Produces:
  - `pub enum Access { Ok, SignIn, Denied, NotFound }` in `answer.rs`, serialized snake_case. `SummaryAnswer.access: Access` uses `#[serde(default)]`, where `Ok` is the default, so an older record reads as `ok`. `parse_answer` requires `access` and checks it right after `read_scope`; the error is "access must be ok, sign_in, denied, or not_found". `answer_schema` adds it to `required` with that enum.
  - In SUMMARY.md, the shape line is `"access": "ok | sign_in | denied | not_found"`, with this field rule: "access: ok when you could read the artifact, from the page file or by retrieving it. sign_in when the page shows a sign-in or session-expired screen instead of the artifact and you could not retrieve it; denied for a no-permission screen; not_found for a not-found or deleted page. When access is not ok, still fill every field from what you saw, briefly."
  - `SummaryFinisher::save`: when `answer.access != Access::Ok`, return `Ok(SavedRun { written_key: <the run's record key>, summary_path: None })` before touching the config, the file or the log.
  - `RunAnswer.access: Option<Access>` on the wire (an optional `"access"` property with the four values), filled from the answer.
  - `pub fn recorded_read_scope(file: &[u8]) -> Option<ReadScope>` in `summary.rs`: reads `read_scope:` from the leading frontmatter of a summary file `owned_summary` accepts, and returns `None` otherwise.
  - `fn summary_blocks_auto(view: &SummaryView, storage: &StorageConfig, today: Date) -> bool` in `native.rs`. It returns false only when the file at `view.summary_path` records `PageOnly` and the date of `view.summarized_at`, in its own offset, is before `today`. A missing or unreadable file blocks (returns true).
  - Panel text in `summary-text.ts`: `notSavedText(access)` returns "Not saved: the agent saw a sign-in page. Sign in, then summarize again." for `sign_in`, "Not saved: you don't have access to this page." for `denied`, and "Not saved: the page was not found." for `not_found`. The run card shows it in place of the saved-summary line when `answer.access` is set and not `ok`.

- [ ] **Step 1: Write failing tests:**
  - `answer.rs`: `access_is_required_and_matched_case_insensitively`; `an_old_record_reads_access_as_ok`.
  - `prompt.rs`: the existing `summary_prompt_asks_for_every_answer_field` now covers `access`, so add only `the_prompt_defines_each_access_value`.
  - `finisher.rs`: `a_sign_in_answer_writes_nothing` (Review Focus 5): no file under `summaries/`, no log line, no index row; `summary_path` is `None`.
  - `summary.rs`: `recorded_read_scope_reads_owned_files_only`.
  - `native.rs`: `a_page_only_summary_from_yesterday_does_not_block`, `a_page_only_summary_from_today_blocks`, `a_whole_summary_blocks`, `a_missing_summary_file_blocks`.
  - `wire_runs.rs`: `run_answer_access_is_optional`.
  - `run-card.test.mjs`: `aSignInRunShowsNotSaved`.
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement**, following the order of the Interfaces above. Regenerate the protocol after the schema change.
- [ ] **Step 4: Run `cargo test --manifest-path host/Cargo.toml --lib -- answer:: prompt:: finisher:: summary:: native::`, `cargo test -p brauser-protocol --test wire_runs`, and `npm run build:extension && node --test extension/tests/run-card.test.mjs`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(summary): skip saving sign-in and no-access pages; retry page-only auto summaries daily`.

### Task 4: Extension candidate logic (pure)

**Files:**
- Create: `extension/auto-summary.ts`
- Create: `extension/tests/auto-summary.test.mjs` (and add it to `test:extension`)

**Interfaces:**
- Produces:
  ```ts
  export interface AutoCandidate { artifact_id: string; url: string; title: string | null;
    queued_at: number; loaded: boolean; focused_ms: number; focused_since: number | null }
  export interface AutoSummaryState { version: 1; candidates: Record<string, AutoCandidate>; submitted: string[] }
  export const MAX_SUBMITTED = 500;
  export function emptyAutoSummaryState(): AutoSummaryState;
  export function noteArtifact(state, tabId: number, match: { artifact_id: string; url: string; title: string | null } | null, now: number): void;
  export function noteLoaded(state, tabId: number): void;
  export function noteAttention(state, focusedTabId: number | null, now: number): void;
  export function noteClosed(state, tabId: number): void;
  export function dueTabs(state, now: number, dwellMs: number): number[];      // oldest queued_at first
  export function nextDueInMs(state, now: number, dwellMs: number): number | null;
  export function markSubmitted(state, tabId: number): void;                     // removes candidate, records artifact
  export function dropCandidate(state, tabId: number): void;                     // removes without recording
  ```
- Rules: `noteArtifact` with the same artifact as the tab's candidate is a no-op. A different artifact replaces the candidate. `null` removes it. An artifact already in `submitted` makes no candidate. Focus accumulates only while that tab is the focused tab. `dueTabs` returns loaded candidates whose `focused_ms` plus any running focus is at least `dwellMs` (with `dwellMs` 0, any loaded candidate). `submitted` keeps the newest `MAX_SUBMITTED` entries.

- [ ] **Step 1: Write failing tests:**
  - `dwellZeroIsDueOnLoadEvenInBackground`
  - `dwellCountsOnlyFocusedTime`: 30 s background plus 20 s focused against 45 s is not due; 25 more focused seconds make it due.
  - `navigatingToAnotherArtifactResetsTheCandidate`
  - `twoTabsOneArtifactSubmitOnce` (Review Focus 1): both due, the first `markSubmitted` removes the second tab's candidate for the same artifact too.
  - `submittedIsCappedOldestFirst`
  - `notSetUpLeavesTheArtifactEligible` (Review Focus 3): `dropCandidate`, then reopening makes a candidate again.
  - `dueTabsIsFifo`
  - `nextDueInMsGivesTheShortestRemainingDwell`
- [ ] **Step 2: Run `npm run build:extension && node --test extension/tests/auto-summary.test.mjs`.** Expected: FAIL.
- [ ] **Step 3: Implement `extension/auto-summary.ts`.**
- [ ] **Step 4: Run the same test.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(extension): auto-summary candidate state`.

### Task 5: Worker wiring and `autoSummarizeTab`

**Files:**
- Modify: `extension/summarize.ts` (add `autoSummarizeTab`)
- Modify: `extension/worker.ts` (the lease, the navigation call sites at ~611 and ~680, `tabs.onUpdated` ~1330, `onActivated` ~1339, window focus, tab removal, a new alarm beside `SYNC_ALARM`)
- Test: `extension/tests/worker.test.mjs`, `extension/tests/agent.test.mjs` or the summarize tests' existing home

**Interfaces:**
- Consumes: Task 1 TS types; Task 4 functions; `readTab(tab, deps)`; `markRunsMayExist`; `leaseFor(config, revision)`.
- Produces:
  - `export async function autoSummarizeTab(tab: {id; url; title}, deps: SummarizeDeps): Promise<"started" | AutoSummarySkipReason>`. It calls `readTab`, then sends `start_auto_summary` and expects `run_started` or `auto_summary_skipped`. On `run_started` it calls `markRunsMayExist`, so an open panel shows the run card.
  - The lease gains `auto_summary: AutoSummaryConfig`.
  - Session key `AUTO_SUMMARY_KEY = \`${BINARY_NAME}-auto-summary\``.
  - Alarm `AUTO_SUMMARY_ALARM`, set to `nextDueInMs` (Chrome rounds up to 30 s). While the worker is alive, a `setTimeout` covers shorter dwells.
- Behavior: the worker drains due tabs one at a time, under `serialize`, in `dueTabs` order, and only when the lease is current, `capture_enabled` is on and `auto_summary.enabled` is on. Outcomes:
  - `started` or `already_summarized`: `markSubmitted`.
  - `not_set_up`, `disabled`, `not_an_artifact`, `unreadable` or `page_changed`: `dropCandidate`.
  - service unavailable or any other error: keep the candidate for the next alarm, and log `diag.warn("auto summary deferred", { request_id })`.

- [ ] **Step 1: Write failing tests**, using the fake Chrome and fake host in `worker.test.mjs`:
  - `recognizedOpenAtDwellZeroSendsOneStartAutoSummary`: a fixture adapter, a navigation, then `status: "complete"` gives exactly one `start_auto_summary` with the tab's URL and no `record_key`.
  - `unrecognizedPagesSendNothing`
  - `pausedCaptureOrDisabledAutoSummarySendsNothing` (Review Focus 3)
  - `alreadySummarizedIsNotAskedAgainThisSession`
  - `dwellSurvivesWorkerRestart` (Review Focus 6): the worker reloads from session storage mid-dwell, and the alarm fires the request once the focus total reaches the dwell.
  - `hostUnavailableKeepsTheCandidate`
- [ ] **Step 2: Run `npm run build:extension && node --test extension/tests/worker.test.mjs`.** Expected: FAIL.
- [ ] **Step 3: Implement `autoSummarizeTab` and the worker wiring.**
- [ ] **Step 4: Run `npm run typecheck && npm run lint && npm run test:extension`.** Expected: PASS.
- [ ] **Step 5: Commit** `feat(extension): auto-summarize recognized artifacts`.

### Task 6: Settings page

**Files:**
- Modify: `extension/options.ts` (load near ~311, save near ~1507) and the settings HTML it fills in (the agent/summary section)
- Test: `extension/tests/options.test.mjs`; the a11y gate (`npm run test:a11y`)

**Interfaces:**
- Consumes: `ConfigSnapshot.auto_summary`.
- Produces: a checkbox labelled "Summarize recognized artifacts automatically", and a number input labelled "Wait before summarizing (seconds; 0 = right away)" (min 0, max 3600, step 1), turned off while the checkbox is unchecked. Saving sends them in `update_config`. An out-of-range value shows the host's error as other config errors do.

- [ ] **Step 1: Write failing tests:** `autoSummarySettingsLoadFromConfig`, `autoSummarySettingsSaveInUpdateConfig` (`{enabled: false, dwell_secs: 90}` goes in the request), `dwellInputDisabledWhenAutoSummaryOff`.
- [ ] **Step 2: Run `npm run build:extension && node --test extension/tests/options.test.mjs`.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the options tests and `npm run test:a11y`.** Expected: PASS (or the documented sandbox skip).
- [ ] **Step 5: Commit** `feat(settings): auto-summary toggle and dwell`.

### Task 7: Docs, smoke and full gate

**Files:**
- Modify: `DESIGN.md` (§6.6 and §7.1 for the `access` field and the not-saved run; a new "Auto-summary" subsection after the Summarize & log section §5.2; §7 configuration; the rollback and deploy-together note), `AGENTS.md` (Current state row "Auto-summary", the invariant "deploy host and extension together"), `TESTING.md` (how to see an auto run in the headless smoke), `README.md` if it lists messages
- Modify: `scripts/smoke-macos.mjs`: the fake agent already answers summaries. Add one step: open a recognized fixture artifact with auto-summary on and wait for its summary file. Make sure the existing manual-summary steps still see the counts they expect. Set `enabled = false` in the smoke config for those steps if auto runs would change their counts.
- Modify: `.beads/issues.jsonl`: a bead for this feature, closed with `backlog_shipped`, then `python3 scripts/render_backlog.py`.

- [ ] **Step 1: Update the smoke and run `npm run smoke:macos:headless`.** Expected: PASS, including the new auto step.
- [ ] **Step 2: Run the README full gate** (cargo build/test/fmt/clippy, typecheck, lint, build/test extension, a11y, markdown-neutralizer, scripts, queue, dev-tools). Expected: all pass. Known flakes: `brwr-kpg`, which applies only under load.
- [ ] **Step 3: Write the docs**, then run `python3 scripts/check_current_state.py` and `python3 scripts/check_queue_consistency.py`. Expected: PASS.
- [ ] **Step 4: Commit, open the PR** (with the Owner and Handoff lines), **run one Codex pass alongside CI, merge with `gh pr merge --auto --merge`** (the merge queue is not enabled), **then `scripts/dev/deploy-live`** and tell John to reload the extension.
