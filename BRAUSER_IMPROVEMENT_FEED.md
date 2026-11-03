# Brauser Improvement Feed

Improvement suggestions for Brauser: the app, this repository, and how agents
build it. This is the single place for them; for Brauser work it replaces the
AMOS Improvement Feed. The rules are in [AGENTS.md](AGENTS.md#brauser-improvement-feed).

Append new entries at the end, each under its own `## YYYY-MM-DD — Title`
heading. Never rewrite or reorder earlier entries. Reinforce an entry with a
`- +1 YYYY-MM-DD: <evidence>` line under it, and close one with
`- Done YYYY-MM-DD: <PR, commit or bead>`.

## 2026-11-01 — Keep concurrent sessions out of each other's checkout

Two agent sessions worked in the main checkout (`work/rauserBrowser`) at once.
One switched to `chore/panel-drop-redundant-status` and committed while the
other ran `git checkout -b`, so the new branch stacked on the first session's
commit and moved that session's HEAD; untangling it took a cherry-pick onto
`origin/main` and a manual branch restore. The main checkout is also the live
install, so a `cargo build` or `cargo test` there replaces the running
`target/debug/brauser`. Change: have `scripts/dev/worktree-hygiene` (or a new
`scripts/dev/start-branch`) create every new branch as a worktree from
`origin/main`, and refuse `checkout`/`switch` in the main checkout when its HEAD
is not `main` or another session's branch is checked out there. Where: the
"PR ownership and handoff" section of AGENTS.md and `scripts/dev/`.

- Also logged in the AMOS Improvement Feed on 2026-11-01, before this feed
  existed.

## 2026-11-01 — Check this feed's format in CI

The AMOS feed's writer refuses an entry without its own dated heading; this
feed is a plain file with no such guard, so a malformed or rewritten entry
would go unnoticed. Change: extend `scripts/check_current_state.py` (it already
runs in the docs-only CI job) to check that `BRAUSER_IMPROVEMENT_FEED.md` keeps
its preamble, that every entry heading matches `## YYYY-MM-DD — Title`, and
that, compared with the base branch, the change only appends.

## 2026-11-02 — Say which shell variables a harness is missing under the service

Claude Code's first **Test and use** on the dev build failed with "Not logged
in · Please run /login". The cause was that `CLAUDE_CODE_USE_BEDROCK=1` and the
`ANTHROPIC_*_MODEL` names were exported only in `~/.zshenv`, which the launchd
service never reads, while the AWS keys in `~/.claude/settings.json` did reach
it. Copying those names into `agent.env` fixed it, but finding that took log
reading and a hand-built `env -i` repro. Change: when a harness probe fails,
have setup compare the provider variable names a login shell exports
(`$SHELL -lic env`, names only, never values) with the service's environment
plus `agent.env`. Then name the missing ones in the error ("Your shell sets
CLAUDE_CODE_USE_BEDROCK and 5 more that Brauser's service does not see; add them
to agent.env"). The name-only rule for `agent.env` still holds. Also log the
program as `claude`, not its version: Claude runs log `program: "2.1.316"` in
`harness spawned`. Where: `host/src/service/setup.rs`, `host/src/harness.rs`,
and the settings harness section.

- Done 2026-11-02: `host/src/service/shell_env.rs` and `LineChild::spawn_as` (this PR, branch `feat/harness-env-hint`).

## 2026-11-02 — Let the panel test harness see CSS classes

Change: have the fake DOM in `extension/tests/panel/harness.mjs` parse each
element's `class` attribute from the built `panel.html` and back `classList`
and `className` with a real set (`add`, `remove`, `toggle`, `contains`). Why:
hiding the page title and URL from the panel (they became `visually-hidden`)
could not be checked through `page.el(...)`, because `classList` there is a
no-op stub, so the regression test had to regex the raw `PANEL_HTML` string
instead. Any change that shows or hides something by class rather than the
`hidden` attribute has the same blind spot. Where: `harness.mjs` (`FakeElement`
and the id scan), then move the page-area check in
`extension/tests/panel/page-summary.test.mjs` onto it.

## 2026-11-02 — Offer "Replace with the starter" for an unused hand-drafted rule

John's Miro capture saved a drafted `board` adapter before Brauser had a
`miro.board` starter. Switching it to the starter took a hand edit of the live
`config.toml` and `adopted-contracts.json` under the config lock, because
removing an adapter records its contract and the overlap guard then refuses a
starter on the same route under another type (rausified/rauser-browser#219).
Change: when settings or the panel sees a non-starter adapter on a starter's
origin whose routes the starter covers, and that adapter has recorded no
activity, notes or summaries, offer **Replace with the <tool> starter**. It
removes the adapter without recording its contract and adds the starter in one
revision-checked save. Where: `extension/starter-adapters.ts`, the settings
gallery, and a host check that the adapter has no records.

## 2026-11-02 — Shape percent-encoded base64 padding as an ID

The page-pattern shaper now reads a trailing `=` or `==` as an `{id}`
(rausified/rauser-browser#215), but it never percent-decodes path segments,
so a link that encodes the padding as `%3D` (`/app/board/uXjVM0uuBLo%3D/`)
still becomes `{slug}`, and the structural drafter proposes nothing for it.
Matching already decodes `%3D` (`eval.token.miro_board_encoded_padding`).
Change: in `extension/site-patterns.ts`, treat a trailing `%3D` or `%3D%3D`
(any case) like `=` before classifying, and add a shared shapes row for it.

## 2026-11-02 — Separate in-sandbox health checks from the out-of-sandbox gate

Cookie idled for several cycles because the scaffold copies every `--test-cmd`
verbatim into the cycle prompt's health check, including
`BRAUSER_REQUIRE_A11Y_TESTS=1 npm run test:a11y`. That flag exists for the
unsandboxed commit gate and CI, where a skip means a broken setup, but inside
the Codex sandbox it turned a known, unfixable Chromium launch block into a
red suite, and the prompt forbids feature work on a red suite. The launcher now
skips as "blocked by sandbox" there even under the flag. Change: keep that
rule in code for every `BRAUSER_REQUIRE_*` flag, and add a `test:health`
script (or a documented gate-vs-health split in TESTING.md) that names which
commands are gate-only, so a new required-flag suite cannot stall the worker
the same way. Where: `package.json`, TESTING.md "Accessibility checks in a real
browser" and "Local IPC in sandboxes".


## 2026-11-02 — Explain why a Re-learn review cannot be approved

Change: add the conflicting origins and record keys to the host's bounded,
revision-bound conflict evidence, and translate those into plain-language
review messages. For damaged owned markers, show the existing path/line error
with guidance to inspect a backup and the local log before retrying; never
silently repair or choose a winning file. Why: the assigned brwr-eo7/brwr-r62
regressions reproduced an empty report from a torn why marker and a hidden
HTTP/HTTPS merge. Approval now refuses both, but issue codes alone do not tell
a person how to proceed. Where: host/src/relearn.rs, its strict index scan,
and the future panel Re-learn review under brwr-6x2. Tracked as brwr-pgxz
(discovered from both fixes); preserve read-only review and separate recovery.

## 2026-11-02 — Bind the physical note location into Re-learn save revisions

Change: give an alias-resolved note an internal logical key, physical ownership
key and selected file location, and bind a save's revision check to that
selection. Why: brwr-ruwj now projects one corrected artifact while retaining
both original summary paths and raw ownership keys; the migration design
challenge found that passing a corrected key to the existing note-save API
would create another file instead of updating the note the person loaded.
Where: host/src/native.rs note context, host/src/note.rs load/save and the
remaining brwr-dbp apply integration. Test stale saves across a move and a
move back, both-side notes and URL copies; preserve copy-out-only browser
recovery. This belongs to the assigned brwr-dbp work.

## 2026-11-02 — A `brauser summary-check` command for iterating on SUMMARY.md

What: add a developer command (host CLI, beside `brauser adapters relearn-plan`) that renders `SUMMARY.md` for a given `page.md` and knowledge base (`--print-prompt`), runs the configured harness once on it without the service, and validates the final answer with `answer::parse_answer`, printing the rendered summary file. Optionally `--compare` runs both harnesses.

Why: testing the SUMMARY.md v2 rewrite against real pages (a Jira ticket and a GitHub PR from earlier runs' `page.md`) meant adding a throwaway ignored test to render the prompt and parse the answer, then removing it before commit. The live runs found a real wording bug (a two-sentence `overview`) that no unit test can catch. John plans to keep evolving SUMMARY.md and may make it user-editable (brwr-sm4), so this loop will recur.

Where: `host/src/main.rs` (CLI), `host/src/service/prompt.rs` `summary_prompt`, `host/src/answer.rs` `parse_answer`, `host/src/summary.rs` `render_summary`; document it in TESTING.md.


## 2026-11-02 — Show summary overviews where people choose what to reopen

Change: finish brwr-sm3 by carrying SUMMARY.md's overview into run answers and
saved-summary views, then show it in the run card, artifact card and Recent,
with today's wording for older records. Why: brwr-sm1 now preserves the
recorded path into an artifact in its saved summary, but the person deciding
which summary to reopen still sees only its type and importance. The existing
one-sentence overview is the useful decision aid at that point. Where:
protocol/schema.json, host/src/summary.rs and service/record.rs,
extension/summary-text.ts, extension/run-card.ts and the panel. Use the
existing brwr-sm3 rather than create another proposal for the same work.

- Done 2026-11-02: brwr-sm3 carries overview through events, views and run answers; panel, Recent and run cards show it with legacy fallbacks.

## 2026-11-02 — Search the topic a summary actually describes

Change: extend Tasks history search to match a summary's overview as well as
its page title, keeping existing matches for older results. Why: brwr-sm3
now delivers the useful one-sentence description to each retained run, but
`filterTasks` still searches only the title and task instruction; remembering
a decision described by the summary does not find its run. Where:
`extension/task-history.ts`, `extension/tasks.ts`, their tests, and brwr-khqi
(discovered from brwr-sm3).

## 2026-11-02 — Exercise upgrade retries when extending durable event payloads

Change: keep a pre-change journal fixture and replay it with the new caller
payload whenever adding a summary event field. Why: the overview tests covered
old log reading and cache reopen, but an older interrupted append failed Retry
save because its payload hash lacked the new field. The review regression in
`host/src/capture.rs` now covers this transition; apply the same check to future
event extensions alongside the frozen single-event recovery tests.

## 2026-11-02 — Include platform smoke expectations in UI wording reviews

Change: add an explicit check of `scripts/smoke-macos.mjs` when reviewing changes
to panel or run-card text. Why: PR #242 updated the summary unit tests but left
the macOS smoke assertion expecting the old labels; the built panel reproduced
the mismatch on Linux even though the macOS runner itself cannot run there.
Where: the UI change checklist in `TESTING.md` and the platform smoke assertions.


## 2026-11-02 — Generate padded tokens in differential tests

Change: extend `scripts/adapter-templates-diff.mjs`'s `genValue` to emit
unpadded tokens and one/two-character padding within the selected bounds,
then exercise these values through titles and aliases as well as URLs.
Keep malformed padding among the near misses. Why: `brwr-tp1` reproduced
padded-title failures in both engines, but the random generator uses only the
body alphabet and cannot explore this supported grammar family. Fixed shared
vectors now cover it; generated combinations would catch future drift with
case modifiers, bounds and title structure. Where: the generator and
`scripts/tests/adapter-templates-diff.test.mjs`; tracked as `brwr-ys8y`.


## 2026-11-02 — Offer a configured site's missing sibling starter in the panel

Change: when a page on a configured starter origin matches no rule but would
match a starter of the same origin family that the site does not have yet
(a Confluence page on a site with only the Jira starter, a GitHub issue on a
site with only pull requests), say so in the capture explanation and offer
**Review starter**, handing that one starter to the settings gallery's editor.
Why: Capture Site refuses any site that already has rules, and `nearMiss`
only offers rules of an adapter's own starter, so a Jira-configured Atlassian
site visiting Confluence reads "captures Jira issues; this page isn't one of
them" with no way forward from the panel; after three visits the
recurring-pages hint drafts a structural `wiki`-style adapter instead of the
shipped `confluence.page` starter, which would then need re-learning. Where:
`nearMiss` in extension/site-coverage.ts (a third `starter_type` kind built
from `starterOffers` and `hasAdapter`), the `no_rule` case of
renderCaptureExplanation in extension/panel.ts, and `recurringHint` in
extension/capture-site.ts (skip shapes a missing sibling starter matches).
## 2026-11-02 — Give the a11y browser launch a deadline and a clear failure

What: in `scripts/a11y-browser.mjs`, wrap the Playwright `launch` (and the first page open) in a bounded timeout, for example 60 s, that fails the whole `npm run test:a11y` run at once with "Chrome did not start within 60 s (CHROME_PATH=…); is another Chrome using the profile, or is the machine overloaded?", instead of letting `node --test` wait forever.

Why: during the auto-summary build (load average 20–27), `npm run test:a11y` first failed every test with "Promise resolution is still pending but the event loop has already resolved" after a 17.7 s first test. A rerun then hung for over 10 minutes with no Chrome child process and no output after the fixture test, and had to be killed by hand. The same machine passed the headless smoke and every other gate, so the gate's result said nothing about accessibility.

Where: `scripts/a11y-browser.mjs` (the shared launcher), `extension/tests/a11y/*.test.mjs`; note the timeout and its message in TESTING.md "Accessibility checks in a real browser".


## 2026-11-02 — Explain limited reading beside a summary overview

Change: show the validated whole/partial/page-only reading scope in the run
card and Tasks detail, as proposed in `brwr-rr93`. Why: fixing next-day retry
(`brwr-sm9`) exposed that a viewer-only summary can look finished in the panel
while the host deliberately allows another attempt tomorrow. A short label
would help a person judge the overview and understand a later retry. Where:
`protocol/schema.json`, `host/src/service/record.rs`, `extension/run-card.ts`
and the built run-card/Tasks tests. Keep the label text-only and omit it for
legacy answers without scope.
- +1 2026-11-02: brwr-sm5 now tells agents when browser capture was capped,
  including automatic summaries. Carry the validated resulting scope into saved
  summary choices in Recent and This page too (`brwr-30xv`), where people decide
  whether to reopen an artifact without opening Tasks detail. Keep this distinct
  from the capture flag: a capped page can still yield a whole-artifact summary
  after the agent retrieves it. The run-card projection is pending held PR #249.

- Done 2026-11-02: `brwr-rr93` (cycle commit prepared for driver).

## 2026-11-02 — Make capture pause stop late automatic starts

Change: check `capture_enabled` under the same native eligibility lock as
`auto_summary.enabled` (`brwr-q0i4`). Why: the `brwr-sm7` review found that a
pause during page extraction is not rechecked by `start_auto_summary`; an
already-reading page can still start a run after capture is paused. Where:
`host/src/native.rs` and its offline auto-summary regression tests. Explicit
Summarize & log should stay available while capture is paused.

## 2026-11-02 — Give the extension a fixed ID before M5

The self-registering app (DESIGN.md §11) writes `allowed_origins` for one
stable extension ID, but the development build has no `key` in
`extension/manifest.json`, so each unpacked load path gets its own ID and
`scripts/register-dev-host.mjs` has to be told it. Change: add a pinned public
`key` to the manifest (matching the future Web Store item's key once it
exists), so development, the release app and `brauser register` share one ID
and the registration code can be tested against it before M5. Where:
`extension/manifest.json`, `scripts/register-dev-host.mjs`, and the
self-registration tests when that code is built.
- Done 2026-11-02: `brwr-q0i4` (cycle commit prepared for driver).


## 2026-11-02 — Disclose reading scope when choosing a saved summary

Change: carry validated saved-summary reading scope to the Summary line in
This page and to Recent, omitting it for legacy records (`brwr-30xv`,
discovered from `brwr-rr93`). Why: scope is now visible in run cards and Tasks
detail, but the overview shown while choosing a remembered artifact can still
look complete when the agent read only its browser viewer. Where:
`SummaryView` in `protocol/schema.json`, the host's saved-summary projection,
`extension/summary-text.ts` and the built page-summary/Recent tests. Use
recorded scope rather than inference from overview text.

## 2026-11-02 — Give the browser fixture a recognized-artifact panel state

Change: add a `recognized` state to `extension/tests/a11y/fixture.mjs` whose
fake host answers `get_artifact` with a recognized artifact (mode, measures,
`within`, incoming navigation), a saved summary with an overview, and one
running task, and audit it in light and dark. Why: the side panel redesign
added the page card's chips, the Summary card's tags and run-card state
indicators, but the fixture only models an unrecognized page with no summary,
so the axe gate never measures those surfaces and visual checks need DOM
injection by hand. Where: `extension/tests/a11y/fixture.mjs` and
`pages.test.mjs`; the same state would let a screenshot check compare the
panel against the design.

## 2026-11-02 — Record which frame a page read came from and whether the Word wait timed out

Change: have `readTab` return a small read report (each frame's host, trimmed
character count and `word` flag, which frame was chosen, and whether the Word
paragraph wait ended by paragraphs, timeout or navigation, with its elapsed
ms), and log it through the extension's diagnostics and the host's
`request done kind=start_run|start_task` line. Why: diagnosing run 92dcfad9
("Word is starting." as page.md) had only the 17-character page.md and the
host's `thin_office` flag; nothing said which frame produced the text, so the
fix had to key on "no Word paragraphs yet" without ever seeing the splash's
markup, and a future timeout (empty Word content) will look the same as an
unrendered document. Where: extension/summarize.ts `readTab`/`choosePage`,
extension/diag.ts, protocol/schema.json (an optional field on `start_run` and
`start_task`), host/src/service/capture_quality.rs for the log line.
## 2026-11-02 — Show the source page when recalling a task

Change: add a compact original-page cue and an explicit route to inspect or
reopen the recalled task (`brwr-k6rg`). Why: keyboard recall reuses the complete
stored instruction on the current page, whereas Tasks-page Reuse also reopens
its original page; page-specific instructions deserve a visible distinction
before Run. Keep the draft, current-page context and Enter/Run boundary intact;
never navigate automatically. Where: `extension/task-recall.ts`, the task-bar
context in `extension/panel.ts`, and `extension/tests/panel/task-bar.test.mjs`.

## 2026-11-02 — Stop docs/history.md from blocking PRs on append conflicts

Change: decide how `docs/history.md` merges so that two PRs each adding a
dated note no longer conflict. Either give it `merge=union` in
`.gitattributes`, as the feed, `CookieFeature.md` and
`PENDING_AMOS_DOC_UPDATES.md` already have, or, since history.md is sectioned
and union would also silently keep both sides of a real mid-file edit, have
new notes go to one append-only tail section that union can handle safely.
Why: PR #246 (Word-loading fix) went DIRTY twice in one afternoon, once after
Cookie's cycle-8 merge (#252). Both times the only real conflict was two new
`2026-11-02` entries in history.md. A DIRTY PR runs no CI, so auto-merge sat
for about four hours with nothing failing visibly, until a status check found
it. Also: the PR owner check could flag a PR whose auto-merge is armed but
whose state is DIRTY, so the owner hears about it. Where: `.gitattributes`,
the AGENTS.md history.md rule, and `scripts/pr-owner.mjs` or a small hygiene
job.

## 2026-11-02 — Name the working folder before running a task

Show the configured knowledge-base folder beside the task scope in the panel
(`brwr-5xpb`), using host-projected configuration as text. The no-page task work
(`brwr-qin`) makes the instruction independent of a tab, but its context line
still says only “Runs in your knowledge base”; a person choosing among multiple
note/project folders cannot see the actual working location before Run. Extend
`extension/panel.ts`'s task context and its built panel tests, retaining setup
hints and leaving the separate per-task chooser (`brwr-zla`) for later.

## 2026-11-02 — Preview which rules an adapter repair removes

Show a plain-language comparison of recognized adapters kept, changed, added
and removed before a complete-document repair is saved (`brwr-jxrw`, discovered
from `brwr-jnj`). The repair workspace protects the exact original with a backup
and shows preserved values, but manually rebuilding a document can still omit
useful supported rules. Compare supported-format values only; keep future
formats opaque and say when a comparison is unavailable. Add this review to
`extension/adapter-repair-settings.ts` and its offline settings tests, retaining
the revision gate and explicit replacement action.

## 2026-11-02 — Exercise TOML-only values through the native repair response

Extend the native adapter-repair fixtures with a small TOML value corpus,
including positive/negative infinity and nested unknown fields. Independent
review reproduced a repair workspace that could not open when an unknown
artifact field held NaN: copying from the JSON snapshot lost its type. The
config-level regression now covers that case and dates; checking the same
values through `get_adapter_repair` in `host/src/native.rs` would protect the
complete transport path as it evolves.
