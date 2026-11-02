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
