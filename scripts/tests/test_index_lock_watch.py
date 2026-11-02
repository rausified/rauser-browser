"""Tests for scripts/dev/index-lock-watch with fake ps/lsof output and a fake
launchctl runner. Logs, plists and repositories live in temporary
directories; nothing touches launchctl, ~/Library or the live install.

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
import sys
import tempfile
import time
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "dev" / "index-lock-watch"
_loader = importlib.machinery.SourceFileLoader("index_lock_watch", str(SCRIPT))
_spec = importlib.util.spec_from_loader("index_lock_watch", _loader)
ilw = importlib.util.module_from_spec(_spec)
sys.modules["index_lock_watch"] = ilw
_loader.exec_module(ilw)


class FakeSystem:
    """Answers ps and lsof like macOS does, from canned tables."""

    def __init__(self, procs=(), cwds=None, holders=(), fail=()):
        self.procs = list(procs)  # (pid, ppid, command)
        self.cwds = cwds or {}
        self.holders = list(holders)  # (pid, command)
        self.fail = set(fail)
        self.calls = []

    def __call__(self, argv):
        argv = [str(a) for a in argv]
        self.calls.append(argv)
        name = os.path.basename(argv[0])
        if name == "ps":
            if "ps" in self.fail:
                return ilw.Result(1, "", "ps: denied")
            return ilw.Result(0, "".join(f"{p:>6} {pp:>6} {c}\n" for p, pp, c in self.procs))
        if name == "lsof" and "-d" in argv:  # cwd lookup
            if "cwd" in self.fail:
                return ilw.Result(1, "", "lsof: denied")
            pids = [int(p) for p in argv[argv.index("-p") + 1].split(",")]
            text = "".join(f"p{p}\nfcwd\nn{self.cwds[p]}\n" for p in pids if p in self.cwds)
            return ilw.Result(0 if text else 1, text)
        if name == "lsof":  # holders of the lock
            if "holders" in self.fail:
                return ilw.Result(2, "", "lsof: broke")
            if not self.holders:
                return ilw.Result(1, "")
            lock = argv[-1]
            return ilw.Result(0, "".join(f"p{p}\nc{c}\nf3\nn{lock}\n" for p, c in self.holders))
        if name == "launchctl":
            return ilw.Result(0, "")
        raise AssertionError(f"unexpected command {argv}")


class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="index-lock-watch-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "rauserBrowser"
        (self.repo / ".git" / "worktrees" / "rb-x").mkdir(parents=True)
        self.worktree = self.tmp / "rb-x"
        self.worktree.mkdir()
        (self.repo / ".git" / "worktrees" / "rb-x" / "gitdir").write_text(f"{self.worktree}/.git\n")
        self.lock = self.repo / ".git" / "index.lock"
        self.log = self.tmp / "logs" / "index-lock-watch.jsonl"
        self.home = self.tmp / "home"
        self.home.mkdir()

    def make_lock(self, age=0.0, content=""):
        self.lock.write_text(content)
        if age:
            past = time.time() - age
            os.utime(self.lock, (past, past))

    def watch(self, *argv, system=None, now=None):
        out = io.StringIO()
        system = system or FakeSystem()
        code = ilw.main(list(argv), runner=system, out=out, home=self.home, uid=501, now=now)
        return code, out.getvalue(), system

    def record(self, system=None, now=None):
        return self.watch("record", "--repo", str(self.repo), "--log", str(self.log),
                          system=system, now=now)

    def records(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]


# A process tree: launchd(1) -> Orca(100) -> zsh(200) -> claude(300) -> git(400),
# plus git in a linked worktree, git naming the repo, and git elsewhere.
# codex(350) runs with a private prompt, and git(470) carries a credential
# helper and a token in its URL: neither may reach the log.
SECRETS = ("secret prompt", "do-not-log", "credential.helper", "hunter2", "tok3n",
           "github.com", "user:", "notes-git.txt")


def busy_system(repo, worktree, holders=()):
    procs = [
        (1, 0, "/sbin/launchd"),
        (100, 1, "/Applications/Orca.app/Contents/MacOS/Orca"),
        (200, 100, "/bin/zsh -l"),
        (300, 200, "claude --resume"),
        (400, 300, "git status --porcelain"),
        (410, 300, "/usr/bin/git -C /elsewhere fetch"),
        (420, 200, f"git -C {repo} pull --ff-only"),
        (430, 100, "git-remote-https origin https://github.com/x/y"),
        (440, 200, "rust-analyzer"),
        (450, 440, "/opt/homebrew/bin/git rev-parse HEAD"),
        (460, 1, "vim notes-git.txt"),
        (350, 200, "codex exec 'secret prompt: do-not-log'"),
        (470, 350, "git -c credential.helper=!f-hunter2 fetch "
                   "https://user:tok3n@github.com/x/y.git main"),
    ]
    cwds = {400: str(repo), 410: "/elsewhere", 420: "/", 430: "/tmp", 450: str(worktree / "host"),
            470: str(repo), 300: str(repo), 350: str(repo / "host"), 200: "/Users/me"}
    return FakeSystem(procs, cwds, holders)


class Record(Fixture):
    def test_no_lock_writes_nothing(self):
        code, out, system = self.record()
        self.assertEqual(code, 0, out)
        self.assertFalse(self.log.exists())
        self.assertEqual(system.calls, [])

    def test_records_the_lock_holders_and_matching_git_processes(self):
        self.make_lock(age=5)
        system = busy_system(self.repo, self.worktree, holders=[(400, "git")])
        code, out, _ = self.record(system=system)
        self.assertEqual(code, 0, out)
        (rec,) = self.records()
        self.assertEqual(rec["repo"], str(self.repo))
        self.assertEqual(rec["lock"]["path"], str(self.lock))
        self.assertEqual(rec["lock"]["size"], 0)
        self.assertIn("mtime", rec["lock"])
        self.assertIn("inode", rec["lock"])
        self.assertTrue(rec["ts"])
        self.assertEqual(rec["holders"], [{"pid": 400, "exe": "git"}])
        pids = sorted(p["pid"] for p in rec["git_processes"])
        # 410 (another repo), 430 (cwd /tmp, argv names no repo) and the
        # non-git processes are left out; 450 runs in a linked worktree.
        self.assertEqual(pids, [400, 420, 450, 470])
        by_pid = {p["pid"]: p for p in rec["git_processes"]}
        self.assertEqual(by_pid[400], {
            "pid": 400, "ppid": 300, "exe": "git", "cwd": str(self.repo),
            "git": {"subcommand": "status", "flags": ["--porcelain"]},
            "ancestors": [
                {"pid": 300, "ppid": 200, "exe": "claude", "cwd": str(self.repo)},
                {"pid": 200, "ppid": 100, "exe": "zsh", "cwd": "/Users/me"},
                {"pid": 100, "ppid": 1, "exe": "Orca", "cwd": ""},
                {"pid": 1, "ppid": 0, "exe": "launchd", "cwd": ""},
            ]})
        # -C is kept only because it names the repo.
        self.assertEqual(by_pid[420]["git"],
                         {"subcommand": "pull", "dir": str(self.repo), "flags": ["--ff-only"]})
        self.assertEqual(by_pid[450]["exe"], "git")
        self.assertEqual(by_pid[450]["git"], {"subcommand": "rev-parse", "flags": []})
        self.assertEqual(by_pid[450]["ancestors"][0]["exe"], "rust-analyzer")
        self.assertEqual(by_pid[470]["git"], {"subcommand": "fetch", "flags": []})
        self.assertEqual([a["exe"] for a in by_pid[470]["ancestors"]][:2], ["codex", "zsh"])
        self.assertEqual(rec["errors"], [])

    def test_private_command_lines_never_reach_the_log(self):
        self.make_lock()
        self.record(system=busy_system(self.repo, self.worktree, holders=[(470, "git")]))
        text = self.log.read_text()
        for secret in SECRETS:
            self.assertNotIn(secret, text)
        self.assertNotIn('"command"', text)  # no raw command-line field at all
        code, out, _ = self.watch("report", "--log", str(self.log))
        self.assertEqual(code, 0, out)
        for secret in SECRETS:
            self.assertNotIn(secret, out)
        self.assertIn("codex", out)
        self.assertIn("git fetch", out)

    def test_git_flags_come_from_an_allowlist(self):
        repo = str(self.repo)
        roots = ilw.worktree_roots(self.repo)
        cases = {
            "git status --porcelain=v2 -z": {"subcommand": "status",
                                              "flags": ["--porcelain=v2", "-z"]},
            f"git -C {repo}/host --no-optional-locks status --short": {
                "subcommand": "status", "dir": f"{repo}/host",
                "flags": ["--no-optional-locks", "--short"]},
            "git -C /private/elsewhere status": {"subcommand": "status", "flags": []},
            "git --git-dir=/x/.git --work-tree /y commit -m my-message": {
                "subcommand": "commit", "flags": []},
            "git commit -m secret-message --amend": {"subcommand": "commit",
                                                      "flags": ["--amend"]},
            "git not-a-real-subcommand --porcelain": {"subcommand": None, "flags": []},
            "git -c user.name=me": {"subcommand": None, "flags": []},
            # ps joins argv with spaces: a value with a space ends the parse.
            "git -c credential.helper=!echo password=hunter2 fetch": {"subcommand": None,
                                                                      "flags": []},
            "git-remote-https origin https://user:tok3n@github.com/x": {"subcommand": None,
                                                                        "flags": []},
            "git": {"subcommand": None, "flags": []},
        }
        for command, want in cases.items():
            self.assertEqual(ilw.git_fields(command.split(), roots), want, command)

    def test_ancestor_chain_stops_at_six(self):
        self.make_lock()
        procs = [(i, i - 1, f"proc{i}") for i in range(1, 11)] + [(50, 10, "git commit")]
        system = FakeSystem(procs, {50: str(self.repo)})
        self.record(system=system)
        (rec,) = self.records()
        chain = rec["git_processes"][0]["ancestors"]
        self.assertEqual([a["pid"] for a in chain], [10, 9, 8, 7, 6, 5])
        self.assertEqual(chain[0], {"pid": 10, "ppid": 9, "exe": "proc10", "cwd": ""})

    def test_probe_failures_are_recorded_not_fatal(self):
        self.make_lock()
        system = FakeSystem(fail={"ps", "holders"})
        code, out, _ = self.record(system=system)
        self.assertEqual(code, 0, out)
        (rec,) = self.records()
        self.assertEqual(rec["git_processes"], [])
        self.assertEqual(len(rec["errors"]), 2, rec["errors"])

    def test_appends(self):
        self.make_lock()
        self.record()
        self.record()
        self.assertEqual(len(self.records()), 2)

    def test_rotates_above_one_mib(self):
        self.make_lock()
        self.log.parent.mkdir(parents=True)
        self.log.write_text("x" * (ilw.MAX_LOG_BYTES + 1))
        self.record()
        rotated = self.log.with_name(self.log.name + ".1")
        self.assertTrue(rotated.exists())
        self.assertEqual(rotated.stat().st_size, ilw.MAX_LOG_BYTES + 1)
        self.assertEqual(len(self.records()), 1)

    def test_parse_ps(self):
        procs = ilw.parse_ps("  1     0 /sbin/launchd\n 42     1 git -C /a b\nbad line\n")
        self.assertEqual(procs[42].ppid, 1)
        self.assertEqual(procs[42].command, "git -C /a b")
        self.assertEqual(set(procs), {1, 42})

    def test_exe_is_the_basename_of_argv0(self):
        self.assertEqual(ilw.exe_name("/usr/bin/git -C x"), "git")
        self.assertEqual(ilw.exe_name("codex exec 'a prompt'"), "codex")
        self.assertEqual(ilw.exe_name(""), "")

    def test_is_git(self):
        for cmd in ("git status", "/usr/bin/git -C x", "git-remote-https origin",
                    "/Library/Developer/CommandLineTools/usr/libexec/git-core/git fetch"):
            self.assertTrue(ilw.is_git(cmd), cmd)
        for cmd in ("vim git.txt", "gitk", "rust-analyzer", ""):
            self.assertFalse(ilw.is_git(cmd), cmd)


class Install(Fixture):
    def plist_path(self):
        return self.home / "Library/LaunchAgents/com.rauser.brauser.index-lock-watch.plist"

    def test_writes_the_plist_and_bootstraps_it(self):
        code, out, system = self.watch("install", "--repo", str(self.repo))
        self.assertEqual(code, 0, out)
        with self.plist_path().open("rb") as fh:
            data = plistlib.load(fh)
        self.assertEqual(data["Label"], "com.rauser.brauser.index-lock-watch")
        self.assertEqual(data["WatchPaths"], [str(self.repo / ".git")])
        self.assertEqual(data["ThrottleInterval"], 1)
        args = data["ProgramArguments"]
        self.assertEqual(Path(args[0]), SCRIPT)
        self.assertEqual(args[1:4], ["record", "--repo", str(self.repo)])
        self.assertEqual(system.calls, [
            ["/bin/launchctl", "bootout", "gui/501/com.rauser.brauser.index-lock-watch"],
            ["/bin/launchctl", "bootstrap", "gui/501", str(self.plist_path())],
        ])

    def test_custom_log_is_passed_to_record(self):
        code, out, _ = self.watch("install", "--repo", str(self.repo), "--log", str(self.log))
        self.assertEqual(code, 0, out)
        with self.plist_path().open("rb") as fh:
            args = plistlib.load(fh)["ProgramArguments"]
        self.assertEqual(args[-2:], ["--log", str(self.log)])

    def test_dry_run_writes_nothing(self):
        code, out, system = self.watch("install", "--repo", str(self.repo), "--dry-run")
        self.assertEqual(code, 0, out)
        self.assertEqual(system.calls, [])
        self.assertFalse(self.plist_path().exists())
        self.assertIn("launchctl bootstrap gui/501", out)
        self.assertIn("<key>WatchPaths</key>", out)

    def test_bootstrap_failure_fails(self):
        class Failing(FakeSystem):
            def __call__(self, argv):
                self.calls.append(argv)
                return ilw.Result(5, "", "Bootstrap failed: 5")

        code, out, _ = self.watch("install", "--repo", str(self.repo), system=Failing())
        self.assertNotEqual(code, 0)
        self.assertIn("Bootstrap failed", out)

    def test_linked_worktree_is_refused(self):
        (self.worktree / ".git").write_text("gitdir: elsewhere\n")
        code, out, system = self.watch("install", "--repo", str(self.worktree))
        self.assertNotEqual(code, 0)
        self.assertIn("main worktree", out)
        self.assertEqual(system.calls, [])

    def test_uninstall_boots_out_and_removes_the_plist(self):
        self.watch("install", "--repo", str(self.repo))
        code, out, system = self.watch("uninstall")
        self.assertEqual(code, 0, out)
        self.assertEqual(system.calls,
                         [["/bin/launchctl", "bootout", "gui/501/com.rauser.brauser.index-lock-watch"]])
        self.assertFalse(self.plist_path().exists())

    def test_uninstall_dry_run(self):
        self.watch("install", "--repo", str(self.repo))
        code, out, system = self.watch("uninstall", "--dry-run")
        self.assertEqual(code, 0, out)
        self.assertEqual(system.calls, [])
        self.assertTrue(self.plist_path().exists())
        self.assertIn("launchctl bootout", out)


class Report(Fixture):
    def write_log(self, entries, rotated=()):
        self.log.parent.mkdir(parents=True, exist_ok=True)
        self.log.write_text("".join(json.dumps(e) + "\n" for e in entries) + "not json\n")
        if rotated:
            self.log.with_name(self.log.name + ".1").write_text(
                "".join(json.dumps(e) + "\n" for e in rotated))

    @staticmethod
    def entry(ts, inode, mtime_ns, parents=(), holders=()):
        procs = [{"pid": 400 + i, "ppid": 300, "exe": "git", "cwd": "/r",
                  "git": {"subcommand": "status", "flags": ["--porcelain"]},
                  "ancestors": [{"pid": 300, "ppid": 1, "exe": p, "cwd": "/r"},
                                {"pid": 1, "ppid": 0, "exe": "launchd", "cwd": ""}]}
                 for i, p in enumerate(parents)]
        return {"ts": f"2026-11-01T10:{ts:02d}:00-07:00", "ts_epoch": 1000.0 + ts * 60,
                "repo": "/r", "lock": {"path": "/r/.git/index.lock", "size": 0, "inode": inode,
                                       "mtime_ns": mtime_ns, "mtime": "x"},
                "holders": [{"pid": 9, "exe": h} for h in holders],
                "git_processes": procs, "errors": []}

    def test_counts_parent_commands(self):
        self.write_log([self.entry(1, 1, 1, ["rust-analyzer"]),
                        self.entry(2, 2, 2, ["rust-analyzer", "claude"])],
                       rotated=[self.entry(0, 3, 3, ["rust-analyzer"])])
        code, out, _ = self.watch("report", "--log", str(self.log))
        self.assertEqual(code, 0, out)
        self.assertIn("3 records", out)
        lines = out.splitlines()
        parent = next(ln for ln in lines if "rust-analyzer" in ln)
        self.assertRegex(parent, r"^\s+3\s+rust-analyzer")
        self.assertTrue(any(ln.strip().startswith("1") and "claude" in ln for ln in lines))
        self.assertTrue(any(ln.strip().startswith("4") and "git status --porcelain" in ln
                            for ln in lines), out)
        self.assertIn("1 unreadable line", out)

    def test_ignores_raw_commands_in_older_records(self):
        old = self.entry(1, 1, 1, ["claude"])
        old["git_processes"][0]["command"] = "git commit -m xyzzy-text"
        old["git_processes"][0]["ancestors"][0]["command"] = "codex exec 'xyzzy prompt'"
        self.write_log([old])
        _, out, _ = self.watch("report", "--log", str(self.log))
        self.assertNotIn("xyzzy", out)

    def test_lock_seen_again_later_is_a_stale_candidate(self):
        self.write_log([
            self.entry(1, 7, 111, ["claude"]),  # created while claude's git ran
            self.entry(2, 8, 222, ["rust-analyzer"]),    # a different, short-lived lock
            self.entry(36, 7, 111),                      # same lock 35 minutes later, no git
        ])
        code, out, _ = self.watch("report", "--log", str(self.log))
        self.assertEqual(code, 0, out)
        section = out[out.index("stale candidates"):]
        self.assertIn("1 stale candidate", out)
        self.assertIn("10:01:00", section)
        self.assertIn("10:36:00", section)
        self.assertIn("2100s", section)
        self.assertIn("claude", section)
        self.assertIn("git status --porcelain", section)
        self.assertNotIn("rust-analyzer", section)

    def test_lock_with_no_git_and_no_holder_is_flagged(self):
        self.write_log([self.entry(1, 7, 111)])
        _, out, _ = self.watch("report", "--log", str(self.log))
        self.assertIn("1 record(s) with no git process and no holder", out)

    def test_missing_log(self):
        code, out, _ = self.watch("report", "--log", str(self.log))
        self.assertEqual(code, 0, out)
        self.assertIn("no records", out)


if __name__ == "__main__":
    unittest.main()
