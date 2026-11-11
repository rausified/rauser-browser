"""Tests for scripts/queue_doctor.py (`scripts/dev/queue doctor`).

The probe is a fixture: no bd, no network and no real database. The cited-id
scan and the identity parser run on scratch files and recorded bd output.
Run: python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""

from __future__ import annotations

import io
import pathlib
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

import queue_doctor  # noqa: E402
import work_queue  # noqa: E402

ID = "11111111-2222-3333-4444-555555555555"
OTHER = "99999999-8888-7777-6666-555555555555"
# What bd prints when metadata.json and the database disagree (recorded shape).
MISMATCH = (f"Error: project identity mismatch\n  metadata.json project_id: {ID}\n"
            f"  database _project_id:     {OTHER}\n")


class FakeProbe(queue_doctor.Probe):
    def __init__(self, repo, *, committed=ID, context=None, context_error="", workspace=ID, ids=("brwr-a1",),
                 cached="aaa", remote="aaa", pending=(), fail=()):
        self.repo = repo
        self._committed, self._workspace = committed, workspace
        self._context = context if context is not None else {"project_id": ID, "beads_dir": "/ws/.beads"}
        self._context_error, self._ids = context_error, set(ids)
        self._cached, self._remote, self._pending = cached, remote, list(pending)
        self.fail = set(fail)

    def _maybe(self, what):
        if what in self.fail:
            raise queue_doctor.ProbeError(f"{what} broke")

    def committed_project_id(self):
        return self._committed

    def context(self):
        return (None if self._context_error else self._context), self._context_error

    def workspace_project_id(self, beads_dir):
        return self._workspace

    def queue_ids(self):
        self._maybe("export")
        return self._ids

    def cached_remote_sha(self, context):
        self._maybe("cache")
        return self._cached

    def remote_sha(self):
        self._maybe("remote")
        return self._remote

    def pending(self):
        self._maybe("pending")
        return self._pending


class DoctorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.tmp.name)
        (self.repo / "docs/backlog/notes").mkdir(parents=True)
        (self.repo / "docs/backlog/frame.md").write_text("# Frame\r\nsee brwr-a1\r\n", encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def doctor(self, **kw):
        out = io.StringIO()
        code = queue_doctor.doctor(self.repo, FakeProbe(self.repo, **kw), out)
        return code, out.getvalue()

    def test_a_healthy_queue_exits_zero_and_changes_nothing(self):
        code, text = self.doctor()
        self.assertEqual(code, 0, text)
        self.assertIn("read-only", text)
        self.assertEqual(text.count("[ok]"), 4, text)
        self.assertIn("no problems", text)

    def test_a_database_identity_mismatch_names_both_ids_and_the_safe_repair(self):
        code, text = self.doctor(context_error=MISMATCH)
        self.assertEqual(code, 1)
        self.assertIn("[PROBLEM] identity", text)
        self.assertIn(f"database _project_id {OTHER}", text)
        self.assertIn("bd bootstrap", text)
        self.assertIn("repair:", text)

    def test_a_workspace_metadata_that_differs_from_the_committed_one(self):
        code, text = self.doctor(workspace=OTHER)
        self.assertEqual(code, 1)
        self.assertIn("[PROBLEM] identity", text)
        self.assertIn(f"workspace metadata.json says {OTHER}", text)

    def test_bead_ids_cited_in_the_backlog_but_missing_from_the_queue(self):
        (self.repo / "docs/backlog/notes/2026-11-01-x.md").write_text("one\nneeds brwr-zz9 and brwr-a1\n",
                                                                       encoding="utf-8")
        code, text = self.doctor()
        self.assertEqual(code, 1)
        self.assertIn("[PROBLEM] cited ids", text)
        self.assertIn("brwr-zz9 (docs/backlog/notes/2026-11-01-x.md:2)", text)
        self.assertNotIn("brwr-a1 (", text)

    def test_a_remote_that_moved_since_the_last_pull(self):
        code, text = self.doctor(cached="aaa", remote="bbb")
        self.assertEqual(code, 1)
        self.assertIn("[PROBLEM] remote", text)
        self.assertIn("scripts/dev/queue sync", text)

    def test_a_database_that_never_pulled_or_a_remote_never_seeded(self):
        _, text = self.doctor(cached=None)
        self.assertIn("[unknown] remote", text)
        code, text = self.doctor(remote=None)
        self.assertEqual(code, 1)
        self.assertIn("never seeded", text)

    def test_pending_unsynced_edits_are_counted_with_their_ids(self):
        code, text = self.doctor(pending=[{"IssueID": "brwr-a1", "DiffType": "modified"},
                                          {"IssueID": "brwr-b2", "DiffType": "added"}])
        self.assertEqual(code, 1)
        self.assertIn("[PROBLEM] pending", text)
        self.assertIn("2 bead edit(s)", text)
        self.assertIn("brwr-a1 (modified)", text)

    def test_a_source_that_cannot_answer_is_unknown_not_a_crash(self):
        code, text = self.doctor(fail={"export", "remote", "pending"})
        self.assertIn("[unknown] cited ids: export broke", text)
        self.assertIn("[unknown] remote", text)
        self.assertIn("[unknown] pending", text)
        self.assertEqual(code, 0)


class ParsingTests(unittest.TestCase):
    def test_parses_bds_identity_mismatch_message(self):
        self.assertEqual(queue_doctor.parse_mismatch(MISMATCH), (ID, OTHER))
        self.assertEqual(queue_doctor.parse_mismatch(MISMATCH.replace("\n", "\r\n")), (ID, OTHER))
        self.assertIsNone(queue_doctor.parse_mismatch("Error: database locked"))

    def test_bds_plain_no_changes_line_means_nothing_pending(self):
        # `bd diff --json` prints text, not JSON, when there is no difference.
        self.assertEqual(queue_doctor.parse_diff("No changes between origin/main and main\r\n"), [])
        self.assertEqual(queue_doctor.parse_diff('[{"IssueID": "brwr-a1"}]'), [{"IssueID": "brwr-a1"}])
        with self.assertRaises(queue_doctor.ProbeError):
            queue_doctor.parse_diff("Error: something else")


class CommandTests(unittest.TestCase):
    def test_doctor_is_a_queue_subcommand(self):
        calls = []
        original = queue_doctor.doctor
        queue_doctor.doctor = lambda repo, probe=None, out=None: calls.append(repo) or 1
        try:
            code = work_queue.main(["--repo", str(SCRIPTS.parent), "doctor"])
        finally:
            queue_doctor.doctor = original
        self.assertEqual((code, len(calls)), (1, 1))


if __name__ == "__main__":
    unittest.main()
