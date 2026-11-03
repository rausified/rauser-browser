"""Tests for scripts/work_queue.py (scripts/dev/queue): the queue in bd's Dolt database.

The unit tests need nothing installed. The integration tests run the real bd
against a scratch bare git remote, the way refs/dolt/data works on GitHub,
and are skipped where bd is not installed (CI's build jobs). Nothing touches
the real repository or any real database.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import pathlib
import shutil
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
GATE = bead("brwr-gate", status="deferred")
COMMITTED_METADATA = {"database": "dolt", "backend": "dolt", "dolt_mode": "embedded", "dolt_database": "brwr",
                      "project_id": "11111111-2222-3333-4444-555555555555"}


def rows(text: str) -> dict[str, dict]:
    return {json.loads(line)["id"]: json.loads(line) for line in text.splitlines() if line.strip()}


class PlanAdoptionTests(unittest.TestCase):
    """Field-level application of a branch's committed edits (no bd needed)."""

    def plan(self, base, current, theirs):
        return work_queue.plan_adoption(jsonl(*base), jsonl(*current), jsonl(*theirs))

    def test_only_the_branchs_changed_fields_are_applied(self) -> None:
        base = bead("brwr-a1", updated_at="2026-11-01T00:00:00Z")
        current = dict(base, priority=0, updated_at="2026-11-02T00:00:00Z")  # changed here since
        theirs = dict(base, status="closed", updated_at="2026-11-01T12:00:00Z")  # changed on the branch
        imports, labels, deps, predicted = self.plan([base], [current], [theirs])
        self.assertEqual([(r["id"], r["status"], r["priority"]) for r in imports], [("brwr-a1", "closed", 0)])
        self.assertGreater(imports[0]["updated_at"], current["updated_at"])
        self.assertEqual((labels, deps), ({}, {}))
        self.assertEqual(rows(predicted)["brwr-a1"]["status"], "closed")

    def test_new_beads_are_added_and_unchanged_ones_are_left_alone(self) -> None:
        a = bead("brwr-a1")
        imports, _, _, _ = self.plan([a], [a, bead("brwr-db1")], [a, bead("brwr-p1")])
        self.assertEqual([r["id"] for r in imports], ["brwr-p1"])

    def test_removed_labels_and_dependencies_are_removed_explicitly(self) -> None:
        # bd import only adds labels and edges, so removals are separate bd calls.
        base = bead("brwr-a1", labels=["x", "y"], dependencies=[{"depends_on_id": "brwr-b2", "type": "blocks"}])
        theirs = dict(base, labels=["x"], dependencies=[])
        _, labels, deps, _ = self.plan([base, bead("brwr-b2")], [base, bead("brwr-b2")], [theirs, bead("brwr-b2")])
        self.assertEqual(labels, {"brwr-a1": ["y"]})
        self.assertEqual(deps, {"brwr-a1": [("brwr-b2", "blocks")]})

    def test_a_field_both_sides_changed_differently_refuses_even_when_timestamps_order_them(self) -> None:
        # Codex P1 on #274: merge_jsonl would take the newer side, turning the
        # database's `blocked` into the branch's `closed` without a word.
        base = bead("brwr-a1", updated_at="2026-11-01T00:00:00Z")
        current = dict(base, status="blocked", updated_at="2026-11-02T00:00:00Z")
        theirs = dict(base, status="closed", updated_at="2026-11-03T00:00:00Z")
        with self.assertRaisesRegex(work_queue.QueueError, r"brwr-a1 status: database 'blocked', branch 'closed'"):
            self.plan([base], [current], [theirs])

    def test_a_metadata_key_both_sides_changed_differently_refuses(self) -> None:
        base = bead("brwr-a1", metadata={"backlog_section": "plan", "backlog_order": 1})
        current = dict(base, metadata={"backlog_section": "quality", "backlog_order": 1})
        theirs = dict(base, metadata={"backlog_section": "gated", "backlog_order": 2})
        with self.assertRaisesRegex(work_queue.QueueError, r"brwr-a1 metadata\.backlog_section"):
            self.plan([base], [current], [theirs])

    def test_the_same_change_on_both_sides_is_no_conflict(self) -> None:
        base = bead("brwr-a1")
        imports, _, _, _ = self.plan([base], [dict(base, status="closed")], [dict(base, status="closed")])
        self.assertEqual(imports, [])

    def test_adopted_rows_are_stamped_after_their_current_updated_at(self) -> None:
        # Codex P2: a stamp equal to the row's own second lets bd import keep the old fields.
        future = "2999-01-01T00:00:00Z"
        base = bead("brwr-a1", updated_at="2026-11-01T00:00:00Z")
        current = dict(base, updated_at=future)
        imports, _, _, _ = self.plan([base], [current], [dict(base, title="New")])
        self.assertEqual(imports[0]["updated_at"], "2999-01-01T00:00:01Z")

    def test_a_dependency_type_change_removes_the_old_edge(self) -> None:
        base = bead("brwr-a1", dependencies=[{"depends_on_id": "brwr-b2", "type": "blocks"}])
        theirs = dict(base, dependencies=[{"depends_on_id": "brwr-b2", "type": "related"}])
        imports, _, deps, _ = self.plan([base, bead("brwr-b2")], [base, bead("brwr-b2")], [theirs, bead("brwr-b2")])
        self.assertEqual(deps, {"brwr-a1": [("brwr-b2", "blocks")]})
        self.assertEqual(imports[0]["dependencies"], [{"issue_id": "brwr-a1", "depends_on_id": "brwr-b2", "type": "related"}])

    def test_a_field_both_changed_without_order_refuses_everything(self) -> None:
        a = bead("brwr-a1")
        with self.assertRaisesRegex(work_queue.QueueError, "brwr-a1"):
            self.plan([a], [dict(a, status="blocked")], [dict(a, status="closed")])

    def test_a_bead_the_branch_deleted_refuses_everything(self) -> None:
        a, b = bead("brwr-a1"), bead("brwr-b2")
        with self.assertRaisesRegex(work_queue.QueueError, "deletes bead.*brwr-b2"):
            self.plan([a, b], [a, b], [a])


class SeedInputTests(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = pathlib.Path(tmp.name)

    def file(self, name: str, text: str) -> pathlib.Path:
        path = self.tmp / name
        path.write_text(text, encoding="utf-8")
        return path

    def test_extra_rows_are_added_and_an_identical_duplicate_is_fine(self) -> None:
        main = self.file("main.jsonl", jsonl(GATE, bead("brwr-a1")))
        extra = self.file("extra.jsonl", jsonl(bead("brwr-9eio", status="closed"), GATE))
        self.assertEqual(sorted(rows(work_queue.combine(main, [extra]))), ["brwr-9eio", "brwr-a1", "brwr-gate"])

    def test_a_conflicting_duplicate_or_bad_extra_is_refused(self) -> None:
        main = self.file("main.jsonl", jsonl(GATE, bead("brwr-a1")))
        for name, text, message in (
            ("dup.jsonl", jsonl(bead("brwr-a1", title="other")), "different fields"),
            ("bad.jsonl", "not json\n", "not valid JSON"),
            ("prefix.jsonl", jsonl(bead("xyz-1")), "prefix"),
            ("memory.jsonl", json.dumps({"_type": "memory", "id": "brwr-m1"}) + "\n", "not an issue"),
        ):
            with self.subTest(name=name), self.assertRaisesRegex(work_queue.QueueError, message):
                work_queue.combine(main, [self.file(name, text)])

    def test_the_committed_9eio_extra_row_is_valid(self) -> None:
        path = SCRIPTS / "dev" / "queue-seed" / "brwr-9eio.jsonl"
        self.assertEqual(list(work_queue.validated_rows(path)), ["brwr-9eio"])
        self.assertEqual(rows(path.read_text(encoding="utf-8"))["brwr-9eio"]["status"], "closed")


@unittest.skipUnless(shutil.which("bd") and sys.platform != "win32", "needs bd (beads) installed")
class DoltIntegrationTests(unittest.TestCase):
    """Real bd and Dolt against a scratch bare remote, like refs/dolt/data on GitHub."""

    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = pathlib.Path(tmp.name).resolve()
        environ = unittest.mock.patch.dict(os.environ, {**IDENTITY, "BD_NON_INTERACTIVE": "1"})
        environ.start()
        self.addCleanup(environ.stop)
        os.environ.pop(work_queue.WORKSPACE_ENV, None)
        self.remote = self.tmp / "remote.git"
        self.url = self.remote.as_uri()
        self.git(self.tmp, "init", "-q", "--bare", "-b", "main", str(self.remote))
        seed = self.tmp / "seedrepo"
        self.git(self.tmp, "init", "-q", "-b", "main", str(seed))
        (seed / "docs" / "backlog").mkdir(parents=True)
        (seed / "docs" / "backlog" / "frame.md").write_text(SKELETON, encoding="utf-8")
        # Main commits bd's metadata.json, project_id included (brwr-765q).
        (seed / ".beads").mkdir()
        (seed / ".beads" / "metadata.json").write_text(json.dumps(COMMITTED_METADATA, indent=2), encoding="utf-8")
        self.git(seed, "add", ".")
        self.git(seed, "commit", "-qm", "frame")
        self.git(seed, "push", "-q", self.url, "main")
        self.a = self.clone("a")

    def git(self, cwd, *args):
        result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout.strip()

    def bd(self, cwd, *args):
        result = subprocess.run(["bd", *args], cwd=cwd, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def clone(self, name):
        path = self.tmp / name
        self.git(self.tmp, "clone", "-q", self.url, str(path))
        return path

    def queue(self, repo, *argv):
        return subprocess.run([sys.executable, str(SCRIPTS / "dev" / "queue"), "--repo", str(repo), *argv],
                              capture_output=True, text=True)

    def ok(self, repo, *argv):
        result = self.queue(repo, *argv)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def seed(self, *records, extra=None):
        main = self.tmp / "seed.jsonl"
        main.write_text(jsonl(GATE, *records), encoding="utf-8")
        argv = ["seed", "--remote", self.url, "--from", str(main), "--yes"]
        if extra:
            path = self.tmp / "extra.jsonl"
            path.write_text(jsonl(*extra), encoding="utf-8")
            argv += ["--extra", str(path)]
        return self.ok(self.a, *argv)

    def bootstrap(self, repo):
        self.bd(repo, "bootstrap")
        return repo

    def exported(self, repo):
        return rows(self.ok(repo, "export"))

    def dolt_head(self):
        return self.git(self.remote, "for-each-ref", "--format=%(objectname)", work_queue.DOLT_REF)

    def test_seed_then_bootstrap_gives_every_row_and_seeding_twice_is_refused(self) -> None:
        self.seed(bead("brwr-a1"), extra=[bead("brwr-9eio", status="closed")])
        self.assertTrue(self.dolt_head())
        b = self.bootstrap(self.clone("b"))
        self.assertEqual(sorted(self.exported(b)), ["brwr-9eio", "brwr-a1", "brwr-gate"])
        again = self.queue(self.a, "seed", "--remote", self.url, "--from", str(self.tmp / "seed.jsonl"), "--yes")
        self.assertEqual(again.returncode, 1)
        self.assertIn("one-shot", again.stderr)

    def test_seed_refuses_a_queue_that_fails_the_check_and_pushes_nothing(self) -> None:
        main = self.tmp / "seed.jsonl"
        main.write_text(jsonl(GATE, bead("brwr-a1", dependencies=[{"depends_on_id": "brwr-zz9"}])), encoding="utf-8")
        result = self.queue(self.a, "seed", "--remote", self.url, "--from", str(main), "--yes")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown-dependency", result.stderr)
        self.assertEqual(self.dolt_head(), "")

    def test_sync_refuses_a_remote_that_was_never_seeded(self) -> None:
        self.bd(self.a, "init", "--prefix", "brwr", "--non-interactive", "--skip-hooks", "--skip-agents")
        result = self.queue(self.a, "sync")
        self.assertEqual(result.returncode, 1)
        self.assertIn("never seeded", result.stderr)
        self.assertEqual(self.dolt_head(), "")

    def test_two_clones_edit_different_fields_of_one_bead_and_both_survive(self) -> None:
        self.seed(bead("brwr-a1"))
        a, b = self.bootstrap(self.a), self.bootstrap(self.clone("b"))
        self.bd(a, "update", "brwr-a1", "--title", "Retitled in a")
        self.bd(b, "update", "brwr-a1", "--priority", "0")
        self.ok(a, "sync")
        self.ok(b, "sync")
        self.ok(a, "sync")
        for repo in (a, b):
            got = self.exported(repo)["brwr-a1"]
            self.assertEqual((got["title"], got["priority"]), ("Retitled in a", 0), repo)

    def test_a_queue_that_fails_the_check_is_not_pushed(self) -> None:
        self.seed(bead("brwr-a1"))
        a = self.bootstrap(self.a)
        before = self.dolt_head()
        self.bd(a, "update", "brwr-a1", "--set-metadata", "backlog_section=nowhere")
        result = self.queue(a, "sync")
        self.assertEqual(result.returncode, 1)
        self.assertIn("unknown-backlog-section", result.stderr)
        self.assertEqual(self.dolt_head(), before)

    def test_export_prints_the_whole_queue_on_stdout_and_can_also_write_a_file(self) -> None:
        self.seed(bead("brwr-a1"), bead("brwr-b2"))
        a = self.bootstrap(self.a)
        printed = self.ok(a, "export")
        self.assertEqual(rows(printed), rows(self.bd(a, "export")))
        self.assertEqual(sorted(rows(printed)), ["brwr-a1", "brwr-b2", "brwr-gate"])
        path = self.tmp / "out" / "q.jsonl"
        self.assertEqual(self.ok(a, "export", str(path)), printed)
        self.assertEqual(path.read_text(encoding="utf-8"), printed)

    def test_an_empty_queue_fails_with_a_one_line_reason(self) -> None:
        self.bd(self.a, "init", "--prefix", "brwr", "--non-interactive", "--skip-hooks", "--skip-agents")
        result = self.queue(self.a, "export")
        self.assertEqual(result.returncode, 1)
        self.assertEqual(result.stdout, "")
        self.assertEqual(len(result.stderr.strip().splitlines()), 1, result.stderr)
        self.assertIn("empty", result.stderr)

    def test_a_bootstrapped_clone_matches_the_committed_project_id(self) -> None:
        # brwr-765q: the seed used to get a fresh project_id, so every clone
        # carrying the committed metadata.json failed bd's identity check.
        self.seed(bead("brwr-a1"))
        b = self.bootstrap(self.clone("b"))
        info = subprocess.run(["bd", "info"], cwd=b, capture_output=True, text=True)
        self.assertEqual(info.returncode, 0, info.stdout + info.stderr)
        self.assertNotIn("mismatch", info.stdout + info.stderr)
        committed = json.loads((b / ".beads" / "metadata.json").read_text(encoding="utf-8"))
        self.assertEqual(committed["project_id"], COMMITTED_METADATA["project_id"])
        self.assertEqual(self.git(b, "status", "--porcelain", "--", ".beads/metadata.json"), "")

    def test_seed_without_a_committed_metadata_json_says_which_id_to_commit(self) -> None:
        self.git(self.a, "rm", "-q", ".beads/metadata.json")
        out = self.seed(bead("brwr-a1"))
        self.assertRegex(out, r"commit \.beads/metadata\.json with project_id [0-9a-f-]{36}")

    def test_contract_commands_from_a_pristine_checkout_act_on_bosun_workspace(self) -> None:
        # The shared driver runs them in a pristine checkout of main, with
        # BOSUN_WORKSPACE and BEADS_DIR naming the workspace and its database.
        self.seed(bead("brwr-a1", metadata={"backlog_section": "plan"}))
        ws = self.bootstrap(self.clone("ws"))
        pristine = self.clone("pristine")
        env = {**os.environ, "BOSUN_WORKSPACE": str(ws), "BEADS_DIR": str(ws / ".beads")}

        def queue(*argv):
            result = subprocess.run([sys.executable, str(SCRIPTS / "dev" / "queue"), *argv], cwd=pristine, env=env,
                                    stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=180)
            self.assertEqual(result.returncode, 0, f"{argv}: {result.stdout}{result.stderr}")
            return result.stdout

        self.bd(ws, "update", "brwr-a1", "--title", "Edited in the workspace")
        before = self.dolt_head()
        queue("sync")
        self.assertNotEqual(self.dolt_head(), before, "sync must push the workspace's database")
        self.assertEqual(rows(queue("export"))["brwr-a1"]["title"], "Edited in the workspace")
        self.assertIn("Edited in the workspace", queue("backlog", "--print"))
        self.assertFalse((pristine / ".beads" / "embeddeddolt").exists(), "nothing may create a database in the checkout")

    def test_the_contract_commands_run_with_no_tty_and_driver_credentials(self) -> None:
        # The shared driver: workspace cwd, no TTY (new session, stdin closed),
        # GH_TOKEN, a credential helper in GIT_CONFIG_*, BOSUN_WORKSPACE.
        self.seed(bead("brwr-a1"))
        b = self.clone("b")
        env = {
            **os.environ, "GH_TOKEN": "not-a-real-token", "BOSUN_WORKSPACE": str(b),
            "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "credential.helper",
            "GIT_CONFIG_VALUE_0": "!f() { echo username=x-access-token; echo password=$GH_TOKEN; }; f",
        }
        contract = json.loads((SCRIPTS.parent / "worker-contract.json").read_text(encoding="utf-8"))

        def run(command: str) -> subprocess.CompletedProcess[str]:
            if command.startswith("scripts/dev/queue"):
                command = f"{sys.executable} {SCRIPTS / 'dev' / 'queue'}{command[len('scripts/dev/queue'):]}"
            return subprocess.run(["sh", "-c", command], cwd=b, env=env, stdin=subprocess.DEVNULL,
                                  capture_output=True, text=True, start_new_session=True, timeout=180)

        for key in ("bootstrap", "sync", "export"):
            result = run(contract["queue"][key])
            self.assertEqual(result.returncode, 0, f"{key}: {result.stdout}{result.stderr}")
        self.assertIn("brwr-a1", rows(result.stdout))
        self.bd(b, "update", "brwr-a1", "--set-metadata", "backlog_section=nowhere")
        failed = run(contract["queue"]["sync"])
        self.assertEqual(failed.returncode, 1)
        self.assertEqual(len(failed.stderr.strip().splitlines()), 1, failed.stderr)
        self.assertIn("unknown-backlog-section", failed.stderr)

    def test_adopt_changes_a_dependency_type_end_to_end(self) -> None:
        self.seed(bead("brwr-a1", dependencies=[{"issue_id": "brwr-a1", "depends_on_id": "brwr-b2", "type": "blocks"}]),
                  bead("brwr-b2"))
        a = self.bootstrap(self.a)
        base, _ = self.branch(a, [GATE, bead("brwr-a1", dependencies=[{"issue_id": "brwr-a1", "depends_on_id": "brwr-b2", "type": "blocks"}]), bead("brwr-b2")],
                              [GATE, bead("brwr-a1", dependencies=[{"issue_id": "brwr-a1", "depends_on_id": "brwr-b2", "type": "related"}]), bead("brwr-b2")])
        self.ok(a, "adopt", "--from", "HEAD", "--base", base)
        edges = [(d["depends_on_id"], d["type"]) for d in self.exported(a)["brwr-a1"].get("dependencies") or []]
        self.assertEqual(edges, [("brwr-b2", "related")])

    def test_adopt_refuses_a_field_the_database_changed_differently(self) -> None:
        self.seed(bead("brwr-a1"))
        a = self.bootstrap(self.a)
        self.bd(a, "update", "brwr-a1", "--status", "blocked")
        base, _ = self.branch(a, [GATE, bead("brwr-a1")], [GATE, bead("brwr-a1", status="closed", updated_at="2999-01-01T00:00:00Z")])
        before = self.dolt_head()
        result = self.queue(a, "adopt", "--from", "HEAD", "--base", base)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("brwr-a1 status: database 'blocked', branch 'closed'", result.stderr)
        self.assertEqual(self.exported(a)["brwr-a1"]["status"], "blocked")
        self.assertEqual(self.dolt_head(), before)

    def test_sync_passes_bd_syncs_exit_status_through(self) -> None:
        self.seed(bead("brwr-a1"))
        a = self.bootstrap(self.a)
        fake = self.tmp / "fakebin"
        fake.mkdir()
        real = shutil.which("bd")
        (fake / "bd").write_text(f'#!/bin/sh\nif [ "$1" = sync ]; then echo "push race" >&2; exit 3; fi\nexec {real} "$@"\n')
        (fake / "bd").chmod(0o755)
        result = subprocess.run([sys.executable, str(SCRIPTS / "dev" / "queue"), "--repo", str(a), "sync"],
                                capture_output=True, text=True, env={**os.environ, "PATH": f"{fake}:{os.environ['PATH']}"})
        self.assertEqual(result.returncode, 3, result.stderr)
        self.assertIn("bd sync exited 3", result.stderr)

    def branch(self, repo, base_rows, branch_rows):
        """A branch that committed the old queue file: (base commit, tip)."""
        self.git(repo, "checkout", "-q", "-b", "feat/old")
        (repo / ".beads").mkdir(exist_ok=True)
        (repo / work_queue.QUEUE_PATH).write_text(jsonl(*base_rows), encoding="utf-8")
        self.git(repo, "add", "-f", work_queue.QUEUE_PATH)
        self.git(repo, "commit", "-qm", "base")
        base = self.git(repo, "rev-parse", "HEAD")
        (repo / work_queue.QUEUE_PATH).write_text(jsonl(*branch_rows), encoding="utf-8")
        self.git(repo, "commit", "-qam", "branch edits the queue")
        return base, self.git(repo, "rev-parse", "HEAD")

    def test_backlog_renders_the_beads_into_the_frame(self) -> None:
        self.seed(bead("brwr-a1", metadata={"backlog_section": "plan"}))
        a = self.bootstrap(self.a)
        text = self.ok(a, "backlog", "--print")
        self.assertIn("`brwr-a1`", text)
        self.assertIn("Plan intro.", text)
        out = pathlib.Path(self.ok(a, "backlog").strip())
        self.assertEqual(out, a / "target" / "queue" / "BOSUN_FEATURE_BACKLOG.md")
        self.assertIn("passed", self.ok(a, "check"))

    def test_adopt_applies_an_old_branchs_edits_field_by_field_and_syncs(self) -> None:
        self.seed(bead("brwr-a1", labels=["keep", "drop"]), bead("brwr-b2"))
        a, b = self.bootstrap(self.a), self.bootstrap(self.clone("b"))
        # Since the branch was cut, b changed brwr-b2's priority in the database.
        self.bd(b, "update", "brwr-b2", "--priority", "0")
        self.ok(b, "sync")
        # The branch committed the old file: base, then its edits.
        self.git(a, "checkout", "-q", "-b", "feat/old")
        (a / ".beads").mkdir(exist_ok=True)
        base_rows = [GATE, bead("brwr-a1", labels=["keep", "drop"]), bead("brwr-b2")]
        (a / work_queue.QUEUE_PATH).write_text(jsonl(*base_rows), encoding="utf-8")
        self.git(a, "add", "-f", work_queue.QUEUE_PATH)
        self.git(a, "commit", "-qm", "base")
        base = self.git(a, "rev-parse", "HEAD")
        (a / work_queue.QUEUE_PATH).write_text(jsonl(
            GATE, bead("brwr-a1", labels=["keep"], status="closed"), bead("brwr-b2", title="Retitled on the branch"),
            bead("brwr-p1"),
        ), encoding="utf-8")
        self.git(a, "commit", "-qam", "branch edits the queue")
        self.assertIn("would change 3", self.ok(a, "adopt", "--from", "HEAD", "--base", base, "--dry-run"))
        self.ok(a, "adopt", "--from", "HEAD", "--base", base)
        self.ok(b, "sync")
        got = self.exported(b)
        self.assertEqual(got["brwr-a1"]["status"], "closed")
        self.assertEqual(got["brwr-a1"].get("labels"), ["keep"])
        self.assertEqual((got["brwr-b2"]["title"], got["brwr-b2"]["priority"]), ("Retitled on the branch", 0))
        self.assertIn("brwr-p1", got)


if __name__ == "__main__":
    with contextlib.suppress(SystemExit):
        unittest.main()
