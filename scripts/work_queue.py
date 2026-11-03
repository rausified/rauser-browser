#!/usr/bin/env python3
"""Keep Brauser's work queue on its own `beads` branch.

The queue (`.beads/issues.jsonl`) and its rendered view
(`BOSUN_FEATURE_BACKLOG.md`) live on the `beads` branch, not on `main`, so
pull requests never carry them and never conflict on them (AGENTS.md "Work
queue"). Sessions and Cookie push the branch directly: no PR, no CI beyond
the queue check `.github/workflows/queue-check.yml` runs on that branch.

Every clone keeps one shared worktree of the branch, beside the clone:
`<clone>-queue-beads` (for work/rauserBrowser that is
work/rauserBrowser-queue-beads), or $BRAUSER_QUEUE_DIR when set. All of a
clone's worktrees and sessions use that one directory; every command that
changes it holds an exclusive lock (`<git common dir>/brauser-queue.lock`),
so concurrent calls queue up instead of racing.

Commands (scripts/dev/queue is the entry point):

  path              print the queue worktree's path, creating it on demand
  pull              fetch origin/beads and fast-forward the worktree (local
                    edits are kept: a rebase with --autostash)
  push [-m MSG]     re-render the backlog, run the queue check, commit and
                    push to origin beads; a push race is retried with a
                    rebase that uses the bead-by-bead merge driver. Never
                    forces.
  from-db ID...     copy those beads' rows from this checkout's bd database
                    (`bd export`) into the queue worktree, then push yourself
  adopt [--from REF] [--base REF] [-m MSG] [--dry-run]
                    carry the queue edits a branch made (REF's queue files
                    against BASE's, default the merge base with origin/main)
                    into the beads branch and push them. PR doctor uses this
                    for PRs opened before the queue left main. Refuses, and
                    writes nothing, when an edit conflicts.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
from typing import Iterator, Optional

BRANCH = "beads"
REMOTE = "origin"
QUEUE_PATH = ".beads/issues.jsonl"
BACKLOG_PATH = "BOSUN_FEATURE_BACKLOG.md"
QUEUE_FILES = (QUEUE_PATH, BACKLOG_PATH)
DIR_ENV = "BRAUSER_QUEUE_DIR"
# The clone-wide merge drivers (scripts/install-merge-drivers.sh) run the
# renderer this names when the worktree being merged has no
# scripts/render_backlog.py, as the queue worktree never does; set it to merge
# there with plain git. scripts/dev/queue itself passes the drivers with -c.
RENDERER_ENV = "BRAUSER_RENDER_BACKLOG"
LOCK_NAME = "brauser-queue.lock"
PUSH_ATTEMPTS = 5

SCRIPTS = pathlib.Path(__file__).resolve().parent


class QueueError(Exception):
    """A queue operation cannot go ahead; the message says why and what to do."""


def git(*args: str, cwd: pathlib.Path, check: bool = True,
        env: Optional[dict[str, str]] = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8",
        env={**os.environ, **(env or {})},
    )
    if check and result.returncode != 0:
        raise QueueError(f"git {' '.join(args)} failed: {(result.stderr or result.stdout).strip()}")
    return result


# --------------------------------------------------------------------------
# Where the queue lives


def repo_root(start: Optional[pathlib.Path] = None) -> pathlib.Path:
    """The working tree to act for: START, else this script's checkout, else cwd."""
    for candidate in ([start] if start else []) + [SCRIPTS, pathlib.Path.cwd()]:
        result = git("rev-parse", "--show-toplevel", cwd=candidate, check=False)
        if result.returncode == 0:
            return pathlib.Path(result.stdout.strip()).resolve()
        if start:
            break
    raise QueueError(f"not inside a git repository: {start or pathlib.Path.cwd()}")


def common_dir(repo: pathlib.Path) -> pathlib.Path:
    out = git("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=repo).stdout.strip()
    return pathlib.Path(out).resolve()


def queue_dir(repo: pathlib.Path) -> pathlib.Path:
    """`<clone>-queue-beads` beside the clone's main worktree, or $BRAUSER_QUEUE_DIR.

    The location depends only on the shared git directory, so every worktree
    of a clone, and every session, finds the same queue worktree.
    """
    override = os.environ.get(DIR_ENV)
    if override:
        return pathlib.Path(override).expanduser().resolve()
    common = common_dir(repo)
    clone = common.parent if common.name == ".git" else common
    name = clone.name[:-4] if clone.name.endswith(".git") else clone.name
    return clone.parent / f"{name}-queue-beads"


@contextlib.contextmanager
def locked(repo: pathlib.Path) -> Iterator[None]:
    """Hold the clone's queue lock: one queue command at a time per clone."""
    path = common_dir(repo) / LOCK_NAME
    with open(path, "a+", encoding="utf-8") as handle:
        try:
            import fcntl
        except ImportError:  # Windows: these are macOS/Linux dev tools
            yield
            return
        fcntl.flock(handle.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def fetch(repo: pathlib.Path) -> None:
    git("fetch", "-q", "--no-tags", REMOTE, f"+refs/heads/{BRANCH}:refs/remotes/{REMOTE}/{BRANCH}", cwd=repo)


def _is_queue_worktree(repo: pathlib.Path, path: pathlib.Path) -> bool:
    if not (path / ".git").exists():
        return False
    other = git("rev-parse", "--path-format=absolute", "--git-common-dir", cwd=path, check=False)
    if other.returncode != 0 or pathlib.Path(other.stdout.strip()).resolve() != common_dir(repo):
        return False
    head = git("symbolic-ref", "-q", "HEAD", cwd=path, check=False).stdout.strip()
    return head == f"refs/heads/{BRANCH}"


def _ensure_unlocked(repo: pathlib.Path) -> pathlib.Path:
    path = queue_dir(repo)
    if path.exists():
        if _is_queue_worktree(repo, path):
            return path
        raise QueueError(
            f"{path} exists but is not this clone's worktree of the {BRANCH} branch; "
            f"move it aside (or set {DIR_ENV}) and run this again"
        )
    fetch(repo)
    has_local = git("rev-parse", "-q", "--verify", f"refs/heads/{BRANCH}", cwd=repo, check=False).returncode == 0
    if has_local:
        git("worktree", "add", "-q", str(path), BRANCH, cwd=repo)
        git("branch", "-q", f"--set-upstream-to={REMOTE}/{BRANCH}", BRANCH, cwd=repo, check=False)
    else:
        git("worktree", "add", "-q", "--track", "-b", BRANCH, str(path), f"{REMOTE}/{BRANCH}", cwd=repo)
    print(f"queue: created the {BRANCH} worktree at {path}", file=sys.stderr)
    return path


def ensure(repo: pathlib.Path) -> pathlib.Path:
    """The queue worktree, created (from origin/beads) when it is missing."""
    path = queue_dir(repo)
    if path.exists() and _is_queue_worktree(repo, path):
        return path
    with locked(repo):
        return _ensure_unlocked(repo)


# --------------------------------------------------------------------------
# Pull and push


def driver_config() -> list[str]:
    """`git -c` options that register the two queue merge drivers for one command.

    They name this checkout's renderer by absolute path, so a rebase in the
    queue worktree (which has no scripts/) merges bead by bead, and no code
    from whatever branch the clone has checked out runs (PR doctor checks out
    PR branches). Equivalent to scripts/install-merge-drivers.sh, without
    touching the clone's config.
    """
    python = shlex.quote(pathlib.Path(sys.executable).as_posix())
    script = shlex.quote((SCRIPTS / "render_backlog.py").as_posix())
    options = []
    for name, flag in (("beads-jsonl", "--merge-jsonl"), ("bosun-backlog", "--merge-backlog")):
        options += ["-c", f"merge.{name}.name=Brauser queue ({name})",
                    "-c", f"merge.{name}.driver={python} {script} {flag} %O %A %B"]
    return options


def _ahead(path: pathlib.Path) -> int:
    out = git("rev-list", "--count", f"{REMOTE}/{BRANCH}..HEAD", cwd=path).stdout.strip()
    return int(out or "0")


def _dirty(path: pathlib.Path) -> bool:
    return bool(git("status", "--porcelain", cwd=path).stdout.strip())


def _rebase(repo: pathlib.Path, path: pathlib.Path) -> None:
    """Rebase local commits (and any edits, stashed) onto origin/beads."""
    result = git(*driver_config(), "rebase", "-q", "--autostash", f"{REMOTE}/{BRANCH}", cwd=path, check=False)
    if result.returncode == 0:
        return
    if git("diff", "--name-only", "--diff-filter=U", cwd=path).stdout.strip():
        # The drivers stop only on a bead whose one field both sides changed
        # with the same updated_at, or on prose edited twice.
        resolved = subprocess.run(
            [sys.executable, str(SCRIPTS / "render_backlog.py"), "--resolve"],
            cwd=path, capture_output=True, text=True,
        )
        if resolved.returncode == 0:
            cont = git(*driver_config(), "-c", "core.editor=true", "rebase", "--continue", cwd=path, check=False)
            if cont.returncode == 0:
                return
    raise QueueError(
        f"rebasing the queue onto {REMOTE}/{BRANCH} stopped on a conflict in {path}: "
        f"fix the conflicted files, `python3 {SCRIPTS / 'render_backlog.py'} --resolve` "
        f"and `git rebase --continue` there (or `git rebase --abort`), then push again.\n"
        f"{(result.stderr or result.stdout).strip()}"
    )


def pull(repo: pathlib.Path) -> pathlib.Path:
    with locked(repo):
        path = _ensure_unlocked(repo)
        fetch(repo)
        if not _dirty(path) and _ahead(path) == 0:
            git("merge", "-q", "--ff-only", f"{REMOTE}/{BRANCH}", cwd=path)
        else:
            _rebase(repo, path)
    return path


def _render_and_check(path: pathlib.Path) -> None:
    issues, backlog = path / QUEUE_PATH, path / BACKLOG_PATH
    steps = (
        ("render_backlog.py", ["--issues", str(issues), "--backlog", str(backlog)]),
        ("check_queue_consistency.py", ["--issues", str(issues), "--backlog", str(backlog)]),
    )
    for script, args in steps:
        result = subprocess.run([sys.executable, str(SCRIPTS / script), *args], capture_output=True, text=True)
        if result.returncode != 0:
            raise QueueError(f"{script} failed; nothing was committed:\n{(result.stdout + result.stderr).strip()}")


def _commit(path: pathlib.Path, message: str) -> bool:
    git("add", "-A", cwd=path)
    if git("diff", "--cached", "--quiet", cwd=path, check=False).returncode == 0:
        return False
    git("commit", "-q", "-m", message, cwd=path)
    return True


def _push_unlocked(repo: pathlib.Path, path: pathlib.Path, message: str) -> Optional[str]:
    """Render, check, commit and push. Returns the pushed commit, or None if nothing changed."""
    _render_and_check(path)
    _commit(path, message)
    fetch(repo)
    for _attempt in range(PUSH_ATTEMPTS):
        if _ahead(path) == 0:
            return None
        result = git("push", "-q", REMOTE, f"HEAD:refs/heads/{BRANCH}", cwd=path, check=False)
        if result.returncode == 0:
            git("update-ref", f"refs/remotes/{REMOTE}/{BRANCH}", "HEAD", cwd=path, check=False)
            return git("rev-parse", "HEAD", cwd=path).stdout.strip()
        text = result.stderr + result.stdout
        if not any(word in text for word in ("rejected", "non-fast-forward", "fetch first", "stale info")):
            raise QueueError(f"git push failed: {text.strip()}")
        # Someone pushed first: replay ours on theirs, bead by bead.
        fetch(repo)
        _rebase(repo, path)
        _render_and_check(path)
        _commit(path, f"{message} (re-rendered after a rebase)")
    raise QueueError(f"the {BRANCH} branch kept moving; {PUSH_ATTEMPTS} push attempts lost the race. Run push again.")


def push(repo: pathlib.Path, message: str) -> Optional[str]:
    with locked(repo):
        path = _ensure_unlocked(repo)
        return _push_unlocked(repo, path, message)


# --------------------------------------------------------------------------
# Rows from bd, and edits from another branch


def _render_backlog():
    sys.path.insert(0, str(SCRIPTS))
    import render_backlog  # noqa: PLC0415 (render_backlog imports this module lazily too)

    return render_backlog


def upsert_rows(queue_file: pathlib.Path, rows: list[str]) -> list[str]:
    """Replace or insert whole JSONL rows by id, keeping `bd export`'s line order."""
    rb = _render_backlog()
    text = queue_file.read_text(encoding="utf-8") if queue_file.exists() else ""
    records = {issue_id: line for issue_id, (line, _) in rb._records(text, str(queue_file)).items()}
    changed = []
    for row in rows:
        record = json.loads(row)
        issue_id = record["id"]
        if records.get(issue_id) != row.strip():
            records[issue_id] = row.strip()
            changed.append(issue_id)
    ordered = sorted(records.values(), key=lambda line: rb.export_order(json.loads(line)))
    rb.write(queue_file, "".join(line + "\n" for line in ordered))
    return changed


def bd_rows(root: pathlib.Path, ids: Optional[set[str]] = None) -> dict[str, str]:
    """Rows of `bd export` in ROOT's database, by id (all of them when IDS is None)."""
    result = subprocess.run(["bd", "export"], cwd=root, capture_output=True, text=True, encoding="utf-8")
    if result.returncode != 0:
        raise QueueError(f"bd export failed: {result.stderr.strip()}")
    rows = {}
    for line in result.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        record = json.loads(line)
        if record.get("_type", "issue") != "issue" or not isinstance(record.get("id"), str):
            continue
        if ids is None or record["id"] in ids:
            rows[record["id"]] = line
    return rows


def from_db(repo: pathlib.Path, ids: list[str]) -> list[str]:
    rows = bd_rows(repo, set(ids))
    missing = [issue_id for issue_id in ids if issue_id not in rows]
    if missing:
        raise QueueError(f"bd export has no bead {', '.join(missing)}")
    with locked(repo):
        path = _ensure_unlocked(repo)
        return upsert_rows(path / QUEUE_PATH, [rows[issue_id] for issue_id in ids])


def _show(repo: pathlib.Path, rev: str, file: str) -> Optional[str]:
    result = git("show", f"{rev}:{file}", cwd=repo, check=False)
    return result.stdout.replace("\r\n", "\n") if result.returncode == 0 else None


def resolve_insertions(text: str) -> Optional[str]:
    """TEXT (a diff3 merge) with every conflict resolved, or None.

    Only a hunk whose base is empty (both sides inserted text at the same
    point, as two cycles appending observations do) is resolved: ours, then
    theirs. Any other hunk, or a marker-looking line inside a side, returns
    None. scripts/pr-doctor.mjs resolveAppendOnly is the same rule for PRs.
    """
    out: list[str] = []
    lines = text.splitlines(keepends=True)
    i = 0
    while i < len(lines):
        if not lines[i].startswith("<<<<<<< "):
            out.append(lines[i])
            i += 1
            continue
        parts: dict[str, list[str]] = {"ours": [], "base": [], "theirs": []}
        part, i = "ours", i + 1
        while i < len(lines) and not lines[i].startswith(">>>>>>> "):
            line = lines[i]
            if line.startswith("<<<<<<< "):
                return None
            if line.startswith("||||||| ") or line.rstrip("\n") == "|||||||":
                if part != "ours":
                    return None
                part = "base"
            elif line.rstrip("\n") == "=======":
                if part != "base":
                    return None
                part = "theirs"
            else:
                parts[part].append(line)
            i += 1
        if i >= len(lines) or part != "theirs" or parts["base"]:
            return None
        ours, theirs = parts["ours"], parts["theirs"]
        if ours and not ours[-1].endswith("\n"):
            ours[-1] += "\n"
        out += ours + theirs
        i += 1
    return "".join(out)


def merge_prose(base: str, ours: str, theirs: str) -> Optional[str]:
    """Three-way merge of hand-written text; both-sides insertions keep both."""
    with tempfile.TemporaryDirectory() as tmp:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            path = pathlib.Path(tmp) / name
            path.write_text(text, encoding="utf-8", newline="\n")
            paths.append(str(path))
        result = subprocess.run(
            ["git", "merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs", *paths],
            capture_output=True, text=True, encoding="utf-8",
        )
    if result.returncode < 0 or result.returncode > 127:
        raise QueueError(f"git merge-file failed: {result.stderr.strip()}")
    return result.stdout if result.returncode == 0 else resolve_insertions(result.stdout)


def merge_edits(base: dict[str, Optional[str]], ours: dict[str, str], theirs: dict[str, Optional[str]]) -> dict[str, str]:
    """The queue files with THEIRS' edits (against BASE) applied on OURS.

    The queue merges bead by bead (render_backlog.merge_jsonl); the backlog's
    hand-written text merges three ways and its generated blocks are
    re-rendered from the merged queue. Raises QueueError on any conflict.
    """
    rb = _render_backlog()
    queue, conflicts, _notes = rb.merge_jsonl(base[QUEUE_PATH] or "", ours[QUEUE_PATH], theirs[QUEUE_PATH] or ours[QUEUE_PATH])
    if conflicts:
        raise QueueError(f"bead(s) {', '.join(conflicts)} changed differently on both sides; carry them by hand")
    issues = rb.parse_issues(queue, QUEUE_PATH)
    backlog = ours[BACKLOG_PATH]
    their_backlog, base_backlog = theirs[BACKLOG_PATH], base[BACKLOG_PATH]
    if their_backlog is not None and base_backlog is not None and their_backlog != base_backlog:
        try:
            skeletons = [rb.strip_generated(text) for text in (base_backlog, backlog, their_backlog)]
        except rb.BacklogError as error:
            raise QueueError(f"{BACKLOG_PATH}: {error}") from error
        merged = merge_prose(*skeletons)
        if merged is None:
            raise QueueError(f"{BACKLOG_PATH}: a hand-written conflict that is not two insertions; carry the prose by hand")
        backlog = merged
    elif their_backlog is not None and base_backlog is None and their_backlog != backlog:
        raise QueueError(f"{BACKLOG_PATH}: no base version to merge against; carry the prose by hand")
    try:
        return {QUEUE_PATH: queue, BACKLOG_PATH: rb.render(backlog, issues)}
    except rb.BacklogError as error:
        raise QueueError(f"{BACKLOG_PATH}: {error}") from error


def adopt(repo: pathlib.Path, source: str, base: Optional[str], message: str, *, dry_run: bool) -> Optional[str]:
    """Carry SOURCE's queue edits (against BASE) onto the beads branch."""
    fetch(repo)
    if base is None:
        found = git("merge-base", source, f"{REMOTE}/main", cwd=repo, check=False)
        if found.returncode != 0:
            raise QueueError(f"{source} has no merge base with {REMOTE}/main; pass --base")
        base = found.stdout.strip()
    theirs = {file: _show(repo, source, file) for file in QUEUE_FILES}
    base_files = {file: _show(repo, base, file) for file in QUEUE_FILES}
    if theirs[QUEUE_PATH] is None and theirs[BACKLOG_PATH] is None:
        print(f"queue: {source} has no queue files; nothing to adopt")
        return None
    if theirs == base_files:
        print(f"queue: {source} did not change the queue files since {base[:12]}; nothing to adopt")
        return None
    with locked(repo):
        path = _ensure_unlocked(repo)
        if not dry_run:
            if _dirty(path) or _ahead(path):
                _rebase(repo, path)
            else:
                git("merge", "-q", "--ff-only", f"{REMOTE}/{BRANCH}", cwd=path)
        ours = {file: (path / file).read_text(encoding="utf-8") for file in QUEUE_FILES}
        merged = merge_edits(base_files, ours, theirs)
        # Check a scratch copy first, so a failing check leaves the shared
        # worktree exactly as it was (the next push would commit it otherwise).
        with tempfile.TemporaryDirectory() as tmp:
            for file, text in merged.items():
                (pathlib.Path(tmp) / file).parent.mkdir(parents=True, exist_ok=True)
                _render_backlog().write(pathlib.Path(tmp) / file, text)
            _render_and_check(pathlib.Path(tmp))
        if dry_run:
            print(f"queue: {source}'s queue edits apply cleanly (dry run; nothing pushed)")
            return None
        for file, text in merged.items():
            _render_backlog().write(path / file, text)
        return _push_unlocked(repo, path, message)


# --------------------------------------------------------------------------
# CLI


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="scripts/dev/queue", description=__doc__.splitlines()[0])
    parser.add_argument("--repo", type=pathlib.Path, help="act for this clone (default: this checkout)")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("path", help="print the queue worktree path (created on demand)")
    sub.add_parser("pull", help="fetch and fast-forward the queue worktree")
    push_parser = sub.add_parser("push", help="render, check, commit and push the queue")
    push_parser.add_argument("-m", "--message", default="queue: update beads")
    db_parser = sub.add_parser("from-db", help="copy beads' rows from this checkout's bd database")
    db_parser.add_argument("ids", nargs="+")
    adopt_parser = sub.add_parser("adopt", help="carry a branch's queue edits onto the beads branch")
    adopt_parser.add_argument("--from", dest="source", default="HEAD")
    adopt_parser.add_argument("--base")
    adopt_parser.add_argument("-m", "--message")
    adopt_parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)

    try:
        repo = repo_root(args.repo.resolve() if args.repo else None)
        if args.command == "path":
            print(ensure(repo))
        elif args.command == "pull":
            path = pull(repo)
            print(f"queue: {path} at {git('rev-parse', '--short=12', 'HEAD', cwd=path).stdout.strip()}")
        elif args.command == "push":
            sha = push(repo, args.message)
            print(f"queue: pushed {BRANCH} at {sha}" if sha else "queue: nothing to push")
        elif args.command == "from-db":
            changed = from_db(repo, args.ids)
            print(f"queue: updated {', '.join(changed) or 'nothing'}; publish with scripts/dev/queue push")
        elif args.command == "adopt":
            message = args.message or f"queue: adopt bead edits from {args.source}"
            sha = adopt(repo, args.source, args.base, message, dry_run=args.dry_run)
            if sha:
                print(f"queue: pushed {BRANCH} at {sha}")
        return 0
    except QueueError as error:
        print(f"queue: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
