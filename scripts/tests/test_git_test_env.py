"""Tests for scripts/tests/git_test_env.py: quiet, removable temporary git repos.

git runs background maintenance (`git maintenance run --auto --detach`, or a
detached `gc --auto`) after commits in the throwaway repos these tests build.
It can create and delete `.git/objects/maintenance.lock` while a test is
already removing the directory, so shutil.rmtree's error handler met a path
that had just vanished (macOS CI, PR #267, 2026-11-03).
"""

from __future__ import annotations

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import git_test_env  # noqa: E402


class RmtreeTests(unittest.TestCase):
    def test_a_path_that_vanished_counts_as_removed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            gone = pathlib.Path(tmp) / "objects" / "maintenance.lock"
            # What rmtree hands its handler when the entry disappeared first.
            git_test_env.make_writable_and_retry(os.unlink, str(gone), None)

    def test_rmtree_removes_read_only_git_objects(self) -> None:
        tmp = pathlib.Path(tempfile.mkdtemp())
        obj = tmp / ".git" / "objects" / "ab" / "cdef"
        obj.parent.mkdir(parents=True)
        obj.write_text("x")
        obj.chmod(0o444)
        git_test_env.rmtree(tmp)
        self.assertFalse(tmp.exists())


class EnvTests(unittest.TestCase):
    def config(self, env: dict[str, str], key: str) -> str:
        with tempfile.TemporaryDirectory() as tmp:
            subprocess.run(["git", "init", "-q", tmp], check=True, env=env)
            result = subprocess.run(["git", "-C", tmp, "config", "--get", key],
                                    capture_output=True, text=True, env=env)
            return result.stdout.strip()

    def test_background_maintenance_and_auto_gc_are_off(self) -> None:
        env = git_test_env.env()
        self.assertEqual(self.config(env, "maintenance.auto"), "false")
        self.assertEqual(self.config(env, "gc.auto"), "0")
        self.assertEqual(self.config(env, "gc.autoDetach"), "false")

    def test_extra_config_is_appended_not_replacing_the_quiet_settings(self) -> None:
        env = git_test_env.env(extra_config=[("credential.helper", "!echo")])
        self.assertEqual(self.config(env, "credential.helper"), "!echo")
        self.assertEqual(self.config(env, "maintenance.auto"), "false")

    def test_existing_config_entries_in_the_base_environment_are_kept(self) -> None:
        base = {**os.environ, "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "user.name",
                "GIT_CONFIG_VALUE_0": "kept"}
        env = git_test_env.env(base=base)
        self.assertEqual(self.config(env, "user.name"), "kept")
        self.assertEqual(self.config(env, "gc.auto"), "0")


class HelpersUseItTests(unittest.TestCase):
    """Every helper that builds a temporary repository runs git quietly."""

    def test_each_temp_repo_helper_turns_maintenance_off(self) -> None:
        import test_improvement_feed
        import test_worktree_hygiene

        for name, env in (("test_improvement_feed", test_improvement_feed.GIT_ENV),
                          ("test_worktree_hygiene", test_worktree_hygiene.GIT_ENV)):
            with self.subTest(helper=name):
                self.assertEqual(self.config_value(env, "maintenance.auto"), "false")
        import test_work_queue
        self.assertEqual(self.config_value(test_work_queue.QUIET_GIT, "gc.autoDetach"), "false")

    @staticmethod
    def config_value(env: dict[str, str], key: str) -> str:
        count = int(env.get("GIT_CONFIG_COUNT", "0"))
        values = {env[f"GIT_CONFIG_KEY_{i}"]: env[f"GIT_CONFIG_VALUE_{i}"] for i in range(count)}
        return values.get(key, "")


if __name__ == "__main__":
    unittest.main()
