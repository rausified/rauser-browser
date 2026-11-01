"""Tests for the hydrated issue-creation workflow; no real bd database."""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import runpy
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "dev" / "create-bead"
create_bead = runpy.run_path(str(SCRIPT))["main"]


class CreateBeadTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        (self.root / ".git").mkdir()
        (self.root / ".beads").mkdir()
        self.source = self.root / ".beads" / "issues.jsonl"
        self.closed = {"id": "brwr-t33", "title": "Already shipped", "status": "closed"}
        self.source.write_text(json.dumps(self.closed) + "\n", encoding="utf-8")

    def run_create(self, args, run):
        with patch("subprocess.run", side_effect=run), contextlib.redirect_stderr(io.StringIO()):
            return create_bead(args, root=self.root)

    def test_closed_ids_are_reserved_before_create_and_arguments_stay_literal(self) -> None:
        # Reproduce a DB missing the tracked closed row: create would reuse
        # t33 unless hydration precedes it. Retain legitimate DB-only work.
        database = {"brwr-local": {"title": "Unexported work"}}
        calls = []
        args = ["Literal $(command) `title`", "-d", "two lines\nof detail", "--json"]

        def bd(command, **options):
            calls.append((command, options))
            if command[1] == "import":
                for line in pathlib.Path(command[2]).read_text(encoding="utf-8").splitlines():
                    row = json.loads(line)
                    database[row["id"]] = row
            else:
                self.assertIn("brwr-t33", database, "closed IDs must already be reserved")
                self.assertIn("brwr-local", database, "hydration must not rebuild the DB")
            return subprocess.CompletedProcess(command, 0)

        self.assertEqual(self.run_create(args, bd), 0)
        self.assertEqual(calls, [
            (["bd", "import", str(self.source)], {
                "cwd": self.root, "check": False, "stdout": unittest.mock.ANY,
            }),
            (["bd", "create", *args], {"cwd": self.root, "check": False}),
        ])
        # Import chatter cannot corrupt `bd create --json` on stdout.
        self.assertIsInstance(calls[0][1]["stdout"], io.StringIO)

    def test_import_failure_prevents_creation(self) -> None:
        calls = []

        def fail(command, **_options):
            calls.append(command)
            return subprocess.CompletedProcess(command, 7)

        self.assertEqual(self.run_create(["New work"], fail), 7)
        self.assertEqual(calls, [["bd", "import", str(self.source)]])

    def test_bad_export_prevents_any_database_write(self) -> None:
        for text in ("not JSON\n", json.dumps(self.closed) + "\n" + json.dumps(self.closed), None):
            with self.subTest(text=text):
                if text is None:
                    self.source.unlink()
                else:
                    self.source.write_text(text, encoding="utf-8")
                with patch("subprocess.run") as run, contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(create_bead(["New work"], root=self.root), 1)
                    run.assert_not_called()

    def test_worktree_and_redirect_refuse_shared_database(self) -> None:
        for kind in ("worktree", "redirect"):
            with self.subTest(kind=kind):
                if kind == "worktree":
                    (self.root / ".git").rmdir()
                    (self.root / ".git").write_text("gitdir: /another/checkout\n", encoding="utf-8")
                else:
                    (self.root / ".git").unlink()
                    (self.root / ".git").mkdir()
                    (self.root / ".beads" / "redirect").write_text("/another/checkout", encoding="utf-8")
                with patch("subprocess.run") as run, contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(create_bead(["New work"], root=self.root), 2)
                    run.assert_not_called()

    def test_routing_overrides_are_rejected_before_hydration(self) -> None:
        for arg in ("--db", "--db=other", "--database=other", "--directory", "-C", "-Cother",
                    "-qC", "-vCother", "-vqC=other", "--global", "--repo", "--repo=other"):
            with self.subTest(arg=arg), patch("subprocess.run") as run, contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(create_bead(["New work", arg], root=self.root), 2)
                run.assert_not_called()

    def test_create_failure_is_reported(self) -> None:
        results = [subprocess.CompletedProcess([], 0), subprocess.CompletedProcess([], 9)]
        with patch("subprocess.run", side_effect=results):
            self.assertEqual(create_bead(["New work"], root=self.root), 9)

    def test_missing_bd_reports_failure(self) -> None:
        with patch("subprocess.run", side_effect=FileNotFoundError("bd")), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(create_bead(["New work"], root=self.root), 1)

    def test_help_does_not_touch_the_database(self) -> None:
        with patch("subprocess.run") as run, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(create_bead(["--help"], root=self.root), 0)
            self.assertEqual(create_bead([], root=self.root), 2)
            run.assert_not_called()
