#!/bin/sh
# Register the merge drivers .gitattributes names for the bookkeeping files
# (.beads/issues.jsonl and BOSUN_FEATURE_BACKLOG.md) in this clone.
#
# git keeps merge drivers in per-clone config, so every clone, worktree's
# parent clone and worker workspace runs this once after cloning. It is
# idempotent: rerunning rewrites the same config entries. The append-only
# logs use git's built-in union driver and need nothing here.
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
