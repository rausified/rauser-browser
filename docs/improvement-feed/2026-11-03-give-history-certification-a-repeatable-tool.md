## 2026-11-03 — Give "history" starter certification a repeatable tool

Change: add `scripts/dev/certify-from-history` (macOS). It copies Chrome's
`History` database read-only to a temporary file, selects the visits on one
starter family's origin, and runs every URL and tab title through both
adapter-template engines against the family's current rules. It prints the
evidence summary that `status.method: "history"` needs (visits checked, rules
and title forms covered, misses, pages that wrongly match), and with `--write`
sets `checked_on` and `checked_by` in `protocol/starters/<family>.json`.

Why: the starter-families decisions (2026-11-03) allow a starter to be
certified either by a person's live check or by real browsing history. Only
the live path has a procedure, so a history certification is a one-off,
hand-run query each time. Miro's draft was checked by hand against 12 Chrome
history visits on 2026-11-01, and D2 in the Week 45 fan-out had to choose
between waiting for John's hands-on check and an unrepeatable agent check.
A tool makes the history method reproducible and makes its evidence the same
every time.

Where: `scripts/dev/` (beside `worktree-hygiene`), `protocol/starters/README.md`
(the certification section), and `docs/adapters/<type>.md` status lines.
