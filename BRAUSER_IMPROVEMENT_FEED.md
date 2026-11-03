# Brauser Improvement Feed

Improvement suggestions for Brauser: the app, this repository, and how agents
build it. This is the single place for them; for Brauser work it replaces the
AMOS Improvement Feed. The rules are in [AGENTS.md](AGENTS.md#brauser-improvement-feed).

The entries live one per file in [docs/improvement-feed/](docs/improvement-feed/),
so two pull requests that each add one never edit the same lines. This page
holds no entries and should not need to change.

## Add an entry

Create `docs/improvement-feed/YYYY-MM-DD-<slug>.md`: the date you write it,
then a short slug of lowercase letters, digits and hyphens taken from the
title. If that name is taken, add `-2`. The file starts with its heading line,
then says what to change, why (the evidence from the task) and where:

```markdown
## YYYY-MM-DD — Title

Change: ... Why: ... Where: ...
```

Never rewrite another entry's text. Reinforce an entry by adding a
`- +1 YYYY-MM-DD: <evidence>` line at the end of its file, and close one with
`- Done YYYY-MM-DD: <PR, commit or bead>`.

`python3 scripts/check_improvement_feed.py` checks the names and headings (CI
runs it through `scripts/check_current_state.py`).

## Read the feed

By date, oldest first: `cat docs/improvement-feed/*.md`. Open entries only:
`grep -L '^- Done ' docs/improvement-feed/*.md`.
