"""Tests for scripts/check_split_docs.py: the frozen design history and testing
log, and their one-file-per-entry directories.

Run with `npm run test:dev-tools` (python3 -m unittest discover -s scripts/tests).
"""

from __future__ import annotations

import pathlib
import shutil
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import check_split_docs  # noqa: E402

HISTORY = "# Brauser design history\n\n## §5.2 Summaries — history\n\n### 2026-11-01 — Old record\n\nText.\n"
TESTING = "# Testing Strategy\n\n## Learning log\n\n- 2026-11-12: An old line.\n\n## SQLite index\n\n- 2026-12-01: not a log line.\n"


class CheckTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "docs" / "history").mkdir(parents=True)
        (self.root / "docs" / "testing-log").mkdir(parents=True)
        (self.root / "docs" / "history.md").write_text(HISTORY, encoding="utf-8")
        (self.root / "TESTING.md").write_text(TESTING, encoding="utf-8")

    def write(self, path: str, text: str) -> None:
        (self.root / path).write_text(text, encoding="utf-8")

    @unittest.skipUnless((REPO / "docs" / "history").is_dir(),
                         "sparse checkout without docs/ (pr-hygiene Dev tools); check_current_state.py checks the real logs")
    def test_repository_logs_pass(self) -> None:
        self.assertEqual(check_split_docs.check(REPO), [])

    def test_frozen_files_and_well_formed_entries_pass(self) -> None:
        self.write("docs/history/2026-11-14-a-record.md", "## 2026-11-14 — A record\n\nText.\n\n### Detail\n")
        self.write("docs/testing-log/2026-11-14-a-lesson.md", "## 2026-11-14 — A lesson\n\nText.\n")
        self.assertEqual(check_split_docs.check(self.root), [])

    def test_a_new_record_in_a_frozen_file_is_refused(self) -> None:
        self.write("docs/history.md", HISTORY + "\n## 2026-11-14 — Appended\n\nText.\n")
        self.write("TESTING.md", TESTING.replace("- 2026-11-12:", "- 2026-11-14: New.\n- 2026-11-12:"))
        errors = check_split_docs.check(self.root)
        self.assertEqual(len(errors), 2, errors)
        self.assertIn("docs/history.md: holds a history entry dated 2026-11-14", errors[0])
        self.assertIn("TESTING.md: holds a testing-log entry dated 2026-11-14", errors[1])

    def test_a_record_dated_on_or_before_the_freeze_still_passes(self) -> None:
        # An open PR from before the split may still land one; only later dates are new records.
        self.write("docs/history.md", HISTORY + f"\n## {check_split_docs.FROZEN_AFTER} — Late\n\nText.\n")
        self.assertEqual(check_split_docs.check(self.root), [])

    def test_malformed_entries_are_refused(self) -> None:
        self.write("docs/history/notes.md", "## 2026-11-14 — No date in the name\n")
        self.write("docs/testing-log/2026-11-14-bad.md", "## 2026-11-15 — Wrong date\n")
        errors = check_split_docs.check(self.root)
        self.assertTrue(any("docs/history/notes.md: not a history entry" in e for e in errors), errors)
        self.assertTrue(any("docs/testing-log/2026-11-14-bad.md: heading date 2026-11-15" in e for e in errors), errors)

    def test_missing_learning_log_is_reported(self) -> None:
        self.write("TESTING.md", "# Testing Strategy\n")
        self.assertTrue(any("no '## Learning log' section" in e for e in check_split_docs.check(self.root)))

    def test_print_puts_the_frozen_text_first_then_entries_in_order(self) -> None:
        self.write("docs/testing-log/2026-11-15-b.md", "## 2026-11-15 — B\n")
        self.write("docs/testing-log/2026-11-14-a.md", "## 2026-11-14 — A\r\n")
        self.assertEqual(
            check_split_docs.render(self.root, "testing-log"),
            "## Learning log\n\n- 2026-11-12: An old line.\n\n## 2026-11-14 — A\n\n## 2026-11-15 — B\n",
        )


if __name__ == "__main__":
    unittest.main()
