"""Who is working on what: scripts/dev/overlap-check and scripts/dev/who-owns.

Both are read-only. They ask GitHub (`gh`), the bead queue (bd, through
scripts/work_queue.py), `git worktree list` and, when it is installed, Orca
(`orca worktree list --json`), and print what they find. A source that cannot
answer is named in the output and the rest still runs.

overlap-check [PATH ...] [--branch [--base REF]]
    The open PRs whose files touch PATH (a file, or a directory and everything
    under it), each with its `Owner:` line, and the in-progress beads that
    mention PATH or that a matching PR cites (title, body or branch), each with
    its assignee. --branch takes the paths from the current branch's changes
    against its merge base with REF (default origin/main) and skips the
    branch's own PR. Always exits 0; any match is printed between OVERLAP and
    END OVERLAP lines, so a session reads it before it edits.

who-owns TARGET
    TARGET is a PR (`12`, `#12` or its URL), a bead (`brwr-xxx`), a worktree
    path, or a branch. Prints the owner session (the PR's `Owner:` line), the
    bead assignee and notes, the worktree path and the Orca workspace for the
    branch, and the PRs and beads linked to it. Exits 1 when nothing matches.

Owner lines are read the way scripts/pr-owner.mjs reads them: HTML comments
and fenced code are ignored and a `<placeholder>` is not an owner.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
from typing import Optional, TextIO

SCRIPTS = pathlib.Path(__file__).resolve().parent
BEAD_ID = re.compile(r"\bbrwr-[a-z0-9]{2,}(?:\.[0-9]+)*\b")
PR_URL = re.compile(r"/pull/(\d+)(?:[/?#].*)?$")
OWNER_LINE = re.compile(r"^[ \t]*(?:[-*+][ \t]+)?(?:\*\*|__)?Owner(?:\*\*|__)?[ \t]*:(?:\*\*|__)?[ \t]*(?P<owner>.*)$",
                        re.IGNORECASE | re.MULTILINE)
LANE_LINE = re.compile(r"^[ \t]*(?:[-*+][ \t]+)?(?:\*\*|__)?Lane / coordinator(?:\*\*|__)?[ \t]*:(?:\*\*|__)?"
                       r"[ \t]*(?P<lane>.*)$", re.IGNORECASE | re.MULTILINE)
BEAD_TEXT_FIELDS = ("title", "description", "notes", "design", "acceptance_criteria")
PR_FIELDS = "number,title,headRefName,body,url,state,files"
GIT_ENV = {"GIT_OPTIONAL_LOCKS": "0", "GIT_TERMINAL_PROMPT": "0"}  # read-only git never takes index.lock


class SourceError(Exception):
    """A source (gh, bd, git, orca) could not answer; the message says why."""


def _visible(body: str) -> str:
    body = re.sub(r"<!--[\s\S]*?(?:-->|$)", "", body)
    return re.sub(r"^[ \t]*(```|~~~)[\s\S]*?(?:^[ \t]*\1[ \t]*$|\Z)", "", body, flags=re.MULTILINE)


def _line(pattern: re.Pattern, body: Optional[str], group: str) -> Optional[str]:
    match = pattern.search(_visible((body or "").replace("\r\n", "\n")))
    value = match.group(group).strip() if match else ""
    return value if value and not re.fullmatch(r"<[^>]*>", value) else None


def owner_line(body: Optional[str]) -> Optional[str]:
    """The PR body's filled-in `Owner:` value, or None (scripts/pr-owner.mjs's rule)."""
    return _line(OWNER_LINE, body, "owner")


def lane_line(body: Optional[str]) -> Optional[str]:
    return _line(LANE_LINE, body, "lane")


def parse_worktrees(text: str) -> list[dict]:
    """`git worktree list --porcelain` as [{path, head, branch}] (branch None when detached)."""
    trees, current = [], None
    for line in text.replace("\r\n", "\n").splitlines():
        if line.startswith("worktree "):
            current = {"path": line[len("worktree "):], "head": None, "branch": None}
            trees.append(current)
        elif current is not None and line.startswith("HEAD "):
            current["head"] = line[len("HEAD "):]
        elif current is not None and line.startswith("branch "):
            current["branch"] = line[len("branch "):].removeprefix("refs/heads/")
    return trees


def parse_orca(payload: object) -> Optional[list[dict]]:
    """The worktree list from `orca worktree list --json`'s envelope, or None."""
    if not isinstance(payload, dict) or not payload.get("ok"):
        return None
    result = payload.get("result")
    trees = result.get("worktrees") if isinstance(result, dict) else None
    return [t for t in trees if isinstance(t, dict)] if isinstance(trees, list) else None


def _run(argv: list[str], cwd: pathlib.Path, timeout: int = 60) -> str:
    try:
        result = subprocess.run(argv, cwd=cwd, capture_output=True, text=True, encoding="utf-8",
                                stdin=subprocess.DEVNULL, timeout=timeout, env={**os.environ, **GIT_ENV})
    except (OSError, subprocess.SubprocessError) as error:
        raise SourceError(f"{argv[0]}: {error}") from error
    if result.returncode != 0:
        raise SourceError(f"{' '.join(argv[:3])} failed: {(result.stderr or result.stdout).strip()[:300]}")
    return result.stdout


class Sources:
    """The live sources. Tests replace every method with fixture data."""

    def __init__(self, repo: pathlib.Path) -> None:
        self.repo = repo

    def _gh_json(self, *args: str) -> object:
        if not shutil.which("gh"):
            raise SourceError("gh is not installed")
        return json.loads(_run(["gh", *args], self.repo))

    def open_prs(self) -> list[dict]:
        return self._gh_json("pr", "list", "--state", "open", "--limit", "200", "--json", PR_FIELDS)

    def prs_for_branch(self, branch: str) -> list[dict]:
        return self._gh_json("pr", "list", "--head", branch, "--state", "all", "--limit", "20", "--json", PR_FIELDS)

    def pr(self, number: int) -> Optional[dict]:
        try:
            return self._gh_json("pr", "view", str(number), "--json", PR_FIELDS)
        except SourceError as error:
            if "no pull requests found" in str(error).lower() or "could not resolve" in str(error).lower():
                return None
            raise

    def beads(self) -> list[dict]:
        """Every bead, from a scratch `bd export` (full fields, including notes and assignee)."""
        sys.path.insert(0, str(SCRIPTS))
        import work_queue  # noqa: PLC0415

        with tempfile.TemporaryDirectory(prefix="brauser-owners-") as scratch:
            try:
                path = work_queue.export(self.repo, pathlib.Path(scratch) / "issues.jsonl")
            except work_queue.QueueError as error:
                raise SourceError(str(error)) from error
            text = path.read_text(encoding="utf-8")
        return [json.loads(line) for line in text.splitlines() if line.strip()]

    def worktrees(self) -> list[dict]:
        try:
            return parse_worktrees(_run(["git", "worktree", "list", "--porcelain"], self.repo))
        except SourceError:
            return []

    def orca_worktrees(self) -> Optional[list[dict]]:
        if not shutil.which("orca"):
            return None
        try:
            return parse_orca(json.loads(_run(["orca", "worktree", "list", "--json"], self.repo, timeout=20)))
        except (SourceError, ValueError):
            return None

    def changed_files(self, base: str) -> list[str]:
        merge_base = _run(["git", "merge-base", base, "HEAD"], self.repo).strip()
        return [p for p in _run(["git", "diff", "--name-only", merge_base, "HEAD"], self.repo).splitlines() if p]

    def current_branch(self) -> Optional[str]:
        try:
            return _run(["git", "symbolic-ref", "--short", "-q", "HEAD"], self.repo).strip() or None
        except SourceError:
            return None


# --------------------------------------------------------------------------
# overlap-check


def _norm(path: str) -> str:
    path = path.replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path


def touches(path: str, file: str) -> bool:
    """FILE is PATH, or lies under PATH taken as a directory."""
    path, file = _norm(path), _norm(file)
    return file == path or file.startswith(path.rstrip("/") + "/")


def _bead_text(bead: dict) -> str:
    return "\n".join(str(bead.get(field) or "") for field in BEAD_TEXT_FIELDS)


def _pr_text(pr: dict) -> str:
    return "\n".join(str(pr.get(field) or "") for field in ("title", "body", "headRefName"))


def _who(bead: dict) -> str:
    return bead.get("assignee") or bead.get("owner") or "(no assignee)"


def overlaps(paths: list[str], prs: list[dict], beads: list[dict], skip_branch: Optional[str] = None
             ) -> tuple[list[tuple[dict, list[str]]], list[tuple[dict, list[str]]]]:
    """(PRs touching PATHS with the files they touch, in-progress beads with why)."""
    hit_prs = []
    for pr in prs:
        if skip_branch and pr.get("headRefName") == skip_branch:
            continue
        files = [f["path"] for f in pr.get("files") or [] if any(touches(p, f["path"]) for p in paths)]
        if files:
            hit_prs.append((pr, files))
    cited: dict[str, list[int]] = {}
    for pr, _ in hit_prs:
        for bead_id in sorted(set(BEAD_ID.findall(_pr_text(pr)))):
            cited.setdefault(bead_id, []).append(pr["number"])
    hit_beads = []
    for bead in beads:
        if bead.get("status") != "in_progress":
            continue
        text = _bead_text(bead).replace("\\", "/")
        why = [f"mentions {_norm(p)}" for p in paths if _norm(p).rstrip("/") and _norm(p).rstrip("/") in text]
        why += [f"cited by PR #{n}" for n in cited.get(bead["id"], [])]
        if why:
            hit_beads.append((bead, why))
    return hit_prs, hit_beads


def overlap_main(argv: Optional[list[str]] = None, *, sources: Optional[Sources] = None,
                 out: TextIO = sys.stdout) -> int:
    parser = argparse.ArgumentParser(prog="scripts/dev/overlap-check",
                                     description="Open PRs and in-progress beads that touch these files.")
    parser.add_argument("paths", nargs="*", help="files or directories, relative to the repository root")
    parser.add_argument("--branch", action="store_true", help="use the current branch's changed files")
    parser.add_argument("--base", default="origin/main", help="with --branch: compare against REF")
    parser.add_argument("--repo", type=pathlib.Path, help="the checkout to act for (default: this one)")
    args = parser.parse_args(argv)
    if not args.paths and not args.branch:
        parser.error("name some paths, or pass --branch")
    sources = sources or Sources(_repo(args.repo))
    paths, skip, scope = [_norm(p) for p in args.paths], None, f"{len(args.paths)} path(s)"
    if args.branch:
        skip = sources.current_branch()
        try:
            changed = sources.changed_files(args.base)
        except SourceError as error:
            print(f"overlap-check: could not list the branch's changed files: {error}", file=out)
            changed = []
        paths += [_norm(p) for p in changed]
        scope = f"{len(paths)} path(s): branch {skip or '(detached)'} vs {args.base}"
    print(f"overlap-check: {scope}", file=out)
    try:
        prs = sources.open_prs()
    except SourceError as error:
        print(f"overlap-check: could not list open PRs: {error}", file=out)
        prs = []
    try:
        beads = sources.beads()
    except SourceError as error:
        print(f"overlap-check: could not list in-progress beads: {error}", file=out)
        beads = []
    hit_prs, hit_beads = overlaps(paths, prs, beads, skip) if paths else ([], [])
    if not hit_prs and not hit_beads:
        print(f"overlap-check: no overlap: no open PR ({len(prs)} checked) or in-progress bead touches these paths",
              file=out)
        return 0
    print("OVERLAP", file=out)
    for pr, files in hit_prs:
        print(f"  PR #{pr['number']} {pr.get('headRefName')} — {pr.get('title', '')}", file=out)
        print(f"    Owner: {owner_line(pr.get('body')) or '(none named)'}", file=out)
        for file in files:
            print(f"    {file}", file=out)
    for bead, why in hit_beads:
        print(f"  bead {bead['id']} ({bead.get('status')}) — {bead.get('title', '')}", file=out)
        print(f"    assignee: {_who(bead)}; {'; '.join(why)}", file=out)
    print("END OVERLAP", file=out)
    print("Coordinate with these owners (scripts/dev/who-owns <pr|bead>) before editing the same files.", file=out)
    return 0


# --------------------------------------------------------------------------
# who-owns


def _branch_trees(branch: Optional[str], trees: list[dict]) -> list[dict]:
    return [t for t in trees if branch and t.get("branch") == branch]


def _orca_for(branch: Optional[str], path: Optional[str], orca: Optional[list[dict]], ours: list[dict]) -> list[dict]:
    """Orca workspaces on BRANCH or at PATH, among this repository's worktrees (OURS) only:
    Orca lists every repository's worktrees, and many share a branch name such as `main`."""
    found = []
    for tree in orca or []:
        if not any(_same_path(tree.get("path"), mine["path"]) for mine in ours):
            continue
        same_branch = branch and (tree.get("branch") or "").removeprefix("refs/heads/") == branch
        same_path = path and _same_path(tree.get("path"), path)
        if same_branch or same_path:
            found.append(tree)
    return found


def _same_path(a: Optional[str], b: Optional[str]) -> bool:
    if not a or not b:
        return False
    try:
        return pathlib.Path(a).resolve() == pathlib.Path(b).resolve()
    except OSError:
        return a == b


class _Report:
    def __init__(self, out: TextIO) -> None:
        self.out, self.found = out, False

    def line(self, text: str, found: bool = False) -> None:
        print(text, file=self.out)
        self.found = self.found or found


def _describe_pr(report: _Report, pr: dict) -> None:
    state = pr.get("state") or "?"
    report.line(f"PR #{pr['number']} ({state}) {pr.get('headRefName')} — {pr.get('title', '')}", found=True)
    report.line(f"  owner: {owner_line(pr.get('body')) or '(no Owner: line)'}")
    lane = lane_line(pr.get("body"))
    if lane:
        report.line(f"  lane / coordinator: {lane}")
    if pr.get("url"):
        report.line(f"  url: {pr['url']}")


def _describe_place(report: _Report, branch: Optional[str], path: Optional[str], sources: Sources) -> None:
    trees = sources.worktrees()
    places = _branch_trees(branch, trees) or [t for t in trees if path and _same_path(t["path"], path)]
    for tree in places:
        report.line(f"  worktree: {tree['path']} (branch {tree.get('branch') or 'detached'})", found=True)
    if not places:
        report.line("  worktree: none on this machine")
    orca = sources.orca_worktrees()
    if orca is None:
        report.line("  orca: not available")
        return
    for tree in _orca_for(branch, path, orca, trees):
        status = tree.get("workspaceStatus") or "?"
        report.line(f"  orca: {tree.get('displayName') or '(unnamed)'} ({status}) at {tree.get('path')}", found=True)
        if tree.get("linkedPR"):
            report.line(f"    linked PR: {tree['linkedPR']}")


def _describe_beads(report: _Report, ids: list[str], beads: list[dict]) -> None:
    by_id = {b["id"]: b for b in beads}
    for bead_id in ids:
        bead = by_id.get(bead_id)
        if bead:
            report.line(f"  bead {bead_id} ({bead.get('status')}): assignee {_who(bead)}", found=True)
        else:
            report.line(f"  bead {bead_id}: not in this queue")


def _beads_or_none(report: _Report, sources: Sources) -> list[dict]:
    try:
        return sources.beads()
    except SourceError as error:
        report.line(f"  (could not read the bead queue: {error})")
        return []


def _who_pr(report: _Report, number: int, sources: Sources) -> None:
    try:
        pr = sources.pr(number)
    except SourceError as error:
        report.line(f"could not ask GitHub about PR #{number}: {error}")
        return
    if not pr:
        return
    _describe_pr(report, pr)
    _describe_place(report, pr.get("headRefName"), None, sources)
    ids = sorted(set(BEAD_ID.findall(_pr_text(pr))))
    if ids:
        _describe_beads(report, ids, _beads_or_none(report, sources))


def _who_bead(report: _Report, bead_id: str, sources: Sources) -> None:
    beads = _beads_or_none(report, sources)
    bead = next((b for b in beads if b["id"] == bead_id), None)
    if not bead:
        return
    report.line(f"bead {bead_id} ({bead.get('status')}) — {bead.get('title', '')}", found=True)
    report.line(f"  assignee: {_who(bead)}")
    notes = (bead.get("notes") or "").replace("\r\n", "\n").strip()
    for line in notes.splitlines()[-6:]:
        report.line(f"  notes: {line}")
    try:
        prs = sources.open_prs()
    except SourceError as error:
        report.line(f"  could not ask GitHub for PRs citing it: {error}")
        prs = []
    for pr in prs:
        if bead_id in BEAD_ID.findall(_pr_text(pr)):
            _describe_pr(report, pr)
            _describe_place(report, pr.get("headRefName"), None, sources)
    for branch in sorted({t["branch"] for t in sources.worktrees() if t.get("branch") and bead_id in t["branch"]}):
        report.line(f"  branch: {branch}")
        _describe_place(report, branch, None, sources)


def _who_branch(report: _Report, branch: Optional[str], path: Optional[str], sources: Sources) -> None:
    if path and not branch:
        tree = next((t for t in sources.worktrees() if _same_path(t["path"], path)), None)
        if not tree:
            return
        branch = tree.get("branch")
        report.line(f"worktree {tree['path']}", found=True)
        report.line(f"  branch: {branch or 'detached'}")
        report.line(f"  head: {tree.get('head')}")
    if not branch:
        _describe_place(report, None, path, sources)
        return
    try:
        prs = sources.prs_for_branch(branch)
    except SourceError as error:
        report.line(f"could not ask GitHub for PRs from {branch}: {error}")
        prs = []
    for pr in prs:
        _describe_pr(report, pr)
    ids = sorted({bead_id for pr in prs for bead_id in BEAD_ID.findall(_pr_text(pr))})
    if ids:
        _describe_beads(report, ids, _beads_or_none(report, sources))
    if not path:
        report.line(f"branch {branch}")
    _describe_place(report, branch, path, sources)


def who_owns_main(argv: Optional[list[str]] = None, *, sources: Optional[Sources] = None,
                  out: TextIO = sys.stdout) -> int:
    parser = argparse.ArgumentParser(prog="scripts/dev/who-owns",
                                     description="Who owns a PR, bead, worktree or branch.")
    parser.add_argument("target", help="a PR number, #N or URL; a bead id (brwr-xxx); a worktree path; or a branch")
    parser.add_argument("--repo", type=pathlib.Path, help="the checkout to act for (default: this one)")
    args = parser.parse_args(argv)
    sources = sources or Sources(_repo(args.repo))
    report, target = _Report(out), args.target.strip()
    number = re.fullmatch(r"#?(\d+)", target) or PR_URL.search(target)
    if number:
        _who_pr(report, int(number.group(1)), sources)
    elif BEAD_ID.fullmatch(target):
        _who_bead(report, target, sources)
    elif pathlib.Path(target).expanduser().is_dir():
        _who_branch(report, None, str(pathlib.Path(target).expanduser().resolve()), sources)
    else:
        _who_branch(report, target, None, sources)
    if not report.found:
        print(f"who-owns: nothing owns {target}: no PR, bead, worktree or Orca workspace matches it", file=out)
        return 1
    return 0


def _repo(start: Optional[pathlib.Path]) -> pathlib.Path:
    cwd = start.resolve() if start else pathlib.Path.cwd()
    try:
        return pathlib.Path(_run(["git", "rev-parse", "--show-toplevel"], cwd).strip()).resolve()
    except SourceError:
        return cwd

