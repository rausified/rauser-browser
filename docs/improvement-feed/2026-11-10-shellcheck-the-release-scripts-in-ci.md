## 2026-11-10 — Shellcheck the release scripts in CI

`packaging/macos/*.sh` (`brwr-3ppp`) runs only in `release-macos.yml`, which
runs on a tag or by hand, so a broken edit to a signing or notarization script
shows up only on release day. Its first real signed run will also be the first
run of `import-identity.sh` and `notarize.sh`. Add `shellcheck -x
packaging/macos/*.sh` to the ubuntu `docs-checks` job in `ci.yml`, or to
`pr-hygiene.yml` (ubuntu runners have shellcheck preinstalled). Also consider
dispatching `release-macos.yml` on a schedule (weekly, unsigned), so the
universal build and the image checks keep working between releases.
- Review 2026-11-12: **Keep.** Lint release shell scripts on PRs so syntax and shell mistakes are found before release day. Use `pr-hygiene.yml` or another always-on path for script changes; a docs-only job alone will not run on code changes.
