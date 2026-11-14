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
- +1 2026-11-02: the same stall hit 4 of 5 open PRs at once (#238, #249, #253, #255), mostly in BOSUN_FEATURE_BACKLOG.md, TESTING.md and history.md. The general fix is the PR doctor (feat/pr-doctor, `.github/workflows/pr-doctor.yml`): it detects every conflicting PR and repairs the bookkeeping-only and append-only ones. Splitting history.md into one file per entry is still worth doing to cut conflicts at the source.
- Review 2026-11-12: **Keep.** Makes dated history updates merge safely without masking actual edits.
