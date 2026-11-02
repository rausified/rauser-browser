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
