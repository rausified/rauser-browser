# Extra rows for `scripts/dev/queue seed`

One-shot seeding data for moving the queue into Dolt (AGENTS.md "Work queue").
Each `*.jsonl` here holds rows that no committed `.beads/issues.jsonl` had but
the queue needs; pass it with `--extra`. `seed` validates every row and refuses
an id that the main file already has with different fields.

- `brwr-9eio.jsonl`: a closed stand-in for Cookie's database-only hold record
  for PR #262. Cookie PRs #265, #267 and #268 make `brwr-dbp` depend on it.
  Prefer Cookie's own row (`bd export` on its VM) if it can be had.

Delete this directory once the remote is seeded.
