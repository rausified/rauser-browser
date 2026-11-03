"""Tests for scripts/work_queue.py (scripts/dev/queue): the queue on the beads branch.

Every test uses a scratch bare origin with a `main` branch and an orphan
`beads` branch holding the queue; nothing touches the real repository.
"""

from __future__ import annotations

import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import work_queue  # noqa: E402
from test_render_backlog import SKELETON, bead, jsonl  # noqa: E402

IDENTITY = {
    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
    "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
}
ATTRIBUTES = ".beads/issues.jsonl merge=beads-jsonl\nBOSUN_FEATURE_BACKLOG.md merge=bosun-backlog\n"
GATE = bead("brwr-gate", status="deferred")


def git(cwd: pathlib.Path, *args: str, check: bool = True) -> str:
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True,
                            env={**os.environ, **IDENTITY})
    if check and result.returncode:
        raise AssertionError(f"git {' '.join(args)}: {result.stderr}")
    return result.stdout.strip()


def queue_text(*records: dict[str, object]) -> str:
    return jsonl(GATE, *records)


def rendered(*records: dict[str, object]) -> str:
    import render_backlog

    return render_backlog.render(SKELETON, render_backlog.parse_issues(queue_text(*records), "q"))


@unittest.skipIf(sys.platform == "win32", "scripts/dev/queue is a macOS/Linux dev tool (it locks with fcntl)")
class QueueFixture(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = pathlib.Path(tmp.name).resolve()
        environ = unittest.mock.patch.dict(os.environ, IDENTITY)
        environ.start()
        self.addCleanup(environ.stop)
        os.environ.pop(work_queue.DIR_ENV, None)

        self.origin = self.tmp / "origin.git"
        git(self.tmp, "init", "-q", "--bare", "-b", "main", str(self.origin))
        seed = self.tmp / "seed"
        git(self.tmp, "init", "-q", "-b", "main", str(seed))
        (seed / ".beads").mkdir()
        # Main as it is after the move: the queue's history is in base only.
        (seed / ".beads" / "issues.jsonl").write_text(queue_text(bead("brwr-a1")), encoding="utf-8")
        (seed / "BOSUN_FEATURE_BACKLOG.md").write_text(rendered(bead("brwr-a1")), encoding="utf-8")
        git(seed, "add", ".")
        git(seed, "commit", "-qm", "main with the queue")
        self.base = git(seed, "rev-parse", "HEAD")
        git(seed, "rm", "-q", ".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md")
        (seed / "README").write_text("main\n", encoding="utf-8")
        git(seed, "add", "README")
        git(seed, "commit", "-qm", "queue moved to beads")
        git(seed, "push", "-q", str(self.origin), "main")
        git(seed, "checkout", "-q", "--orphan", "beads")
        git(seed, "rm", "-rqf", ".")
        (seed / ".beads").mkdir(exist_ok=True)
        (seed / ".beads" / "issues.jsonl").write_text(queue_text(bead("brwr-a1")), encoding="utf-8")
        (seed / "BOSUN_FEATURE_BACKLOG.md").write_text(rendered(bead("brwr-a1")), encoding="utf-8")
        (seed / ".gitattributes").write_text(ATTRIBUTES, encoding="utf-8")
        git(seed, "add", ".")
        git(seed, "commit", "-qm", "beads")
        git(seed, "push", "-q", str(self.origin), "beads")
        self.repo = self.clone("repo")

    def clone(self, name: str) -> pathlib.Path:
        path = self.tmp / name
        git(self.tmp, "clone", "-q", str(self.origin), str(path))
        return path

    def queue(self, repo: pathlib.Path, *argv: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(SCRIPTS / "dev" / "queue"), "--repo", str(repo), *argv],
            capture_output=True, text=True,
        )

    def ok(self, repo: pathlib.Path, *argv: str) -> str:
        result = self.queue(repo, *argv)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def origin_file(self, file: str) -> str:
        return git(self.origin, "show", f"beads:{file}") + "\n"

    def origin_ids(self) -> list[str]:
        return [json.loads(line)["id"] for line in self.origin_file(work_queue.QUEUE_PATH).splitlines() if line]

    def origin_status(self, issue_id: str) -> str:
        rows = [json.loads(line) for line in self.origin_file(work_queue.QUEUE_PATH).splitlines() if line]
        return next(row["status"] for row in rows if row["id"] == issue_id)

    def edit(self, path: pathlib.Path, *records: dict[str, object]) -> None:
        (path / work_queue.QUEUE_PATH).write_text(queue_text(*records), encoding="utf-8")


class PathTests(QueueFixture):
    def test_path_is_one_shared_worktree_beside_the_clone(self) -> None:
        path = pathlib.Path(self.ok(self.repo, "path").strip())
        self.assertEqual(path, self.tmp / "repo-queue-beads")
        self.assertEqual(git(path, "symbolic-ref", "HEAD"), "refs/heads/beads")
        self.assertTrue((path / work_queue.QUEUE_PATH).is_file())
        # A linked worktree of the clone finds the same directory.
        linked = self.tmp / "linked"
        git(self.repo, "worktree", "add", "-q", "-b", "feat/x", str(linked), "origin/main")
        self.assertEqual(pathlib.Path(self.ok(linked, "path").strip()), path)

    def test_concurrent_first_calls_create_it_once(self) -> None:
        procs = [
            subprocess.Popen([sys.executable, str(SCRIPTS / "dev" / "queue"), "--repo", str(self.repo), "path"],
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            for _ in range(4)
        ]
        outputs = [proc.communicate() for proc in procs]
        self.assertEqual([proc.returncode for proc in procs], [0] * 4, outputs)
        self.assertEqual({out.strip() for out, _ in outputs}, {str(self.tmp / "repo-queue-beads")})
        worktrees = git(self.repo, "worktree", "list", "--porcelain").count("branch refs/heads/beads")
        self.assertEqual(worktrees, 1)

    def test_a_foreign_directory_at_the_path_is_refused(self) -> None:
        (self.tmp / "repo-queue-beads").mkdir()
        result = self.queue(self.repo, "path")
        self.assertEqual(result.returncode, 1)
        self.assertIn("is not this clone's worktree", result.stderr)

    def test_the_environment_override_wins(self) -> None:
        with unittest.mock.patch.dict(os.environ, {work_queue.DIR_ENV: str(self.tmp / "elsewhere")}):
            self.assertEqual(pathlib.Path(self.ok(self.repo, "path").strip()), self.tmp / "elsewhere")


class PullPushTests(QueueFixture):
    def test_push_renders_checks_commits_and_pushes(self) -> None:
        path = pathlib.Path(self.ok(self.repo, "path").strip())
        self.edit(path, bead("brwr-a1"), bead("brwr-b2", metadata={"backlog_section": "plan"}))
        out = self.ok(self.repo, "push", "-m", "queue: add brwr-b2")
        self.assertIn("pushed beads at", out)
        self.assertEqual(sorted(self.origin_ids()), ["brwr-a1", "brwr-b2", "brwr-gate"])
        backlog = self.origin_file(work_queue.BACKLOG_PATH)
        self.assertIn("`brwr-b2`", backlog)
        self.assertEqual(git(self.origin, "log", "-1", "--format=%s", "beads"), "queue: add brwr-b2")
        self.assertIn("nothing to push", self.ok(self.repo, "push"))

    def test_a_failing_queue_check_commits_nothing(self) -> None:
        path = pathlib.Path(self.ok(self.repo, "path").strip())
        before = git(self.origin, "rev-parse", "beads")
        self.edit(path, bead("brwr-a1", dependencies=[{"depends_on_id": "brwr-zz9"}]))
        result = self.queue(self.repo, "push")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown-dependency", result.stderr)
        self.assertEqual(git(self.origin, "rev-parse", "beads"), before)
        self.assertEqual(git(path, "rev-list", "--count", "origin/beads..HEAD"), "0")

    def test_pull_fast_forwards_and_keeps_local_edits(self) -> None:
        other = self.clone("other")
        mine = pathlib.Path(self.ok(self.repo, "path").strip())
        theirs = pathlib.Path(self.ok(other, "path").strip())
        self.edit(theirs, bead("brwr-a1"), bead("brwr-t1"))
        self.ok(other, "push")
        self.ok(self.repo, "pull")
        self.assertIn("brwr-t1", (mine / work_queue.QUEUE_PATH).read_text(encoding="utf-8"))
        # An uncommitted edit survives the next pull (rebase --autostash).
        self.edit(theirs, bead("brwr-a1"), bead("brwr-t1"), bead("brwr-t2"))
        self.ok(other, "push")
        self.edit(mine, bead("brwr-a1"), bead("brwr-t1"), bead("brwr-m1"))
        self.ok(self.repo, "pull")
        text = (mine / work_queue.QUEUE_PATH).read_text(encoding="utf-8")
        self.assertIn("brwr-t2", text)
        self.assertIn("brwr-m1", text)

    def test_a_push_race_is_replayed_bead_by_bead_without_force(self) -> None:
        other = self.clone("other")
        mine = pathlib.Path(self.ok(self.repo, "path").strip())
        theirs = pathlib.Path(self.ok(other, "path").strip())
        self.edit(theirs, bead("brwr-a1"), bead("brwr-t1", metadata={"backlog_section": "plan"}))
        self.ok(other, "push", "-m", "theirs")
        first = git(self.origin, "rev-parse", "beads")
        # Ours never saw theirs: the push is rejected, rebased, and retried.
        self.edit(mine, bead("brwr-a1", status="closed"), bead("brwr-m1", metadata={"backlog_section": "plan"}))
        self.ok(self.repo, "push", "-m", "ours")
        self.assertEqual(sorted(self.origin_ids()), ["brwr-a1", "brwr-gate", "brwr-m1", "brwr-t1"])
        self.assertEqual(self.origin_status("brwr-a1"), "closed")
        git(self.origin, "merge-base", "--is-ancestor", first, "beads")
        backlog = self.origin_file(work_queue.BACKLOG_PATH)
        self.assertIn("`brwr-m1`", backlog)
        self.assertIn("`brwr-t1`", backlog)
        git(mine, "fetch", "-q", "origin")
        check = subprocess.run(
            [sys.executable, str(SCRIPTS / "check_queue_consistency.py"), "--ref", "origin/beads"],
            cwd=mine, capture_output=True, text=True,
        )
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)


class AdoptTests(QueueFixture):
    """A PR branch cut before the move still carries queue edits."""

    def pr_branch(self, *records: dict[str, object], prose: str | None = None) -> str:
        git(self.repo, "checkout", "-q", "-b", "feat/old", self.base)
        (self.repo / work_queue.QUEUE_PATH).write_text(queue_text(*records), encoding="utf-8")
        backlog = rendered(*records)
        if prose:
            backlog = backlog.replace("Plan intro.", prose)
        (self.repo / work_queue.BACKLOG_PATH).write_text(backlog, encoding="utf-8")
        git(self.repo, "commit", "-qam", "pr edits the queue")
        return git(self.repo, "rev-parse", "HEAD")

    def test_adopt_carries_bead_and_prose_edits_onto_beads(self) -> None:
        # Beads moved on meanwhile (another bead added there).
        other = self.clone("other")
        theirs = pathlib.Path(self.ok(other, "path").strip())
        self.edit(theirs, bead("brwr-a1"), bead("brwr-q1"))
        self.ok(other, "push")
        head = self.pr_branch(bead("brwr-a1", status="closed"), bead("brwr-p1"), prose="Plan intro, from the PR.")
        self.ok(self.repo, "adopt", "--from", head)
        self.assertEqual(sorted(self.origin_ids()), ["brwr-a1", "brwr-gate", "brwr-p1", "brwr-q1"])
        self.assertEqual(self.origin_status("brwr-a1"), "closed")
        self.assertIn("Plan intro, from the PR.", self.origin_file(work_queue.BACKLOG_PATH))
        # Adopting again changes nothing.
        before = git(self.origin, "rev-parse", "beads")
        self.ok(self.repo, "adopt", "--from", head)
        self.assertEqual(git(self.origin, "rev-parse", "beads"), before)

    def test_dry_run_pushes_nothing(self) -> None:
        head = self.pr_branch(bead("brwr-a1"), bead("brwr-p1"))
        before = git(self.origin, "rev-parse", "beads")
        self.assertIn("apply cleanly", self.ok(self.repo, "adopt", "--from", head, "--dry-run"))
        self.assertEqual(git(self.origin, "rev-parse", "beads"), before)

    def test_a_conflicting_edit_is_refused_and_writes_nothing(self) -> None:
        other = self.clone("other")
        theirs = pathlib.Path(self.ok(other, "path").strip())
        self.edit(theirs, bead("brwr-a1", status="blocked"))
        self.ok(other, "push")
        head = self.pr_branch(bead("brwr-a1", status="closed"))
        before = git(self.origin, "rev-parse", "beads")
        result = self.queue(self.repo, "adopt", "--from", head)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("brwr-a1", result.stderr)
        self.assertEqual(git(self.origin, "rev-parse", "beads"), before)
        mine = self.tmp / "repo-queue-beads"
        self.assertEqual(git(mine, "status", "--porcelain"), "")

    def test_a_branch_without_queue_edits_has_nothing_to_adopt(self) -> None:
        git(self.repo, "checkout", "-q", "-b", "feat/plain", self.base)
        self.assertIn("did not change the queue files", self.ok(self.repo, "adopt", "--from", "HEAD"))


class UpsertTests(unittest.TestCase):
    def test_rows_replace_or_insert_in_export_order(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "issues.jsonl"
            a, b = bead("brwr-a1", priority=1), bead("brwr-b2", priority=3)
            path.write_text(jsonl(a, b), encoding="utf-8")
            new = json.dumps(bead("brwr-c3", priority=2))
            changed = work_queue.upsert_rows(path, [json.dumps(dict(a, status="closed")), new])
            self.assertEqual(changed, ["brwr-a1", "brwr-c3"])
            ids = [json.loads(line)["id"] for line in path.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(ids, ["brwr-a1", "brwr-c3", "brwr-b2"])
            self.assertEqual(work_queue.upsert_rows(path, [new]), [])


if __name__ == "__main__":
    unittest.main()
