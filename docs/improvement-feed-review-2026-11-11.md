# Brauser improvement feed review — 2026-11-11

**Historical snapshot, preserved 2026-11-13.** This review came from a local checkout based on `4cd617df` (2026-11-10), rather than the GitHub tip when it was saved.
Its dated dispositions describe that checkout and may lag changes already merged upstream.
Later published completion markers and the current implementation take precedence; preserving this review does not revalidate all 115 entries.

Reviewed all 115 entries present in `docs/improvement-feed/` on 2026-11-11. Each decision below says what the proposal would change and why it is or is not still needed. “Done” means the entry describes implemented behavior or carries a completion marker; entries marked “Done or verify” need a final implementation or publication check before closing. “Keep” means the gap remains worth addressing; “Revise” means only a narrower or safer version is justified; “Retire” means the proposal is obsolete; “Merge” combines duplicate requests. Evidence pointers are local and bounded; this review did not run platform smoke tests or inspect remote PR publication.

## Summary

- 70 keep, 14 revise, 27 done or done pending verification, 2 retire, and 2 merge into one queue validation item.
- Close implemented entries only after verifying the cited commit or current code is published. Keep the original entry text intact and add the standard `Done` line.
- Consolidate the two November 3 queue validation requests. The native GitHub merge queue and committed derived queue views are obsolete under the current Mergify and Dolt setup.
- Prioritize user-visible gaps and data integrity over test harness conveniences. In particular, keep note ownership, Re-learn transitions, starter history, and scope disclosure visible in planning.
- The proposed committed salted hash denylist for private identifiers needs redesign: use local private comparisons so low-entropy identifiers cannot be guessed from public hashes.

## November 1–4

| Entry | Decision | What it does and why | Evidence |
|---|---|---|---|
| 11-01 feed format | Done | Checks entry names and headings so malformed suggestions fail CI. | Done marker; `scripts/check_improvement_feed.py` |
| 11-01 concurrent checkouts | Keep | Starts agents in separate worktrees so one session cannot move another session's branch or overwrite the live host build. | Recorded collision; main checkout remains live install per entry |
| 11-02 summary-check | Keep | Gives developers one command to render, run, and validate a page summary so prompt changes can be tested on real pages without throwaway code. | No `summary-check` CLI in `host/src/main.rs` |
| 11-02 physical note revisions | Done | Ties note saves to the actual file selected through re-learning, preventing a save to the wrong file. | Done marker `brwr-c1bq` |
| 11-02 DESIGN cache table | Revise | Add a narrow check of cache filenames to catch documentation drift; avoid brittle prose matching. | `DB_FILE`, `REGEN_LOCK`, `OPEN_LOCK` still exist in `host/src/index/` |
| 11-02 saved summary scope | Done | Labels summaries as full, partial, or viewer-only at selection time so readers know what was covered. | Done marker `brwr-30xv` |
| 11-02 GitHub merge queue | Retire | GitHub's native queue is unavailable for this personal-account repo; current Mergify process supersedes it. | Done marker; AGENTS describes Mergify |
| 11-02 TOML-only repair values | Keep | Tests NaN, infinity, and nested unknown values through native repair so config can be opened without losing types. | Entry records config-level test only; native path remains gap |
| 11-02 upgrade retry fixture | Revise | Keep an old interrupted journal fixture when changing event payloads so Retry save remains compatible; this is a regression rule rather than an independent feature. | Entry says `host/src/capture.rs` already covers the observed transition |
| 11-02 limited reading run card | Done | Shows reading coverage next to summary overview so a viewer-only result is not mistaken for complete reading. | Done marker `brwr-rr93` |
| 11-02 re-learn refusal explanation | Keep | Shows conflicting origins, notes, and before/after links when a re-learn review cannot be approved, giving users enough evidence to resolve it safely. | Re-learning apply/panel still not built per AGENTS current state |
| 11-02 flaky panel/smoke waits | Revise | Keep a focused CI reliability item for remaining timing failures; the original Capture Site cause was fixed, so do not retain that subcase as open. | 11-09 +1 says `brwr-b508` fixed ambiguous panel lookup |
| 11-02 padded differential tokens | Keep | Generates valid and invalid padded IDs so Rust and TypeScript matchers stay aligned beyond fixed examples. | `genValue` in `scripts/adapter-templates-diff.mjs` still builds body alphabet only |
| 11-02 a11y launch deadline | Keep | Fails accessibility tests clearly when Chrome startup hangs, avoiding an indefinite gate. | `scripts/a11y-browser.mjs` sets timeout only when sandboxed |
| 11-02 recognized a11y fixture | Keep | Audits the actual artifact, summary, and running task panel states in both themes, closing a coverage hole. | Entry says fixture only models unrecognized page |
| 11-02 fixed extension ID | Done | Gives the unpacked extension a stable ID for native host registration. | Done marker `brwr-q0i4` |
| 11-02 smoke wording checks | Revise | Include smoke assertions in UI text review so label changes do not break platform smoke; a lightweight checklist or test linkage suffices. | PR #242 mismatch recorded; TESTING smoke section exists |
| 11-02 fake DOM classes | Keep | Models CSS classes in panel tests so tests can detect when a control is visually shown or hidden. | `harness.mjs:40` still has no-op `classList` |
| 11-02 pause late auto-summary | Keep | Rechecks capture pause when an automatic summary starts, preventing a run that began reading before Pause. | Entry records race at `start_auto_summary` |
| 11-02 task working folder | Keep | Names the actual folder before Run so users know where the agent will work. | Entry reports generic “Runs in your knowledge base” label |
| 11-02 sibling starter | Keep | Offers the missing starter for a recognized site's other page type, reducing manual Settings navigation. | Entry describes configured-origin gap |
| 11-02 replace handmade adapter | Keep | Offers a reviewed migration from an unused hand-drafted rule to a starter, avoiding manual config and contract edits. | Entry records Miro migration requiring hand edits |
| 11-02 work choices pagination | Done | Lets people reach work beyond the capped first page before sharing it. | Done marker |
| 11-02 adapter repair rule preview | Keep | Shows rules that repair will retain, add, change, and remove before saving, reducing accidental loss. | Entry says complete-document repair can omit rules |
| 11-02 page read frame report | Keep | Records which browser frame supplied page text and whether Word loading timed out, making failed summaries diagnosable. | `readTab` currently returns page only in `extension/summarize.ts` |
| 11-02 missing harness environment | Done | Names missing variable names after setup fails so users can repair launchd configuration. | Done marker |
| 11-02 summary topic search | Keep | Searches summary overview text in Tasks so users can find a remembered decision without knowing its page title. | Entry identifies `filterTasks` gap |
| 11-02 sandbox health checks | Revise | Separate checks that need unsandboxed Chromium from in-sandbox worker checks, so a known launch block does not consume cycles. | Entry records Cookie idle cycles from strict a11y flag |
| 11-02 encoded padding URL shape | Keep | Recognizes `%3D` padding as an ID in page-pattern drafting, matching the existing URL evaluator. | Entry records shaper/evaluator mismatch |
| 11-02 summary overviews | Done | Shows saved overview where a person decides what to reopen. | Done marker |
| 11-02 recalled task source page | Keep | Shows the original page for a recalled instruction so a page-specific task is not accidentally run on the current page. | Entry says keyboard recall uses current page |
| 11-02 derived queue commits | Retire | Queue files have since left Git; no longer need a change to stop committing them. | AGENTS says `BOSUN_FEATURE_BACKLOG.md` never committed; docs/backlog frame and Dolt workflow |
| 11-02 history append conflicts | Keep | Makes dated history updates merge safely without masking actual edits. | AGENTS says `docs/history.md` holds dated notes; entry records conflicts |
| 11-03 built extension imports | Keep | Checks copied module imports after packaging so a TypeScript success cannot ship a missing JS file. | Entry records omitted `task-folders.js` |
| 11-03 final queue check | Merge | Same action as “validate committed queue export”; make one publication gate against final data. | Both entries cite missing `brwr-9eio` and `brwr-fzya` |
| 11-03 focus session sharing | Done or verify | Gives sharing an explicit focus-session choice, preventing same-day activities from being conflated. | AGENTS current state says explicit session sharing built; `brwr-1jeb` referenced |
| 11-03 later-page evidence comparison | Revise | Compare an immutable passage with a newly supplied page only on request; useful but lower priority until a user workflow needs it. | Entry says comparison is manual today; no demand evidence beyond one cycle |
| 11-03 host config repair fields | Keep | Tests invalid host-owned values through refresh and repair so a typed error remains repairable. | Task-folder escape to I/O error documented in entry |
| 11-03 history certification tool | Keep | Replays real browser-history visits against both matcher engines and records evidence, replacing hand-written certification scripts. | 11-10 +1 says five profiles still required hand script |
| 11-03 backlog API verify field | Revise | Document actual response fields and add a simple served-content check; avoid treating a wrong probe key as a service outage. | Entry documents `backlog_sections[].title/body` vs nonexistent `content` |
| 11-03 index timing tests | Done or verify | Replace load-sensitive time limits with outcome checks; the cited `brwr-gxag` change appears to have done this. | 11-04 +1; old test names absent in `host/src/index` search |
| 11-03 open saved task note | Keep | Adds Open note after Save to note so users can inspect the actual persisted report. | Entry says no direct action yet |
| 11-03 summary guidance preview | Keep | Shows effective SUMMARY_STYLE before a run so users can fix validation errors without spending a run. | Entry says errors currently appear only on run |
| 11-03 starter grammar gaps | Keep, split | Prioritize fragment matching and mode-specific title policy from measured misses; separate other three gaps by impact. | `docs/research/blind/SCORECARD.md` counts in entry |
| 11-03 scope maxLength | Done | Caps Settings scope input at the same length as Capture Site. | Done marker 2355e26 |
| 11-03 recorded Mergify tests | Done | Tests PR watcher on actual queue messages so advice follows real states. | Done marker #346 |
| 11-03 committed queue export | Merge | Same final queue validation as the other 11-03 item. | Duplicate evidence and command |
| 11-04 history order check | Keep | Rejects conflict markers and out-of-order dated history notes in CI, catching bad merges. | Entry records #312 and #308 conflict repairs |
| 11-04 IPC guard check | Keep | Finds subprocess tests that omit the sandbox IPC probe, preserving meaningful skip and strict-mode behavior. | 11-09 +1 found 36 more omissions |
| 11-04 Unicode wire bounds | Keep | Shares emoji boundary fixtures between Rust and extension validators, avoiding valid host values disconnecting the UI. | Recorded 256 character vs UTF-16 mismatch |
| 11-04 saved task commands Settings | Done | Adds authoring and repair UI for repeatable commands. | Done marker `brwr-kqt4` |
| 11-04 fuller reading filter | Done | Filters Recent to summaries with partial or unknown reading coverage. | Done marker `brwr-y1q2` |
| 11-04 fake disabled focus | Keep | Models disabled, hidden, and ancestor-fieldset focus loss so keyboard regressions are seen without a browser. | 11-05 and 11-09 +1 found later blind spots |
| 11-04 focus choices paging | Done | Pages through recorded sessions while retaining explicit selection. | Done marker `brwr-t4gf` |
| 11-04 exhausted contention phase | Done or verify | Adds phase and SQLite details to index contention diagnostics so a future Windows failure can be located. | Next entry says `brwr-bszr` diagnostics implemented |
| 11-04 Windows index stress | Keep | Runs repeated Windows concurrency tests with retained output to validate the index fix on the affected platform. | Entry says next Windows failure needed; no completed run cited |
| 11-04 composed command budget | Keep | Shows how much saved command text and extra request can fit before Run, avoiding commands that save but cannot execute. | Entry documents 4096-character composition mismatch |

## November 5–8

| Entry (2026-11-) | Verdict | Plain language: change and reason | Current evidence |
|---|---|---|---|
| 05 acknowledge-task-alerts | Keep | Mark a completed task alert read when its report is visibly selected in Tasks. This prevents a later alert for a result already viewed; a hidden tab must not count. | `extension/tasks.ts:376-390` selects the report and `:621-630` handles visibility, but never sends `task_notice_seen`; `extension/worker.ts:1612-1615` already supports that message. |
| 05 continue-through-matching-recent-summaries | Done; close | Let Recent load more matching results 32 at a time. People can reach older summaries without changing their search. | `extension/panel.ts:2061-2108` validates a snapshot and loads the next page, retaining filters and stale-response checks. The entry is stale. |
| 05 family-files-cannot-express-cross-origin-negative-vectors | Keep, broaden wording | Let a starter's tests cover URLs on other origins, including negative cases and positive cases on another tenant host. Current fixtures discard evidence about where a starter should and should not apply. | `scripts/generate-starters.mjs:600-602` rejects vectors outside the family origin; entry's Webex +1 documents a missed positive tenant case. |
| 05 starter-research-must-check-overlaps | Keep, narrow | During research, run the draft family through the generator and overlap check. This catches unexpressible rule relationships before a starter plan is considered ready. | `docs/superpowers/specs/2026-11-03-starter-families-design.md:62-70` research method omits it; `scripts/generate-starters.mjs:697-745` now supports `container_first`, so the Splunk-specific blocker has been fixed but the workflow gap remains. |
| 05 save-curated-work-sharing-drafts | Done; close after publication check | Save a selected-work preview locally so it can be reopened and explicitly compared later. This preserves careful curation after panel closure. | Entry has `Done` marker; `extension/sharing-drafts.ts`, `extension/sharing-draft-panel.ts`, and `AGENTS.md:21` describe built behavior. Its marker says publication pending, so verify the current branch is published before adding a final receipt. |
| 05 structural-drafter-over-generalizes-shared-prefix-routes | Keep, revise solution | Make the drafter distinguish routes that share an opening segment but diverge later. Today some real samples have to be excluded from structural tests; the proposed longer-prefix heuristic should be validated against both route families before adopting. | `protocol/starters/figma.json:559` and `vidcast.json` still set `samples_shape: false`; `docs/adapters/figma.branch.md:94-103` and `vidcast.video.md:103-109` explain the failed draft. |
| 08 check-node-before-live-deploy-builds | Done; close | Check the Node runtime before staging a live build, so an unsupported version fails early with a useful instruction. | Entry's PR #345 marker; `scripts/dev/deploy-live:88-104,536-538` performs the preflight. |
| 08 validate-scorecard-fixes-before-recording | Keep | Run a proposed starter rule through the actual engine before describing it as resolved. This catches grammar errors while the research and plan can still change. | `docs/research/blind/SCORECARD.md:40-51` records the earlier invalid `/{_}` rule and eventual fix; `docs/superpowers/specs/2026-11-03-starter-families-design.md:62-70` still lacks this check. |

## November 9

| Entry (2026-11-09) | Decision | Plain language: change and reason | Evidence |
|---|---|---|---|
| block-known-real-identifiers-with-a-salted-hash-denylist | Revise | Screen examples for known private identifiers before publishing; real identifiers reached public fixtures. A committed salt and hashes can still reveal low-entropy values by guessing, so keep the private list local and make CI check only safe synthetic patterns. | Entry; public-fixture incident described there |
| bound-panel-startup-waits | Keep | Name and time each startup step, then offer retry after a deadline; a five-minute apparent native-host connection hang obscured which step stalled. | Entry; `extension/panel.ts`, `options.ts` |
| check-guard-models-across-the-whole-sequence | Keep | Test ownership across all recorded pages and actual successive saves; one-step checks missed later identity changes. | Entry; `host/src/config.rs` randomized guards |
| check-open-prs-touching-a-file-before-changing-it | Revise | Warn about overlapping active PRs during PR preparation; overlapping pr-watch rewrites wasted work. Do not make a costly remote check mandatory before every file edit. | Entry; `scripts/dev/pr-watch.mjs` |
| check-private-evidence-before-choosing-synthetic-ids | Keep | Locally compare replacement IDs and shortened variants against private source evidence; a supposedly synthetic replacement might still match real history. Keep evidence out of CI and repo. | Entry; cited Miro fixtures |
| check-which-states-a-guard-sees-before-ruling | Keep | List every save path to the same ownership state before changing a guard; the first narrow fix missed live-adapter edits. | Entry; `host/src/config.rs` |
| clean-patch-equivalent-local-branches | Revise | Report squash-equivalent stale branches and offer verified backup before deletion; ancestry alone misses squash merges. Make deletion explicit and preserve unique commits. | Entry; `scripts/dev/worktree-hygiene` |
| distinguish-recorded-sessions-by-activity-time | Keep | Show when a recorded session was active so users can choose among duplicate names; UUID suffixes alone are not meaningful. | Entry; `host/src/index/focus.rs` |
| exercise-recovery-through-ordinary-request-entry-points | Keep | Test interrupted re-key recovery through actual startup and reads; a direct recovery-unit test missed nested lock and inconsistent snapshot behavior. | Entry; `host/src/relearn/transaction` |
| exercise-scoped-modes-in-the-save-model | Keep | Generate scope and mode edits in save tests; exact-path cases missed valid scoped saves being rejected. | Entry; `host/src/config.rs` |
| fingerprint-adapter-documents-by-content-not-debug | Keep | Hash adapter contents rather than Rust debug formatting; adding an unused field changed all fingerprints and triggered needless index rebuilds. | Entry; `host/src/index/mod.rs:503` |
| gate-a-dependents-compile-check-on-its-dependency-too | Done, close | Compile fuzz targets whenever code changes; an engine type change broke them even though fuzz files were untouched. The unconditional CI job exists. | `.github/workflows/ci.yml:145` |
| gate-every-use-of-a-starter-exception-alike | Keep | Use one condition for all effects of a special starter order; edited rules once changed runtime ordering while Settings hid the warning. | Entry; `extension/starter-adapters.ts` |
| generate-the-starter-adapter-mirror-in-vectors | Keep | Generate or eliminate the hand-maintained starter vector copy; title changes currently require duplicate edits. | Entry; `scripts/generate-starters.mjs` |
| grade-history-against-every-snapshot-not-only-the-latest | Keep | Grade starter rules against all saved private history samples; newer Chrome history may have lost older visits. | Entry; `protocol/starters/README.md` |
| highlight-changed-words-in-refreshed-sharing-entries | Keep | Mark changed words and URLs when reviewing a refreshed sharing entry; complete before/after rows make small changes hard to spot. | Entry; `extension/sharing-draft-panel.ts` |
| inject-re-key-filesystem-sync-errors | Keep, narrow | Inject failures at remaining write, rename and publication steps; existing sync regressions cover some barriers but callbacks after operations cannot prove error recovery. | Entry and +1; `host/src/relearn/transaction` |
| interstitial-titles-block-history-certification | Revise | Ignore generic loading and sign-in titles and add a private history grader; otherwise bad titles block certification. The `generic_titles` field already exists, so focus on grader and remaining title cases. | Entry; `extension/starter-families.generated.ts` |
| keep-bd-bootstrap-from-dirtying-tracked-config | Keep | Restore or reconcile tracked beads config after bootstrap; unintentional config edits can ride `git add -A` into PRs. | Entry; `.beads/config.yaml` |
| keep-setup-outside-performance-timers | Keep | Time only the operation a budget claims to measure; one TypeScript test included compiling fixtures and flaked on Windows. | Entry; `extension/tests/adapter-templates.test.mjs` |
| keep-the-reading-position-when-returning-to-recent | Keep | Preserve loaded Recent pages and reading position within the same panel session; returning currently discards paged results. | Entry; `extension/panel.ts` |
| keyboard-jump-to-activity-dock | Keep | Add a keyboard route to the latest dock item and restore focus when done; otherwise run actions require tabbing through the page. | Entry; `extension/panel.ts` |
| mutation-check-tests-that-pass-first-time | Keep | Show that a new regression test fails when its target behavior is broken; tests that pass before a fix may not exercise the intended path. | Entry; `AGENTS.md` test guidance |
| offer-scope-extension-from-out-of-scope-page | Keep | Open a prefilled scope edit from a recognized but excluded page; users currently have to find the scope field manually. | Entry; `extension/site-coverage.ts` |
| pin-live-deploy-to-green-revision | Keep | Require the exact tested main commit during live deploy; main may move after CI approval and a dry run can show an old checkout. | Entry; `scripts/dev/deploy-live` |
| pr-doctor-auto-refresh-stale-mergify | Revise | Let doctor refresh a green queued PR when Mergify is stale; two PRs needed manual refresh. Rate-limit and ensure status has truly stalled before commenting. | Entry; `scripts/pr-doctor.mjs` |
| pr-watch-reads-neutral-dequeued-titles | Done, close | Recognize neutral Mergify dequeues so the watch tool gives a repair action; entry records #364 completion. | Entry Done line |
| print-how-long-each-smoke-wait-took | Keep | Print wait durations and click counts on smoke failure; CI timestamps were needed to find a wrong panel document. | Entry; `scripts/smoke-macos.mjs` |
| prove-windows-re-key-directory-durability | Keep | Establish durable directory publication before allowing Windows Re-learn apply; current Windows sync is intentionally a no-op. | Entry; `host/src/vault.rs:1245` |
| quarantine-rollback-checks-bead-citations | Keep | After rolling back queue beads, detect references to discarded IDs before next cycle; dead citations blocked sync and left work unpublished. | Entry; `scripts/work_queue.py` |
| queue-a-pr-when-it-opens | Keep | Put new PRs in Mergify queue immediately so green checks can advance without an active session; one PR sat seven hours. | Entry; `scripts/dev/pr-watch.mjs` |
| read-mergify-history-from-comment-edits | Keep | Record Mergify's edited status comment when diagnosing queue behavior; current comment and check runs omit prior waiting details. | Entry; `scripts/tests/fixtures/mergify-comments.json` |
| read-redirect-hops-from-history-title-twins | Revise | Flag likely redirect pairs in history grading for human confirmation; three gaps were redirects and extra rules would duplicate artifacts. Equal titles/counts alone do not prove a redirect. | Entry; starter history spec |
| replace-fixed-panel-notice-test-waits | Keep | Await an explicit notice-read completion signal instead of sleeping 20 ms; Windows had one ordering flake. | Entry; `extension/tests/panel/summarize.test.mjs` |
| review-leading-rule-insertions-and-expansion | Done, close | Guard new leading rules and expanded coverage from changing recorded identity; entry records brwr-98mo and brwr-nhwb completion. | Entry Done line |
| review-live-adapter-precedence-changes | Done, close | Guard reordering of overlapping adapters; entry records brwr-qbub completion. | Entry Done line |
| select-keys-need-a-macos-cdp-fallback | Done, close if merged | Use Playwright selection on macOS for native select controls; raw CDP keys did not change their value. Entry describes implementation; confirm merge before marking Done. | Entry; `extension/tests/a11y/fixture.mjs` |
| share-history-invalidation-across-panel-views | Keep | Coordinate Recent and worklog invalidation after policy changes; separate subscriptions can expose stale results. | Entry; `extension/panel.ts` |
| show-recorded-routes-in-site-review | Keep | Preview alternate starter routes and label synthetic examples; one generated example per kind hides important routes. | Entry; `extension/options.ts` |
| site-level-adapter-bundles | Done, close | Offer a site card with per-kind selection and previews; entry records commit a6e2285. | Entry Done line |
| state-the-invariant-before-patching-guard-cases | Keep | Write the identity invariant and test reachable edit sequences before patching cases; five review rounds exposed neighboring gaps. | Entry and +1; `host/src/config.rs` |
| sweep-vm-archives-for-unpublished-cycles | Keep | Compare worker archive changes against main and salvage genuine unpublished work; several closed or blocked beads referred to code only on VM branches. | Entry; worker driver |
| time-ci-tests-before-caching-builds | Done as guidance, close | Measure compile and test-run time separately before changing CI; Windows slowdown came mostly from temp-disk tests and was already improved. | Entry; `.github/workflows/build-os.yml` |

## November 10

| Entry | Decision | What it does and why | Evidence |
| --- | --- | --- | --- |
| 2026-11-10-build-start-run-test-requests-from-one-helper.md | Keep, low priority | Put common test request fields in one helper so adding a protocol field does not require changing dozens of unrelated tests. | `host/src/service/record.rs:929`, `host/src/service/supervisor.rs:6120` still construct literals. |
| 2026-11-10-check-platform-callers-when-refactoring-shared-helpers.md | Keep | Add a review prompt to check platform gates on remaining callers after a helper refactor, preventing Windows-only lint failures. | `host/src/config.rs:1597` and Unix service callers; no matching guidance in `AGENTS.md`. |
| 2026-11-10-check-starter-upgrade-history-when-rules-change.md | Keep | Compare changed starter match rules with the previous shipped version and require history so existing saved adapters can upgrade. | `scripts/generate-starters.mjs:444` validates history shape but does not compare with a base revision. |
| 2026-11-10-exercise-policy-transitions-after-re-keying.md | Keep, scope to activation | Test removal, narrowing and restoration of capture rules after Re-learn applies so old notes keep the right physical owner. | `host/src/relearn/transaction/tests/note_history.rs` covers ordinary history and removal regression, not full transition matrix. |
| 2026-11-10-run-pr-doctor-when-a-pr-branch-is-pushed.md | Done | Run PR doctor after a PR push to clear stale conflict labels promptly. | `.github/workflows/pr-doctor.yml:24` already has `pull_request: synchronize` gated to `needs-merge`. |
| 2026-11-10-shellcheck-the-release-scripts-in-ci.md | Keep, revise | Lint release shell scripts on PRs so syntax and shell mistakes are found before release day. Use `pr-hygiene.yml` or another always-on path for script changes; a docs-only job alone will not run on code changes. | No `shellcheck` in `.github/workflows/ci.yml` or `pr-hygiene.yml`; release workflow remains tag/dispatch only. |
| 2026-11-10-show-when-the-page-editor-uses-a-preserved-note.md | Revise | Show that the editable note is stored under an earlier capture identity, so the user understands what is being edited. The requested read-only alternative-note view already exists. | `extension/panel.ts:847-899` lists and views preserved notes; no editor provenance indicator found. |
## Applied on 2026-11-12

The 115 assessed entry files now each carry an additive review line with the decision and a plain-language reason. Eight implementation-backed entries without a prior completion line received `Done` lines after verification on main at `4cd617d`; 28 assessed entries now have completion lines in total. The obsolete derived-queue proposal is marked `Retired`, and the duplicate committed-export proposal is marked `Superseded` by the final-export check. The native GitHub merge-queue entry already had a `Done` line explaining that its proposed mechanism was unavailable; its review line calls for retirement. The new feed-status suggestion is a separate 116th entry. These changes are local until published.
