"""Tests for scripts/dev/create-bead: pull, create, sync; no real bd database."""

from __future__ import annotations

import contextlib
import io
import pathlib
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "dev" / "create-bead"
namespace = runpy.run_path(str(SCRIPT))
create_bead = namespace["main"]
work_queue = namespace["work_queue"]


class CreateBeadTests(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = pathlib.Path(tmp.name).resolve()
        self.synced = []
        self.kw = {"root": self.root, "sync": self.synced.append}

    def run_create(self, args, run):
        with patch("subprocess.run", side_effect=run), contextlib.redirect_stderr(io.StringIO()) as err:
            return create_bead(args, **self.kw), err.getvalue()

    def test_pull_then_create_then_sync_with_literal_arguments(self) -> None:
        calls = []
        args = ["Literal $(command) `title`", "-d", "two lines\nof detail", "--json"]

        def bd(command, **options):
            calls.append((command, options.get("cwd")))
            return subprocess.CompletedProcess(command, 0)

        code, _ = self.run_create(args, bd)
        self.assertEqual(code, 0)
        self.assertEqual(calls, [(["bd", "dolt", "pull"], self.root), (["bd", "create", *args], self.root)])
        self.assertEqual(self.synced, [self.root])

    def test_a_failed_pull_creates_nothing(self) -> None:
        calls = []

        def fail(command, **_options):
            calls.append(command)
            return subprocess.CompletedProcess(command, 7)

        self.assertEqual(self.run_create(["New work"], fail)[0], 7)
        self.assertEqual(calls, [["bd", "dolt", "pull"]])
        self.assertEqual(self.synced, [])

    def test_a_failed_create_is_reported_and_not_synced(self) -> None:
        results = iter([subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 9)])
        self.assertEqual(self.run_create(["New work"], lambda *a, **k: next(results))[0], 9)
        self.assertEqual(self.synced, [])

    def test_a_failed_sync_says_the_bead_exists_locally(self) -> None:
        def refuse(_root):
            raise work_queue.QueueError("remote not seeded")

        self.kw["sync"] = refuse
        code, err = self.run_create(["New work"], lambda command, **_: subprocess.CompletedProcess(command, 0))
        self.assertEqual(code, 1)
        self.assertIn("created in this database but not synced", err)

    def test_routing_overrides_are_rejected_before_anything_runs(self) -> None:
        for arg in ("--db", "--db=other", "--database=other", "--directory", "-C", "-Cother",
                    "-qC", "-vCother", "-vqC=other", "--global", "--repo", "--repo=other"):
            with self.subTest(arg=arg), patch("subprocess.run") as run, contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(create_bead(["New work", arg], **self.kw), 2)
                run.assert_not_called()

    def test_missing_bd_reports_failure(self) -> None:
        with patch("subprocess.run", side_effect=FileNotFoundError("bd")), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(create_bead(["New work"], **self.kw), 1)

    def test_help_does_not_touch_the_database(self) -> None:
        with patch("subprocess.run") as run, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(create_bead(["--help"], **self.kw), 0)
            self.assertEqual(create_bead([], **self.kw), 2)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
