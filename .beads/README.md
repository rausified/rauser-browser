# Beads - AI-Native Issue Tracking

Welcome to Beads! This repository uses **Beads** for issue tracking - a modern, AI-native tool designed to live directly in your codebase alongside your code.

## What is Beads?

Beads is issue tracking that lives in your repo, making it perfect for AI coding agents and developers who want their issues close to their code. No web UI required - everything works through the CLI and integrates seamlessly with git.

**Learn more:** [github.com/steveyegge/beads](https://github.com/steveyegge/beads)

## Quick Start

### Essential Commands

```bash
# Create issues after hydrating the local database from tracked history
scripts/dev/create-bead "Add user authentication"

# View all issues
bd list

# View issue details
bd show <issue-id>

# Update issue status
bd update <issue-id> --claim
bd update <issue-id> --status done

# Sync with Dolt remote
bd dolt push
```

### Working with Issues

Issues in Beads are:
- **Git-native**: Stored in Dolt database with version control and branching
- **AI-friendly**: CLI-first design works perfectly with AI coding agents
- **Branch-aware**: Issues can follow your branch workflow
- **Sync-ready**: Uses Dolt remotes for backup and team sharing

## Why Beads?

✨ **AI-Native Design**
- Built specifically for AI-assisted development workflows
- CLI-first interface works seamlessly with AI coding agents
- No context switching to web UIs

🚀 **Developer Focused**
- Issues live in your repo, right next to your code
- Works offline, syncs when you push
- Fast, lightweight, and stays out of your way

🔧 **Git Integration**
- Dolt-native sync via bd dolt push / bd dolt pull
- Branch-aware issue tracking
- Dolt-native three-way merge resolution

## Get Started with Beads

Initialize this repository's local database after installing Beads:

```bash
# Initialize once, then load the queue from the beads branch's worktree
bd init --init-if-missing --skip-agents --skip-hooks --non-interactive --prefix brwr
bd import "$(scripts/dev/queue path)/.beads/issues.jsonl"

# Create your first issue
scripts/dev/create-bead "Try out Beads"
```

## Learn More

In this repository the work queue is not on `main`: `.beads/issues.jsonl`
and `BOSUN_FEATURE_BACKLOG.md` (generated from it) live on the `beads`
branch, in the shared worktree `scripts/dev/queue path` prints. Edit a row
there and publish it with `scripts/dev/queue push`; never commit either file
on a work branch. Initializing an existing database does not load new rows,
and on a fresh clone `bd init --from-jsonl` finds no queue here: run
`bd import "$(scripts/dev/queue path)/.beads/issues.jsonl"` after `bd init`.
Use `scripts/dev/create-bead` in the checkout that owns the database: it
pulls the queue worktree, validates its export and runs `bd import` before
`bd create`, reserving closed IDs as well as open ones, then copies the new
row into the queue worktree for `scripts/dev/queue push`. The import uses the
default timestamp guard (never `--allow-stale`) to keep newer local scalar
fields; related labels, comments and dependencies may still merge. It
preserves local-only work rather than rebuilding the DB. Import progress goes
to stderr so `--json` still returns only the created issue on stdout. A
failed import prevents creation. `scripts/dev/queue from-db <id>…` copies
rows you changed with `bd update` into the queue worktree.

Linked worktrees edit the queue worktree's JSONL directly (or hydrate a
scratch database under `target/` from it and copy only the new row). The
helper refuses shared worktree databases and routing flags. See AGENTS.md
"Work queue" for the full rules.

- **Documentation**: [github.com/steveyegge/beads/docs](https://github.com/steveyegge/beads/tree/main/docs)
- **Quick Start Guide**: Run `bd quickstart`
- **Examples**: [github.com/steveyegge/beads/examples](https://github.com/steveyegge/beads/tree/main/examples)

---

*Beads: Issue tracking that moves at the speed of thought* ⚡
