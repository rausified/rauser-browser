#!/bin/sh
# Register the work queue's merge drivers (beads-jsonl for
# .beads/issues.jsonl, bosun-backlog for BOSUN_FEATURE_BACKLOG.md) in this
# clone. The queue lives on the `beads` branch, whose .gitattributes names
# them (AGENTS.md "Work queue").
#
# scripts/dev/queue does not need this: it passes the drivers on its own git
# command lines. Register them for plain git in the queue worktree (a manual
# `git pull --rebase` there); the queue worktree has no scripts/, so set
# BRAUSER_RENDER_BACKLOG to a checkout's scripts/render_backlog.py for that.
# git keeps merge drivers in per-clone config; this is idempotent. The
# append-only logs on main use git's built-in union driver and need nothing.
#
# Usage: sh scripts/install-merge-drivers.sh [PYTHON]
#   PYTHON defaults to python3 on PATH; the driver command records its
#   absolute path, so rerun this after moving or upgrading the interpreter.
set -eu
root=$(git rev-parse --show-toplevel)
cd "$root"
python=${1:-$(command -v python3 || command -v python || true)}
if [ -z "$python" ]; then
  echo "install-merge-drivers: no python3 on PATH; pass one as the first argument" >&2
  exit 1
fi
exec "$python" scripts/render_backlog.py --install-merge-driver
