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
- Review 2026-11-12: **Done.** Names missing variable names after setup fails so users can repair launchd configuration.
