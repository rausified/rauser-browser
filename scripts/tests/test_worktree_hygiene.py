"""Tests for scripts/dev/worktree-hygiene, against throwaway git repositories.

Run: python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import io
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "dev" / "worktree-hygiene"
_loader = importlib.machinery.SourceFileLoader("worktree_hygiene", str(SCRIPT))
_spec = importlib.util.spec_from_loader("worktree_hygiene", _loader)
wh = importlib.util.module_from_spec(_spec)
sys.modules["worktree_hygiene"] = wh
_loader.exec_module(wh)

GIT_ENV = {
    "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
    "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
    "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
}


def run(cwd, *args):
    proc = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True,
                          env={**os.environ, **GIT_ENV})
    if proc.returncode != 0:
        raise AssertionError(f"git {args}: {proc.stderr}")
    return proc.stdout.strip()


class FakeProbe:
    def __init__(self, open_files=(), holders=None, git_processes=(), fail=None):
        self._open = list(open_files)
        self._holders = holders or {}
        self._git = list(git_processes)
        self.fail = fail or set()

    def open_files(self):
        if "open" in self.fail:
            raise wh.ProbeError("lsof broke")
        return self._open

    def holders(self, path):
        if "holders" in self.fail:
            raise wh.ProbeError("lsof broke")
        return self._holders.get(str(path), [])

    def git_processes(self):
        if "git" in self.fail:
            raise wh.ProbeError("ps broke")
        return self._git


def no_prs(_repo, _branch):
    return []


@unittest.skipIf(sys.platform == "win32", "worktree-hygiene is a macOS/Linux dev tool: it relies on lsof, the Brauser LaunchAgent plist and the Chrome native-messaging manifest, none of which exist on Windows")
class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="wt-hygiene-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.origin = self.tmp / "origin.git"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(self.origin)], check=True,
                       env={**os.environ, **GIT_ENV})
        self.repo = self.tmp / "repo"
        subprocess.run(["git", "clone", "-q", str(self.origin), str(self.repo)], check=True,
                       capture_output=True, env={**os.environ, **GIT_ENV})
        run(self.repo, "checkout", "-q", "-b", "main")
        self.commit(self.repo, "README", "hello")
        run(self.repo, "push", "-q", "-u", "origin", "main")
        self.manifest = self.tmp / "com.rauser.brauser.json"
        self.plist = self.tmp / "com.rauser.brauser.service.plist"

    def commit(self, cwd, name, text):
        (Path(cwd) / name).write_text(text)
        run(cwd, "add", name)
        run(cwd, "commit", "-q", "-m", f"add {name}")
        return run(cwd, "rev-parse", "HEAD")

    def worktree(self, name, branch=None, start="main"):
        path = self.tmp / name
        if branch:
            run(self.repo, "worktree", "add", "-q", "-b", branch, str(path), start)
        else:
            run(self.repo, "worktree", "add", "-q", "--detach", str(path), start)
        return path

    def merged_worktree(self, name="wt-merged", branch="feat/merged"):
        """A worktree whose branch was pushed and merged with a merge commit."""
        path = self.worktree(name, branch)
        self.commit(path, f"{name}.txt", "work")
        run(path, "push", "-q", "-u", "origin", branch)
        run(self.repo, "merge", "-q", "--no-ff", "-m", f"Merge {branch}", branch)
        run(self.repo, "push", "-q", "origin", "main")
        run(self.repo, "fetch", "-q", "origin")
        return path

    def hygiene(self, *argv, probe=None, prs=no_prs, repo=None):
        out = io.StringIO()
        code = wh.main(
            ["--repo", str(repo or self.repo), "--native-host-manifest", str(self.manifest),
             "--launch-agent-plist", str(self.plist), *argv],
            probe=probe or FakeProbe(), pr_lookup=prs, out=out,
        )
        return code, out.getvalue()

    def line_for(self, output, path):
        lines = [ln for ln in output.splitlines() if f" {path} [" in ln]
        self.assertEqual(len(lines), 1, output)
        return lines[0].split()[0], lines[0]


class Candidates(Fixture):
    def test_merged_clean_worktree_is_listed_and_dry_run_changes_nothing(self):
        path = self.merged_worktree()
        code, out = self.hygiene()
        self.assertEqual(code, 0)
        action, line = self.line_for(out, path)
        self.assertEqual(action, "remove", out)
        self.assertIn("merged into origin/main", line)
        self.assertIn("dry run", line)
        self.assertTrue(path.exists())

    def test_apply_removes_the_worktree_but_keeps_the_branch(self):
        path = self.merged_worktree()
        code, out = self.hygiene("--apply")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.line_for(out, path)[0], "removed")
        self.assertFalse(path.exists())
        self.assertIn("feat/merged", run(self.repo, "branch", "--list", "feat/merged"))

    def test_removable_repo_does_not_break_later_removals_or_lock_checks(self):
        first = self.merged_worktree("wt-a", "feat/a")
        second = self.merged_worktree("wt-z", "feat/z")
        lock = self.repo / ".git" / "index.lock"
        lock.write_text("")
        past = time.time() - 3600
        os.utime(lock, (past, past))

        # The caller is outside --repo, so no cwd probe protects it. Remove
        # the supplied worktree first, then another, then check common locks.
        code, out = self.hygiene("--apply", "--clear-locks", repo=first)

        self.assertEqual(code, 0, out)
        for path in (first, second):
            self.assertEqual(self.line_for(out, path)[0], "removed", out)
            self.assertFalse(path.exists())
        self.assertFalse(lock.exists())
        self.assertIn(f"cleared {lock}", out)
        self.assertTrue(self.repo.exists())
        self.assertIn("feat/a", run(self.repo, "branch", "--list", "feat/a"))
        self.assertIn("feat/z", run(self.repo, "branch", "--list", "feat/z"))

    def test_fresh_branch_at_main_is_not_a_candidate(self):
        path = self.worktree("wt-fresh", "feat/fresh")
        _, out = self.hygiene()
        action, line = self.line_for(out, path)
        self.assertEqual(action, "skip")
        self.assertIn("no commits of its own", line)

    def test_head_base_uses_the_supplied_worktree(self):
        first = self.merged_worktree("wt-a", "feat/a")
        second = self.merged_worktree("wt-z", "feat/z")

        # Both branches are merged into the main worktree's HEAD, but neither
        # is finished relative to the first linked worktree's HEAD.
        code, out = self.hygiene("--apply", "--base", "HEAD", repo=first)

        self.assertEqual(code, 0, out)
        for path in (first, second):
            self.assertEqual(self.line_for(out, path)[0], "skip", out)
            self.assertTrue(path.exists())

    def test_the_shared_queue_worktree_is_never_a_candidate(self):
        # Even when its branch tip is merged into main, as a branch cut from
        # main and merged back would be.
        path = self.merged_worktree("repo-queue-beads", "beads")
        code, out = self.hygiene("--apply")
        self.assertEqual(code, 0, out)
        action, line = self.line_for(out, path)
        self.assertEqual(action, "skip")
        self.assertIn("work-queue worktree", line)
        self.assertTrue(path.exists())

    def test_detached_worktree_at_main_is_not_a_candidate(self):
        path = self.worktree("wt-detached")
        _, out = self.hygiene("--apply")
        self.assertEqual(self.line_for(out, path)[0], "skip")
        self.assertTrue(path.exists())

    def test_unmerged_branch_is_not_a_candidate(self):
        path = self.worktree("wt-wip", "feat/wip")
        self.commit(path, "wip.txt", "wip")
        _, out = self.hygiene("--apply")
        self.assertEqual(self.line_for(out, path)[0], "skip")
        self.assertTrue(path.exists())

    def test_closed_pr_makes_a_squash_merged_branch_a_candidate(self):
        path = self.worktree("wt-squash", "feat/squash")
        tip = self.commit(path, "squash.txt", "work")
        run(path, "push", "-q", "-u", "origin", "feat/squash")
        # Squash merge, then GitHub deletes the head branch.
        run(self.repo, "merge", "-q", "--squash", "feat/squash")
        run(self.repo, "commit", "-q", "-m", "squash")
        run(self.repo, "push", "-q", "origin", "main", ":feat/squash")
        run(self.repo, "fetch", "-q", "--prune", "origin")

        def prs(_repo, branch):
            return [{"number": 7, "state": "MERGED", "headRefOid": tip}] if branch == "feat/squash" else []

        _, out = self.hygiene("--apply", prs=prs)
        action, line = self.line_for(out, path)
        self.assertEqual(action, "removed", out)
        self.assertIn("PR #7 is merged", line)

    def test_closed_pr_does_not_cover_newer_local_commits(self):
        path = self.worktree("wt-reused", "feat/reused")
        old = self.commit(path, "a.txt", "a")
        self.commit(path, "b.txt", "b")

        def prs(_repo, _branch):
            return [{"number": 8, "state": "CLOSED", "headRefOid": old}]

        _, out = self.hygiene(prs=prs)
        self.assertEqual(self.line_for(out, path)[0], "skip")

    def test_open_pr_wins_over_merged_ancestry(self):
        path = self.merged_worktree()

        def prs(_repo, _branch):
            return [{"number": 9, "state": "OPEN", "headRefOid": "x"}]

        _, out = self.hygiene(prs=prs)
        action, line = self.line_for(out, path)
        self.assertEqual(action, "skip")
        self.assertIn("PR #9 is open", line)

    def test_main_worktree_is_never_removed(self):
        _, out = self.hygiene("--apply")
        self.assertEqual(self.line_for(out, self.repo)[0], "skip")


class Refusals(Fixture):
    def assert_kept(self, path, reason, **kw):
        _, out = self.hygiene("--apply", **kw)
        action, line = self.line_for(out, path)
        self.assertEqual(action, "keep", out)
        self.assertIn(reason, line)
        self.assertTrue(path.exists())

    def test_uncommitted_changes(self):
        path = self.merged_worktree()
        (path / "README").write_text("edited")
        self.assert_kept(path, "uncommitted")

    def test_untracked_files(self):
        path = self.merged_worktree()
        (path / "notes.txt").write_text("new")
        self.assert_kept(path, "uncommitted or untracked")

    def ignoring_worktree(self):
        """A merged worktree whose repo ignores local state and build output."""
        self.commit(self.repo, ".gitignore", "*.db\n.env\ntarget/\nnode_modules/\n")
        run(self.repo, "push", "-q", "origin", "main")
        return self.merged_worktree()

    def test_ignored_local_state_is_kept(self):
        # `git worktree remove` deletes ignored files without --force, and
        # `git status --untracked-files=all` does not list them.
        path = self.ignoring_worktree()
        (path / "beads.db").write_text("state")
        (path / ".env").write_text("SECRET=1")
        self.assert_kept(path, "2 ignored path(s)")
        self.assertTrue((path / "beads.db").exists())

    def test_ignored_build_output_does_not_block(self):
        path = self.ignoring_worktree()
        (path / "target" / "debug").mkdir(parents=True)
        (path / "target" / "debug" / "app").write_text("bin")
        (path / "host" / "target").mkdir(parents=True)
        (path / "host" / "target" / "x").write_text("bin")
        (path / "node_modules").mkdir()
        (path / "node_modules" / "m.js").write_text("js")
        code, out = self.hygiene("--apply")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.line_for(out, path)[0], "removed", out)
        self.assertFalse(path.exists())

    def test_ignored_state_inside_build_output_dir_name_elsewhere_is_kept(self):
        path = self.ignoring_worktree()
        (path / "data").mkdir()
        (path / "data" / "target.db").write_text("state")
        self.assert_kept(path, "1 ignored path(s)")

    def test_discard_ignored_removes_anyway(self):
        path = self.ignoring_worktree()
        (path / "beads.db").write_text("state")
        _, out = self.hygiene("--apply", "--discard-ignored")
        self.assertEqual(self.line_for(out, path)[0], "removed", out)
        self.assertFalse(path.exists())

    def test_unpushed_commits(self):
        path = self.merged_worktree()
        local = self.commit(path, "local.txt", "local")
        # The local commit lands in main's history only locally.
        run(self.repo, "merge", "-q", "--no-ff", "-m", "local merge", local)
        v = wh.Verdict(wh.list_worktrees(self.repo)[1], candidate=True)
        wh.safety_checks(self.repo, v, [], [], [])
        self.assertTrue(any("on no remote branch" in k for k in v.keep), v.keep)

    def test_process_with_cwd_inside(self):
        path = self.merged_worktree()
        probe = FakeProbe(open_files=[wh.OpenFile(4242, str(path), "zsh")])
        self.assert_kept(path, "in use by zsh[4242]", probe=probe)

    def test_open_file_deep_inside(self):
        path = self.merged_worktree()
        probe = FakeProbe(open_files=[wh.OpenFile(77, str(path / "target/debug/brauser"), "brauser")])
        self.assert_kept(path, "brauser[77]", probe=probe)

    def test_sibling_with_a_shared_prefix_is_not_inside(self):
        path = self.merged_worktree()
        probe = FakeProbe(open_files=[wh.OpenFile(5, str(path) + "-other/file", "vim")])
        _, out = self.hygiene(probe=probe)
        self.assertEqual(self.line_for(out, path)[0], "remove")

    def test_lsof_failure_keeps_everything(self):
        path = self.merged_worktree()
        self.assert_kept(path, "cannot check open files", probe=FakeProbe(fail={"open"}))

    def test_native_messaging_manifest_reference(self):
        path = self.merged_worktree()
        self.manifest.write_text(json.dumps({
            "name": "com.rauser.brauser", "path": str(path / "target/debug/brauser"),
            "type": "stdio", "allowed_origins": ["chrome-extension://abc/"],
        }))
        self.assert_kept(path, f"referenced by {self.manifest}")

    def test_launch_agent_plist_reference(self):
        path = self.merged_worktree()
        with self.plist.open("wb") as fh:
            plistlib.dump({"Label": "com.rauser.brauser.service",
                           "ProgramArguments": [str(path / "target/debug/brauser"), "service"]}, fh)
        self.assert_kept(path, f"referenced by {self.plist}")

    def test_unreadable_install_file_keeps_everything(self):
        path = self.merged_worktree()
        self.manifest.write_text("{not json")
        self.assert_kept(path, "cannot check the live install")

    def test_install_files_naming_another_worktree_do_not_block(self):
        path = self.merged_worktree()
        self.manifest.write_text(json.dumps({"path": str(self.repo / "target/debug/brauser")}))
        _, out = self.hygiene()
        self.assertEqual(self.line_for(out, path)[0], "remove")

    def test_locked_worktree(self):
        path = self.merged_worktree()
        run(self.repo, "worktree", "lock", str(path))
        self.assert_kept(path, "locked")

    @unittest.skipUnless(shutil.which("lsof"), "lsof not installed")
    def test_real_lsof_sees_an_open_file(self):
        path = self.merged_worktree()
        try:
            wh.SystemProbe().open_files()
        except wh.ProbeError as exc:
            self.skipTest(f"lsof unusable here: {exc}")
        with (path / "README").open("r"):
            _, out = self.hygiene("--apply", probe=wh.SystemProbe())
        action, line = self.line_for(out, path)
        self.assertEqual(action, "keep", out)
        self.assertIn("in use by", line)


class Locks(Fixture):
    def make_lock(self, worktree=None, age=3600):
        if worktree is None:
            lock = self.repo / ".git" / "index.lock"
        else:
            gitdir = Path(run(worktree, "rev-parse", "--path-format=absolute", "--git-dir"))
            lock = gitdir / "index.lock"
        lock.write_text("")
        past = time.time() - age
        os.utime(lock, (past, past))
        return lock

    def test_dry_run_reports_a_stale_lock_and_apply_clears_it(self):
        lock = self.make_lock()
        _, out = self.hygiene("--clear-locks")
        self.assertIn(f"clear   {lock}: stale (dry run)", out)
        self.assertTrue(lock.exists())
        _, out = self.hygiene("--clear-locks", "--apply")
        self.assertIn(f"cleared {lock}", out)
        self.assertFalse(lock.exists())

    def test_worktree_lock_is_found(self):
        path = self.worktree("wt-locky", "feat/locky")
        lock = self.make_lock(path)
        _, out = self.hygiene("--clear-locks", "--apply")
        self.assertIn(f"cleared {lock}", out)

    def test_held_lock_is_kept(self):
        lock = self.make_lock()
        probe = FakeProbe(holders={str(lock): [wh.OpenFile(11, str(lock), "git")]})
        _, out = self.hygiene("--clear-locks", "--apply", probe=probe)
        self.assertIn("held open by git[11]", out)
        self.assertTrue(lock.exists())

    def test_running_git_in_a_worktree_keeps_the_lock(self):
        path = self.worktree("wt-busy", "feat/busy")
        lock = self.make_lock()
        probe = FakeProbe(git_processes=[wh.GitProcess(12, str(path / "sub"), "git rebase")])
        _, out = self.hygiene("--clear-locks", "--apply", probe=probe)
        self.assertIn("git is running for this repository: pid 12", out)
        self.assertTrue(lock.exists())

    def test_running_git_naming_the_repo_keeps_the_lock(self):
        lock = self.make_lock()
        probe = FakeProbe(git_processes=[wh.GitProcess(13, "/", f"git -C {self.repo} commit")])
        _, out = self.hygiene("--clear-locks", "--apply", probe=probe)
        self.assertIn("pid 13", out)
        self.assertTrue(lock.exists())

    def test_git_in_another_repo_does_not_block(self):
        lock = self.make_lock()
        probe = FakeProbe(git_processes=[wh.GitProcess(14, str(self.tmp / "elsewhere"), "git status")])
        _, out = self.hygiene("--clear-locks", "--apply", probe=probe)
        self.assertIn(f"cleared {lock}", out)

    def test_young_lock_is_kept(self):
        lock = self.make_lock(age=1)
        _, out = self.hygiene("--clear-locks", "--apply")
        self.assertIn("minimum 60s", out)
        self.assertTrue(lock.exists())

    def test_probe_failure_keeps_the_lock(self):
        lock = self.make_lock()
        for fail in ("holders", "git"):
            _, out = self.hygiene("--clear-locks", "--apply", probe=FakeProbe(fail={fail}))
            self.assertIn("cannot", out)
            self.assertTrue(lock.exists())

    def test_parse_lsof(self):
        files = wh.parse_lsof("p12\ncgit\nfcwd\nn/a/b\nf3\nn/a/b/.git/index.lock\np13\ncvim\nn/c\n")
        self.assertEqual([(f.pid, f.command, f.path) for f in files],
                         [(12, "git", "/a/b"), (12, "git", "/a/b/.git/index.lock"), (13, "vim", "/c")])



class LocksOnly(Fixture):
    """--locks-only: the lock rules of --clear-locks, and no worktree work at all."""

    make_lock = Locks.make_lock

    @staticmethod
    def no_gh(_repo, _branch):
        raise AssertionError("--locks-only must not ask GitHub for PR state")

    def test_apply_clears_a_stale_lock_and_never_removes_a_worktree(self):
        merged = self.merged_worktree()
        lock = self.make_lock()
        code, out = self.hygiene("--locks-only", "--apply", prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertIn(f"cleared {lock}", out)
        self.assertFalse(lock.exists())
        # The merged worktree would be removed by a plain --apply run.
        self.assertTrue(merged.exists())
        self.assertNotIn(str(merged), out)
        for word in ("remove", "removed", "skip"):
            self.assertNotRegex(out, rf"^\s+{word}\s", out)
        self.assertIn(str(merged), run(self.repo, "worktree", "list"))

    def test_dry_run_reports_without_clearing(self):
        lock = self.make_lock()
        code, out = self.hygiene("--locks-only", prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertIn(f"clear   {lock}: stale (dry run)", out)
        self.assertTrue(lock.exists())

    def test_base_need_not_resolve(self):
        lock = self.make_lock()
        code, out = self.hygiene("--locks-only", "--apply", "--base", "origin/no-such-branch",
                                 prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertNotIn("does not exist", out)
        self.assertFalse(lock.exists())

    def test_runs_from_a_linked_worktree_and_finds_its_lock(self):
        path = self.worktree("wt-linked", "feat/linked")
        main_lock = self.make_lock()
        wt_lock = self.make_lock(path)
        code, out = self.hygiene("--locks-only", "--apply", repo=path, prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertFalse(main_lock.exists())
        self.assertFalse(wt_lock.exists())
        self.assertTrue(path.exists())

    def test_no_locks(self):
        code, out = self.hygiene("--locks-only", "--apply", prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertIn("no index.lock files", out)

    def test_young_lock_is_kept(self):
        lock = self.make_lock(age=1)
        _, out = self.hygiene("--locks-only", "--apply", prs=self.no_gh)
        self.assertIn(f"keep    {lock}: only", out)
        self.assertIn("minimum 60s", out)
        self.assertTrue(lock.exists())

    def test_min_lock_age_still_applies(self):
        lock = self.make_lock(age=120)
        _, out = self.hygiene("--locks-only", "--apply", "--min-lock-age", "600", prs=self.no_gh)
        self.assertIn("minimum 600s", out)
        self.assertTrue(lock.exists())

    def test_held_lock_is_kept(self):
        lock = self.make_lock()
        probe = FakeProbe(holders={str(lock): [wh.OpenFile(21, str(lock), "git")]})
        _, out = self.hygiene("--locks-only", "--apply", probe=probe, prs=self.no_gh)
        self.assertIn("held open by git[21]", out)
        self.assertTrue(lock.exists())

    def test_running_git_keeps_the_lock(self):
        lock = self.make_lock()
        probe = FakeProbe(git_processes=[wh.GitProcess(22, str(self.repo), "git pull")])
        _, out = self.hygiene("--locks-only", "--apply", probe=probe, prs=self.no_gh)
        self.assertIn("git is running for this repository: pid 22", out)
        self.assertTrue(lock.exists())

    def test_probe_failure_keeps_the_lock(self):
        lock = self.make_lock()
        for fail in ("holders", "git"):
            _, out = self.hygiene("--locks-only", "--apply", probe=FakeProbe(fail={fail}),
                                  prs=self.no_gh)
            self.assertIn("cannot", out)
            self.assertTrue(lock.exists())

    def test_open_files_probe_is_never_used(self):
        # Worktree safety probes (system-wide lsof) are skipped entirely.
        self.merged_worktree()
        self.make_lock()
        code, out = self.hygiene("--locks-only", "--apply", probe=FakeProbe(fail={"open"}),
                                 prs=self.no_gh)
        self.assertEqual(code, 0, out)
        self.assertNotIn("cannot check open files", out)

if __name__ == "__main__":
    unittest.main()
