"""Tests for scripts/check_improvement_feed.py and the one-file-per-entry feed.

The merge tests prove the point of the layout: two branches that each add a
feed entry merge cleanly under a plain `git merge` with no merge drivers,
which is what GitHub's mergeability check and merge queue do.

Run with `npm run test:dev-tools` (python3 -m unittest discover -s scripts/tests).
"""

from __future__ import annotations

import os
import stat
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent


def _make_writable_and_retry(func, path, _exc):
    # git marks object files read-only; Windows refuses to unlink those.
    os.chmod(path, stat.S_IWRITE)
    func(path)


def _rmtree(path: pathlib.Path) -> None:
    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=_make_writable_and_retry)
    else:
        shutil.rmtree(path, onerror=_make_writable_and_retry)

REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import check_improvement_feed  # noqa: E402

ENTRY_POINT = (REPO / "BRAUSER_IMPROVEMENT_FEED.md").read_text(encoding="utf-8")


def entry(date: str, title: str, body: str = "Change: something. Why: evidence. Where: here.") -> str:
    return f"## {date} — {title}\n\n{body}\n"


class CheckTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "BRAUSER_IMPROVEMENT_FEED.md").write_text(ENTRY_POINT, encoding="utf-8")
        self.feed = self.root / "docs" / "improvement-feed"
        self.feed.mkdir(parents=True)

    def add(self, name: str, text: str) -> None:
        (self.feed / name).write_text(text, encoding="utf-8")

    @unittest.skipUnless((REPO / "docs" / "improvement-feed").is_dir(),
                         "sparse checkout without docs/ (pr-hygiene Dev tools); check_current_state.py checks the real feed in build-os.yml")
    def test_repository_feed_passes(self) -> None:
        self.assertEqual(check_improvement_feed.check(REPO), [])

    def test_well_formed_entries_pass(self) -> None:
        self.add("2026-11-02-a-thing.md", entry("2026-11-02", "A thing"))
        self.add("2026-11-02-a-thing-2.md", entry("2026-11-02", "A thing", "Text.\n\n- +1 2026-11-03: again\n- Done 2026-11-04: #1"))
        self.assertEqual(check_improvement_feed.check(self.root), [])

    def test_rejects_malformed_entries(self) -> None:
        cases = {
            "2026-11-02-Upper.md": entry("2026-11-02", "Upper"),
            "notes.md": entry("2026-11-02", "No date"),
            "2026-11-02-no-heading.md": "Change: no heading.\n",
            "2026-11-02-bad-dash.md": "## 2026-11-02 - Hyphen not em dash\n\nText.\n",
            "2026-11-02-wrong-date.md": entry("2026-11-01", "Wrong date"),
            "2026-11-02-two.md": entry("2026-11-02", "One") + "\n" + entry("2026-11-02", "Two"),
            "2026-11-02-no-newline.md": entry("2026-11-02", "No newline").rstrip("\n"),
            "2026-11-02-blank-tail.md": entry("2026-11-02", "Blank tail") + "\n",
        }
        for name, text in cases.items():
            with self.subTest(name=name):
                for path in self.feed.iterdir():
                    path.unlink()
                self.add(name, text)
                errors = check_improvement_feed.check(self.root)
                self.assertEqual(len(errors), 1, errors)
                self.assertIn(name, errors[0])

    def test_rejects_entries_in_the_entry_point(self) -> None:
        (self.root / "BRAUSER_IMPROVEMENT_FEED.md").write_text(
            ENTRY_POINT + "\n" + entry("2026-11-02", "Appended the old way"), encoding="utf-8")
        errors = check_improvement_feed.check(self.root)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("own file", errors[0])

    def test_rejects_a_missing_title_or_directory(self) -> None:
        (self.root / "BRAUSER_IMPROVEMENT_FEED.md").write_text("# Something else\n", encoding="utf-8")
        shutil.rmtree(self.feed)
        errors = check_improvement_feed.check(self.root)
        self.assertEqual(len(errors), 2, errors)


def git(cwd: pathlib.Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    env = {
        **os.environ,
        # No user or system config, so no merge driver (or rerere) can be
        # registered: the same plain text merge GitHub's server does.
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.invalid",
        "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.invalid",
    }
    return subprocess.run(["git", *args], cwd=cwd, env=env, check=check, capture_output=True, text=True, encoding="utf-8")


class PlainMergeTests(unittest.TestCase):
    """Two branches each add an entry; merge them with no merge drivers."""

    def setUp(self) -> None:
        self.repo = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(_rmtree, self.repo)
        git(self.repo, "init", "-q", "-b", "main")
        # The repository's real merge rules, so the test fails if the feed
        # ever comes to depend on a driver again.
        shutil.copy(REPO / ".gitattributes", self.repo / ".gitattributes")
        self.assertEqual(git(self.repo, "config", "--get-regexp", r"^merge\.", check=False).stdout, "")

    def commit_all(self, message: str) -> None:
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-q", "-m", message)

    def branch(self, name: str, write) -> None:
        git(self.repo, "checkout", "-q", "-b", name, "main")
        write()
        self.commit_all(name)

    def merge_both(self) -> subprocess.CompletedProcess[str]:
        git(self.repo, "checkout", "-q", "-b", "merged", "a")
        return git(self.repo, "merge", "--no-edit", "b", check=False)

    def test_per_entry_files_merge_cleanly(self) -> None:
        feed = self.repo / "docs" / "improvement-feed"
        feed.mkdir(parents=True)
        (self.repo / "BRAUSER_IMPROVEMENT_FEED.md").write_text(ENTRY_POINT, encoding="utf-8")
        (feed / "2026-11-01-old.md").write_text(entry("2026-11-01", "Old"), encoding="utf-8")
        self.commit_all("base")
        a = entry("2026-11-02", "From branch a")
        b = entry("2026-11-02", "From branch b")
        self.branch("a", lambda: (feed / "2026-11-02-from-branch-a.md").write_text(a, encoding="utf-8"))
        self.branch("b", lambda: (feed / "2026-11-02-from-branch-b.md").write_text(b, encoding="utf-8"))

        result = self.merge_both()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual((feed / "2026-11-02-from-branch-a.md").read_text(encoding="utf-8"), a)
        self.assertEqual((feed / "2026-11-02-from-branch-b.md").read_text(encoding="utf-8"), b)
        self.assertEqual((self.repo / "BRAUSER_IMPROVEMENT_FEED.md").read_text(encoding="utf-8"), ENTRY_POINT)
        self.assertEqual(check_improvement_feed.check(self.repo), [])

    def test_reinforcing_one_entry_while_adding_another_merges_cleanly(self) -> None:
        feed = self.repo / "docs" / "improvement-feed"
        feed.mkdir(parents=True)
        (self.repo / "BRAUSER_IMPROVEMENT_FEED.md").write_text(ENTRY_POINT, encoding="utf-8")
        old = feed / "2026-11-01-old.md"
        old.write_text(entry("2026-11-01", "Old"), encoding="utf-8")
        self.commit_all("base")
        self.branch("a", lambda: old.write_text(old.read_text(encoding="utf-8") + "- +1 2026-11-02: seen again\n", encoding="utf-8"))
        self.branch("b", lambda: (feed / "2026-11-02-new.md").write_text(entry("2026-11-02", "New"), encoding="utf-8"))

        result = self.merge_both()

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(old.read_text(encoding="utf-8").endswith("- +1 2026-11-02: seen again\n"))
        self.assertEqual(check_improvement_feed.check(self.repo), [])

    def test_the_old_single_file_layout_conflicts(self) -> None:
        """Control: the layout this replaced conflicts under the same merge."""
        feed = self.repo / "FEED.md"
        feed.write_text("# Feed\n\n" + entry("2026-11-01", "Old"), encoding="utf-8")
        self.commit_all("base")
        self.branch("a", lambda: feed.write_text(feed.read_text(encoding="utf-8") + "\n" + entry("2026-11-02", "A"), encoding="utf-8"))
        self.branch("b", lambda: feed.write_text(feed.read_text(encoding="utf-8") + "\n" + entry("2026-11-02", "B"), encoding="utf-8"))

        result = self.merge_both()

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("CONFLICT", result.stdout)


if __name__ == "__main__":
    unittest.main()
