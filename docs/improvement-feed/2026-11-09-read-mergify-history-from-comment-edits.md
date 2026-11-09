## 2026-11-09 — Read Mergify's queue history from its comment edits

What: When a PR's merge-queue behaviour needs explaining, or pr-watch needs a new
fixture, read Mergify's history from GitHub's edit log of its status comment
instead of reconstructing it from check runs or memory:
`gh api graphql -f query='query{repository(owner:"rausified",name:"rauser-browser"){pullRequest(number:<n>){comments(first:50){nodes{author{login} userContentEdits(first:60){nodes{editedAt diff}}}}}}}'`.
Each edit holds the full comment, with its `-*- Mergify Payload -*-` JSON, so the
log replays every state change (waiting, checking with each step, merged,
dequeued) with timestamps. Add the command to TESTING.md or a comment in
`scripts/tests/fixtures/mergify-comments.json` as the way to record new cases.

Why: It was the only source that showed what #339 looked like while stuck. The
current comment had already moved on to `checking`. It also showed that a
`waiting` payload carries `"required_conditions": []`, so unmet conditions exist
only in the Markdown checklist. A parser that trusts the JSON tree alone would
have missed the #339 stall (brwr-69tz).

Where: TESTING.md, `scripts/tests/fixtures/mergify-comments.json`,
`scripts/dev/pr-watch.mjs` `mergifyStatus`.
