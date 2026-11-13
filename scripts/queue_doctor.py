"""`scripts/dev/queue doctor`: a read-only diagnosis of the bead queue.

It changes nothing: no bd write, no pull, no push, no fetch. For each finding
it prints the safe repair for an operator to run. Checks:

  identity    the project_id in the committed .beads/metadata.json, in the
              workspace's metadata.json, and in the database (bd refuses to
              run when the last two differ, printing both).
  cited ids   bead ids cited in docs/backlog/ (frame.md and notes/) that the
              queue does not hold.
  remote      this database's last-pulled refs/dolt/data (Dolt's git remote
              cache) against the remote's (`git ls-remote`, read-only).
  pending     bead edits in this database that are not in its last-pulled
              copy of the remote (`bd diff origin/main main`), i.e. not
              published by `scripts/dev/queue sync` yet.

Exit status: 0 when nothing is a problem (a check that could not run is
printed as unknown), 1 when any check found a problem. Like `queue check`,
these are not bd sync's exit codes (worker-contract.json
queue.sync_exit_codes covers `queue sync` only).
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import subprocess
import sys
from dataclasses import dataclass
from typing import Optional, TextIO

import work_queue

BEAD_ID = re.compile(rf"\b{work_queue.PREFIX}-[a-z0-9]{{2,}}(?:\.[0-9]+)*\b")
MISMATCH_FILE = re.compile(r"metadata\.json project_id:\s*(\S+)")
MISMATCH_DB = re.compile(r"database _project_id:\s*(\S+)")
REMOTE_CACHE_REFS = "refs/dolt/remotes/origin/dolt/data"
READ_ONLY_GIT = {"GIT_OPTIONAL_LOCKS": "0", "GIT_TERMINAL_PROMPT": "0"}


class ProbeError(Exception):
    """A check's source could not answer; the check is reported as unknown."""


@dataclass
class Finding:
    check: str
    state: str  # "ok", "problem" or "unknown"
    detail: str
    repair: Optional[str] = None


def parse_mismatch(text: str) -> Optional[tuple[str, str]]:
    """(metadata.json id, database id) from bd's identity-mismatch error, or None."""
    text = text.replace("\r\n", "\n")
    file_id, db_id = MISMATCH_FILE.search(text), MISMATCH_DB.search(text)
    return (file_id.group(1), db_id.group(1)) if file_id and db_id else None


def parse_diff(text: str) -> list[dict]:
    """`bd diff --json` output as rows; bd prints a plain "No changes" line when there are none."""
    text = text.replace("\r\n", "\n").strip()
    if not text or text.startswith("No changes"):
        return []
    try:
        rows = json.loads(text)
    except ValueError as error:
        raise ProbeError(f"bd diff printed neither JSON nor 'No changes': {text[:120]}") from error
    return rows if isinstance(rows, list) else []


def cited_ids(repo: pathlib.Path) -> dict[str, list[str]]:
    """Bead id -> ["docs/backlog/<file>:<line>", ...] for every id cited in the backlog prose."""
    rb, _ = work_queue._modules()
    files = [repo / rb.FRAME_PATH, *sorted((repo / rb.NOTES_DIR).glob("*.md"))]
    found: dict[str, list[str]] = {}
    for path in files:
        if not path.is_file():
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").replace("\r\n", "\n").splitlines(), 1):
            for bead_id in BEAD_ID.findall(line):
                found.setdefault(bead_id, []).append(f"{path.relative_to(repo).as_posix()}:{number}")
    return found


def _git(*args: str, cwd: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8",
                          stdin=subprocess.DEVNULL, timeout=60, env={**os.environ, **READ_ONLY_GIT})


class Probe:
    """The live sources, read-only. Tests replace every method."""

    def __init__(self, repo: pathlib.Path) -> None:
        self.repo = repo
        self.ws = work_queue.workspace(repo)

    def committed_project_id(self) -> Optional[str]:
        shown = _git("show", "HEAD:.beads/metadata.json", cwd=self.repo)
        if shown.returncode != 0:
            return None
        return json.loads(shown.stdout).get("project_id")

    def context(self) -> tuple[Optional[dict], str]:
        result = work_queue.bd("context", "--json", cwd=self.ws, check=False)
        if result.returncode != 0:
            return None, (result.stderr or result.stdout)
        try:
            return json.loads(result.stdout), ""
        except ValueError:
            return None, result.stdout

    def workspace_project_id(self, beads_dir: Optional[str]) -> Optional[str]:
        path = pathlib.Path(beads_dir or self.ws / ".beads") / "metadata.json"
        try:
            return json.loads(path.read_text(encoding="utf-8")).get("project_id")
        except (OSError, ValueError):
            return None

    def queue_ids(self) -> set[str]:
        import tempfile  # noqa: PLC0415

        with tempfile.TemporaryDirectory(prefix="brauser-doctor-") as scratch:
            try:
                path = work_queue.export(self.repo, pathlib.Path(scratch) / "issues.jsonl")
            except work_queue.QueueError as error:
                raise ProbeError(str(error)) from error
            return {json.loads(line)["id"] for line in path.read_text(encoding="utf-8").splitlines() if line.strip()}

    def cached_remote_sha(self, context: Optional[dict]) -> Optional[str]:
        """The remote refs/dolt/data this database last pulled or pushed, from Dolt's git remote cache."""
        beads_dir = pathlib.Path((context or {}).get("beads_dir") or self.ws / ".beads")
        database = (context or {}).get("database") or work_queue.PREFIX
        caches = sorted((beads_dir / "embeddeddolt" / database / ".dolt" / "git-remote-cache").glob("*/repo.git"))
        shas = []
        for cache in caches:
            refs = _git("--git-dir", str(cache), "for-each-ref", "--sort=-committerdate", "--format=%(objectname)",
                        REMOTE_CACHE_REFS, cwd=self.repo)
            if refs.returncode != 0:
                raise ProbeError(f"could not read {cache}: {refs.stderr.strip()}")
            shas += refs.stdout.split()
        return shas[0] if shas else None

    def remote_sha(self) -> Optional[str]:
        url = work_queue.dolt_remote(self.ws) or _git("remote", "get-url", "origin", cwd=self.repo).stdout.strip()
        listed = _git("ls-remote", url, work_queue.DOLT_REF, cwd=self.repo)
        if listed.returncode != 0:
            raise ProbeError(f"git ls-remote {url} failed: {listed.stderr.strip()[:200]}")
        line = listed.stdout.strip()
        return line.split()[0] if line else None

    def pending(self) -> list[dict]:
        result = work_queue.bd("diff", "origin/main", "main", "--json", cwd=self.ws, check=False)
        if result.returncode != 0:
            raise ProbeError(f"bd diff origin/main main failed: {(result.stderr or result.stdout).strip()[:200]}")
        return parse_diff(result.stdout)


def _identity(probe: Probe) -> tuple[Finding, Optional[dict]]:
    committed = probe.committed_project_id()
    context, error = probe.context()
    mismatch = parse_mismatch(error) if error else None
    if mismatch:
        file_id, db_id = mismatch
        return Finding(
            "identity", "problem",
            f"bd refuses this workspace: metadata.json project_id {file_id}, database _project_id {db_id} "
            f"(committed: {committed or 'none'})",
            "if the committed id is right, the database was built from another seed: move the workspace's "
            ".beads/embeddeddolt aside (do not delete it) and run `scripts/dev/queue bootstrap`, which "
            "clones the published queue; first run `scripts/dev/queue doctor` elsewhere to confirm the remote "
            "is healthy, and ask the coordinator before changing the committed .beads/metadata.json",
        ), None
    if context is None:
        return Finding("identity", "unknown", f"bd context failed: {error.strip()[:200]}"), None
    workspace = probe.workspace_project_id(context.get("beads_dir"))
    if committed and workspace and workspace != committed:
        return Finding(
            "identity", "problem",
            f"the workspace metadata.json says {workspace}, the committed .beads/metadata.json says {committed}",
            "restore the committed file in the workspace (`git restore --source origin/main .beads/metadata.json`) "
            "only after confirming the database's _project_id is the committed one; otherwise re-bootstrap as above",
        ), context
    return Finding("identity", "ok", f"project_id {context.get('project_id') or workspace or committed}"), context


def _cited(repo: pathlib.Path, probe: Probe) -> Finding:
    try:
        held = probe.queue_ids()
    except ProbeError as error:
        return Finding("cited ids", "unknown", str(error))
    cited = cited_ids(repo)
    missing = {bead_id: where for bead_id, where in cited.items() if bead_id not in held}
    if not missing:
        return Finding("cited ids", "ok", f"all {len(cited)} bead ids cited in docs/backlog are in the queue")
    listed = ", ".join(f"{bead_id} ({where[0]})" for bead_id, where in sorted(missing.items()))
    return Finding(
        "cited ids", "problem", f"{len(missing)} bead id(s) cited in docs/backlog are not in the queue: {listed}",
        "if the bead was created elsewhere, `scripts/dev/queue sync` pulls it; if it was never created or was "
        "deleted, fix the citation in a docs PR (or recreate it with scripts/dev/create-bead)",
    )


def _remote(probe: Probe, context: Optional[dict]) -> Finding:
    try:
        cached = probe.cached_remote_sha(context)
        remote = probe.remote_sha()
    except (ProbeError, OSError, subprocess.SubprocessError, work_queue.QueueError) as error:
        return Finding("remote", "unknown", f"could not compare with the remote: {error}")
    if remote is None:
        return Finding("remote", "problem", f"the remote has no {work_queue.DOLT_REF}: the queue was never seeded there",
                       "seeding is a one-time coordinated step (`scripts/dev/queue seed`); ask the coordinator")
    if cached is None:
        return Finding("remote", "unknown", f"this database has no cached copy of the remote (remote at {remote[:12]})",
                       "`scripts/dev/queue sync` pulls it; a fresh clone runs `scripts/dev/queue bootstrap`")
    if cached != remote:
        return Finding("remote", "problem",
                       f"the remote moved since this database last pulled ({cached[:12]} -> {remote[:12]})",
                       "`scripts/dev/queue sync` pulls and merges it (a halt with exit 2 is a conflict for an "
                       "operator: `bd conflicts`)")
    return Finding("remote", "ok", f"in step with the remote at {remote[:12]}")


def _pending(probe: Probe) -> Finding:
    try:
        rows = probe.pending()
    except (ProbeError, OSError, subprocess.SubprocessError, work_queue.QueueError) as error:
        return Finding("pending", "unknown", str(error))
    if not rows:
        return Finding("pending", "ok", "no bead edits waiting to be published")
    listed = ", ".join(f"{r.get('IssueID')} ({r.get('DiffType')})" for r in rows[:10])
    more = f" and {len(rows) - 10} more" if len(rows) > 10 else ""
    return Finding("pending", "problem",
                   f"{len(rows)} bead edit(s) are in this database but not published: {listed}{more}",
                   "`scripts/dev/queue sync` publishes them (another session's edit in a shared database is "
                   "published by whichever session syncs next)")


def doctor(repo: pathlib.Path, probe: Optional[Probe] = None, out: Optional[TextIO] = None) -> int:
    out = out or sys.stdout
    probe = probe or Probe(repo)
    print("queue doctor (read-only: nothing was changed, pulled or pushed)", file=out)
    identity, context = _identity(probe)
    findings = [identity, _cited(repo, probe), _remote(probe, context), _pending(probe)]
    for finding in findings:
        label = "PROBLEM" if finding.state == "problem" else finding.state
        print(f"  [{label}] {finding.check}: {finding.detail}", file=out)
        if finding.repair and finding.state != "ok":
            print(f"      repair: {finding.repair}", file=out)
    problems = sum(f.state == "problem" for f in findings)
    print(f"queue doctor: {problems} problem(s)" if problems else "queue doctor: no problems", file=out)
    return 1 if problems else 0
