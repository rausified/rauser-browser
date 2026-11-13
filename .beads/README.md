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
# Once per clone: clone the queue from this repository's refs/dolt/data
# (runs bd bootstrap and keeps .beads/config.yaml as it was)
scripts/dev/queue bootstrap

# Create your first issue
scripts/dev/create-bead "Try out Beads"
```

## Learn More

In this repository the work queue is not a file in git. bd keeps it in its
Dolt database and syncs it through this repository's `refs/dolt/data`
(`bd sync` merges concurrent edits field by field). A fresh clone runs
`scripts/dev/queue bootstrap`. Edit beads with bd (`bd update`,
`bd close`, `scripts/dev/create-bead`), then publish with
`scripts/dev/queue sync`, which checks the queue before it pushes. Never
commit `.beads/issues.jsonl` or `BOSUN_FEATURE_BACKLOG.md`; read the backlog
with `scripts/dev/queue backlog --print`, and export JSONL with
`scripts/dev/queue export`. `worker-contract.json` at the repository root is
the contract every worker follows; AGENTS.md "Work queue" has the rules.

- **Documentation**: [github.com/steveyegge/beads/docs](https://github.com/steveyegge/beads/tree/main/docs)
- **Quick Start Guide**: Run `bd quickstart`
- **Examples**: [github.com/steveyegge/beads/examples](https://github.com/steveyegge/beads/tree/main/examples)

---

*Beads: Issue tracking that moves at the speed of thought* ⚡
