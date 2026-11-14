"""Tests for scripts/dev/deploy-live. Every subprocess goes through a fake
runner, and every path (repo, support dir, native host manifest, home) is a
temporary directory: nothing touches git, cargo, npm, launchctl, Chrome or
the live install.

Run: python3 -m unittest discover -s scripts/tests -p 'test_*.py'
"""

from __future__ import annotations

import hashlib
import importlib.machinery
import importlib.util
import io
import json
import os
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / "dev" / "deploy-live"
_loader = importlib.machinery.SourceFileLoader("deploy_live", str(SCRIPT))
_spec = importlib.util.spec_from_loader("deploy_live", _loader)
dl = importlib.util.module_from_spec(_spec)
sys.modules["deploy_live"] = dl
_loader.exec_module(dl)

EXT_ID = "hngmhpdocldeeflhomgknpfilahjcidd"
OLD = "1111111111111111111111111111111111111111"
NEW = "abcdef012345abcdef012345abcdef012345abcd"
MID = "5555555555555555555555555555555555555555"


def frame(obj) -> bytes:
    body = json.dumps(obj).encode()
    return struct.pack("<I", len(body)) + body


class FakeRunner:
    """Answers the commands deploy-live runs; records every call."""

    def __init__(self, repo: Path):
        self.repo = repo
        self.calls: list[dict] = []
        self.git_dir = repo / ".git"
        self.common_dir = repo / ".git"
        self.toplevel = repo
        self.branch = "main"
        self.status = ""
        self.head = OLD
        self.after_pull = NEW
        self.changed_files = ""
        self.fail: set[str] = set()
        self.hygiene_out = "worktree hygiene: x index locks only, apply\n  no index.lock files\n"
        self.protocol = 8
        self.host_version = None  # default: 0.1.0+<NEW[:12]>
        self.hello_type = "hello_result"
        self.build_writes_dist = True
        self.on_cargo = None  # optional callable run during the cargo build
        self.lock_after_pull = None  # new package-lock.json text the pull brings
        self.script_after_pull = None  # new scripts/dev/deploy-live text the pull brings
        self.npm_hook = None  # optional callable(argv) -> Result or None for npm calls
        self.node_version = "v24.14.0"
        self.node_import_error = None
        self.history = [OLD, MID, NEW]  # origin/main, oldest first
        self.remote_head = NEW  # what git ls-remote reports for origin main
        self.refs: dict = {}  # ref name -> commit, for names that look like short SHAs
        self.ls_remote_result = None  # a Result to answer git ls-remote with instead

    # Calls whose first words match one of these act on the system.
    ACTING = ("pull", "fetch", "merge", "cargo", "npm", "node", "binary", "hygiene-apply",
              "publish")

    def kind(self, argv):
        if argv[0] == "os.replace":  # recorded by Fixture's os.replace wrapper
            return "publish" if argv[2].endswith("target/debug/brauser") else "os.replace"
        if argv[0] == "git":
            return argv[3] if argv[1] == "-C" else argv[1]
        if argv[0] == sys.executable and argv[1].endswith("worktree-hygiene"):
            return "hygiene-apply" if "--apply" in argv else "hygiene"
        if argv[0].endswith("target/debug/brauser"):
            return "binary"
        if argv[0] == "node" and argv[1] in ("--version", "--input-type=module"):
            return "node-preflight"
        return argv[0]

    def acting_calls(self):
        out = []
        for c in self.calls:
            k = self.kind(c["argv"])
            if k in self.ACTING:
                out.append(c)
        return out

    def __call__(self, argv, *, cwd, input=None, stream=False, timeout=None, env=None):
        argv = [str(a) for a in argv]
        self.calls.append({"argv": argv, "cwd": cwd, "input": input, "stream": stream,
                           "timeout": timeout, "env": env})
        k = self.kind(argv)
        if k in self.fail:
            return dl.Result(1, b"", f"{k} failed".encode())
        if k == "rev-parse" and "--show-toplevel" in argv:
            text = f"{self.toplevel}\n{self.git_dir}\n{self.common_dir}\n"
            return dl.Result(0, text.encode())
        if k == "rev-parse" and argv[-1] == "HEAD":
            return dl.Result(0, (self.head + "\n").encode())
        if k == "rev-parse" and "--verify" in argv:
            want = argv[-1].removesuffix("^{commit}")
            if want in self.refs:  # git prefers a ref with this name over a short SHA
                return dl.Result(0, (self.refs[want] + "\n").encode())
            found = [c for c in self.history if c.startswith(want)]
            if len(found) != 1:
                return dl.Result(128, b"", b"fatal: Needed a single revision")
            return dl.Result(0, (found[0] + "\n").encode())
        if k == "ls-remote":
            if self.ls_remote_result is not None:
                return self.ls_remote_result
            return dl.Result(0, f"{self.remote_head}\trefs/heads/main\n".encode())
        if k == "merge-base":
            a, b = argv[-2], argv[-1]
            b = self.remote_head if b == "origin/main" else (self.head if b == "HEAD" else b)
            a = self.head if a == "HEAD" else a
            known = a in self.history and b in self.history
            return dl.Result(0 if known and self.history.index(a) <= self.history.index(b) else 1)
        if k == "merge":
            self.head = argv[-1]
            return dl.Result(0, b"Fast-forward\n")
        if k == "symbolic-ref":
            return dl.Result(0 if self.branch else 1, (self.branch + "\n").encode() if self.branch else b"")
        if k == "status":
            return dl.Result(0, self.status.encode())
        if k == "worktree":
            return dl.Result(0, f"worktree {self.repo}\nHEAD {OLD}\nbranch refs/heads/main\n\n"
                                f"worktree /elsewhere\nHEAD {OLD}\nbranch refs/heads/x\n\n".encode())
        if k == "pull":
            self.head = self.after_pull
            if self.lock_after_pull is not None:
                (self.repo / "package-lock.json").write_text(self.lock_after_pull)
            if self.script_after_pull is not None:
                script = self.repo / "scripts" / "dev" / "deploy-live"
                script.parent.mkdir(parents=True, exist_ok=True)
                script.write_text(self.script_after_pull)
            return dl.Result(0, b"Updating\n")
        if k == "diff":
            return dl.Result(0, self.changed_files.encode())
        if k == "node-preflight":
            if argv[1] == "--version":
                return dl.Result(0, (self.node_version + "\n").encode())
            if self.node_import_error:
                return dl.Result(1, b"", self.node_import_error.encode())
            return dl.Result(0)
        if k == "cargo":
            target = Path(argv[argv.index("--target-dir") + 1]) if "--target-dir" in argv \
                else self.repo / "target"
            (target / "debug").mkdir(parents=True, exist_ok=True)
            (target / "debug" / "brauser").write_text(f"binary {self.head}")
            if self.on_cargo:
                self.on_cargo()
        if k in ("hygiene", "hygiene-apply"):
            return dl.Result(0, self.hygiene_out.encode())
        if k == "npm" and self.npm_hook is not None:
            hooked = self.npm_hook(argv)
            if hooked is not None:
                return hooked
        if k == "npm" and argv[1:3] == ["run", "build:extension"] and self.build_writes_dist:
            dist = self.repo / "extension" / "dist"
            dist.mkdir(parents=True, exist_ok=True)
            (dist / "build.txt").write_text(self.head)
            return dl.Result(0)
        if k == "binary":
            version = self.host_version or f"0.1.0+{self.head[:12]}"
            return dl.Result(0, frame({
                "type": self.hello_type, "protocol_version": self.protocol,
                "request_id": "x", "host_version": version, "configured": True,
                "config_issue": None,
            }))
        return dl.Result(0)


class Platform(unittest.TestCase):
    """Runs everywhere: deploy-live refuses anything but macOS before acting."""

    def test_refuses_other_platforms(self):
        for platform in ("win32", "linux", "cygwin"):
            calls = []

            def runner(argv, **_kw):
                calls.append(argv)
                return dl.Result(0)

            out = io.StringIO()
            code = dl.main(["--dry-run", "--repo", "/nowhere"], runner=runner, out=out,
                           home=Path("/nowhere"), platform=platform)
            self.assertEqual(code, 2, out.getvalue())
            self.assertIn("only runs on macOS", out.getvalue())
            self.assertIn(platform, out.getvalue())
            self.assertEqual(calls, [])


class ImportsEverywhere(unittest.TestCase):
    """Windows CI imports deploy-live to discover these tests; its signal
    module has no SIGHUP, so the trap takes only the signals it has."""

    def test_imports_without_sighup(self):
        import types
        fake = types.ModuleType("signal")
        for name in dir(signal):
            if name != "SIGHUP":
                setattr(fake, name, getattr(signal, name))
        loader = importlib.machinery.SourceFileLoader("deploy_live_nohup", str(SCRIPT))
        module = importlib.util.module_from_spec(
            importlib.util.spec_from_loader("deploy_live_nohup", loader))
        with mock.patch.dict(sys.modules, {"signal": fake, "deploy_live_nohup": module}):
            loader.exec_module(module)
        self.assertEqual(module.TRAPPED_SIGNALS, (signal.SIGTERM,))


DARWIN_ONLY = unittest.skipUnless(
    sys.platform == "darwin",
    "deploy-live is macOS-only (LaunchAgents, the macOS Chrome native-messaging manifest, "
    "target/debug/brauser without .exe)")


@DARWIN_ONLY
class Fixture(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="deploy-live-")).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.repo = self.tmp / "repo"
        (self.repo / "protocol" / "src").mkdir(parents=True)
        (self.repo / "protocol" / "src" / "generated.rs").write_text(
            "// generated\npub const PROTOCOL_VERSION: u32 = 8;\n")
        (self.repo / "node_modules").mkdir()
        (self.repo / ".git").mkdir()
        (self.repo / "package-lock.json").write_text("lock-v1")
        self.state = self.repo / ".git" / "deploy-live-state.json"
        self.record_lock("lock-v1")
        self.live = self.repo / "target" / "debug" / "brauser"
        self.staged = self.repo / "target" / "deploy-staging" / "debug" / "brauser"
        self.live.parent.mkdir(parents=True)
        self.live.write_text("old binary")
        (self.repo / "extension" / "dist").mkdir(parents=True)
        (self.repo / "extension" / "dist" / "build.txt").write_text(OLD)
        self.support = self.tmp / "support"
        (self.support / "runs").mkdir(parents=True)
        self.manifest = self.tmp / "com.rauser.brauser.json"
        self.write_manifest([f"chrome-extension://{EXT_ID}/"])
        self.home = self.tmp / "home"
        self.home.mkdir()
        self.runner = FakeRunner(self.repo)
        # The script this deploy runs as, and the repo's copy at HEAD; they
        # match unless a test's pull brings a new one.
        self.running_script = self.tmp / "running-deploy-live"
        self.running_script.write_text("deploy-live v1")
        (self.repo / "scripts" / "dev").mkdir(parents=True)
        (self.repo / "scripts" / "dev" / "deploy-live").write_text("deploy-live v1")
        self.reexecs: list[list[str]] = []
        real_replace = os.replace

        def replace(src, dst):
            self.runner.calls.append({"argv": ["os.replace", str(src), str(dst)], "cwd": None,
                                      "input": None, "stream": False})
            real_replace(src, dst)

        patcher = mock.patch.object(dl.os, "replace", side_effect=replace)
        patcher.start()
        self.addCleanup(patcher.stop)

    def record_lock(self, text):
        self.state.write_text(json.dumps(
            {"package_lock_sha256": hashlib.sha256(text.encode()).hexdigest()}))

    def recorded_lock(self):
        return json.loads(self.state.read_text())["package_lock_sha256"]

    def write_manifest(self, origins):
        self.manifest.write_text(json.dumps({
            "name": "com.rauser.brauser", "path": "/x/target/debug/brauser",
            "type": "stdio", "allowed_origins": origins,
        }))

    def add_run(self, run_id, state, **fields):
        d = self.support / "runs" / run_id
        d.mkdir()
        (d / "status.json").write_text(json.dumps({"version": 1, "run_id": run_id, "state": state,
                                                   **fields}))
        return d

    # Process IDs the fake process table says are running.
    alive: set = set()
    now = 1_800_000_000.0  # the fake clock, in Unix seconds
    on_sleep = None  # optional callable run on each fake sleep

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds
        if self.on_sleep:
            self.on_sleep()

    def deploy(self, *argv):
        out = io.StringIO()
        self.slept = getattr(self, "slept", [])
        code = dl.main(["--repo", str(self.repo), "--support-dir", str(self.support),
                        "--native-host-manifest", str(self.manifest), *argv],
                       runner=self.runner, out=out, home=self.home,
                       running_script=self.running_script, reexec=self.reexecs.append,
                       pid_alive=lambda pid: pid in self.alive, clock=lambda: self.now,
                       sleep=self.sleep)
        return code, out.getvalue()

    def kinds(self):
        return [self.runner.kind(c["argv"]) for c in self.runner.calls]

    def call(self, kind):
        found = [c for c in self.runner.calls if self.runner.kind(c["argv"]) == kind]
        self.assertTrue(found, f"no {kind} call in {self.kinds()}")
        return found[0]


class HappyPath(Fixture):
    def test_steps_run_in_order_and_succeed(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        acting = [self.runner.kind(c["argv"]) for c in self.runner.acting_calls()]
        self.assertEqual(acting, ["hygiene-apply", "pull", "cargo", "npm", "publish", "node",
                                  "binary"], out)
        kinds = self.kinds()
        self.assertEqual(kinds.count("node-preflight"), 2)
        self.assertLess(kinds.index("pull"), kinds.index("node-preflight"))
        self.assertLess(kinds.index("node-preflight"), kinds.index("cargo"))
        self.assertIn(f"{OLD[:12]}..{NEW[:12]}", out)
        self.assertIn(f"0.1.0+{NEW[:12]}", out)

    def test_lock_step_uses_locks_only_apply_on_the_repo(self):
        self.runner.hygiene_out = "  cleared /r/.git/index.lock\n"
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        argv = self.call("hygiene-apply")["argv"]
        self.assertTrue(argv[1].endswith("scripts/dev/worktree-hygiene"), argv)
        self.assertIn("--locks-only", argv)
        self.assertIn("--apply", argv)
        self.assertEqual(argv[argv.index("--repo") + 1], str(self.repo))
        self.assertNotIn("--clear-locks", argv)
        self.assertIn("cleared /r/.git/index.lock", out)
        # The lock step runs before the pull.
        kinds = self.kinds()
        self.assertLess(kinds.index("hygiene-apply"), kinds.index("pull"))

    def test_build_and_register_commands(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        argvs = [c["argv"] for c in self.runner.calls]
        self.assertIn(["git", "-C", str(self.repo), "pull", "--ff-only"], argvs)
        self.assertIn(["cargo", "build", "--locked", "-p", "brauser", "--target-dir",
                       str(self.repo / "target" / "deploy-staging")], argvs)
        self.assertIn(["npm", "run", "build:extension"], argvs)
        binary = str(self.repo / "target" / "debug" / "brauser")
        self.assertIn(["node", "scripts/register-dev-host.mjs", EXT_ID, binary], argvs)
        for c in self.runner.calls:
            if c["argv"][0] in ("cargo", "npm", "node"):
                self.assertEqual(Path(c["cwd"]), self.repo)

    def test_the_staged_binary_is_published_atomically(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        (publish,) = [c["argv"] for c in self.runner.calls if self.runner.kind(c["argv"]) == "publish"]
        _, src, dst = publish
        self.assertEqual(Path(dst), self.live)
        self.assertEqual(Path(src).parent, self.live.parent)  # same filesystem: rename is atomic
        self.assertNotEqual(Path(src), self.live)
        self.assertEqual(self.live.read_text(), f"binary {NEW}")
        self.assertEqual(self.staged.read_text(), f"binary {NEW}")  # kept for the next build
        self.assertEqual(sorted(p.name for p in self.live.parent.iterdir()), ["brauser"])

    def test_publish_keeps_the_staged_mode(self):
        def build():
            self.staged.chmod(0o755)
        self.runner.on_cargo = build
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertTrue(os.access(self.live, os.X_OK))

    def test_reload_hint_when_the_extension_changed(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("click Reload Brauser in the side panel", out)
        # The panel first; chrome://extensions only as the fallback.
        self.assertLess(out.index("side panel"), out.index("chrome://extensions only if"))

    def test_the_loaded_build_is_said_to_be_undetected_never_guessed(self):
        for argv in ((), ("--dry-run",)):
            code, out = self.deploy(*argv)
            self.assertEqual(code, 0, out)
            self.assertIn("loaded:    not detected", out)
            self.assertNotIn("loaded:    yes", out)

    def test_a_refused_deploy_keeps_the_reload_hint_until_one_completes(self):
        # The extension is rebuilt, then a late run refuses the deploy; the
        # retry at the same commit rebuilds an identical dist (brwr-dl2).
        self.runner.on_cargo = lambda: self.add_run("late-1", "running")
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        shutil.rmtree(self.support / "runs" / "late-1")
        self.runner.on_cargo = None
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("already up to date", out)
        self.assertIn("click Reload Brauser in the side panel", out)
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertNotIn("Reload Brauser", out)

    def test_no_reload_hint_when_the_extension_is_unchanged(self):
        self.runner.after_pull = OLD  # already up to date; build writes the same dist
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("already up to date", out)
        self.assertNotIn("Reload Brauser", out)

    def test_untracked_files_are_fine(self):
        # Untracked files (EXPLORER/) never reach the check: it asks git for
        # tracked changes only.
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        argv = self.call("status")["argv"]
        self.assertIn("--untracked-files=no", argv)

    def test_git_runs_without_optional_locks(self):
        self.assertEqual(dl.CHILD_ENV.get("GIT_OPTIONAL_LOCKS"), "0")


class Refusals(Fixture):
    def assert_refused(self, text, *argv):
        code, out = self.deploy(*argv)
        self.assertNotEqual(code, 0, out)
        self.assertIn(text, out)
        self.assertEqual(self.runner.acting_calls(), [], out)
        return out

    def test_linked_worktree(self):
        self.runner.git_dir = self.repo / ".git" / "worktrees" / "x"
        self.assert_refused("main worktree")

    def test_repo_is_a_subdirectory(self):
        self.runner.toplevel = self.tmp
        self.assert_refused("not the top")

    def test_not_on_main(self):
        self.runner.branch = "feat/x"
        self.assert_refused("on branch feat/x, not main")

    def test_detached_head(self):
        self.runner.branch = ""
        self.assert_refused("not on a branch")

    def test_tracked_changes(self):
        self.runner.status = " M host/src/lib.rs\n"
        self.assert_refused("tracked changes")

    def test_running_run(self):
        self.add_run("r-1", "running")
        out = self.assert_refused("r-1")
        self.assertIn("--allow-active-runs", out)

    def test_queued_run(self):
        self.add_run("r-2", "queued")
        self.assert_refused("r-2")

    def test_unreadable_status_is_refused(self):
        d = self.support / "runs" / "r-3"
        d.mkdir()
        (d / "status.json").write_text("{not json")
        self.assert_refused("r-3")

    def test_finished_runs_do_not_block(self):
        for i, state in enumerate(("succeeded", "failed", "canceled")):
            self.add_run(f"done-{i}", state)
        (self.support / "runs" / "no-status").mkdir()  # being created; no status yet
        code, out = self.deploy()
        self.assertEqual(code, 0, out)

    def test_missing_support_dir_does_not_block(self):
        shutil.rmtree(self.support)
        code, out = self.deploy()
        self.assertEqual(code, 0, out)

    def test_allow_active_runs(self):
        self.add_run("r-4", "running")
        code, out = self.deploy("--allow-active-runs")
        self.assertEqual(code, 0, out)
        self.assertIn("r-4", out)

    def test_run_started_during_the_build_stops_before_register(self):
        self.runner.on_cargo = lambda: self.add_run("late-1", "running")
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertIn("cargo", self.kinds())
        self.assertNotIn("publish", self.kinds())
        self.assertNotIn("node", self.kinds())
        self.assertNotIn("binary", self.kinds())
        self.assertIn("late-1", out)
        self.assertIn("--allow-active-runs", out)
        # The build went to the staging dir; the live binary is untouched.
        self.assertEqual(self.live.read_text(), "old binary")
        self.assertEqual(self.staged.read_text(), f"binary {NEW}")
        self.assertIn("untouched", out)
        self.assertIn("rerun", out)

    def test_run_started_just_before_the_publish_stops_it(self):
        answers = iter([[], [], ["late-9 (running)"]])
        with mock.patch.object(dl, "active_runs", side_effect=lambda *_a, **_k: next(answers)):
            code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertNotIn("publish", self.kinds())
        self.assertNotIn("node", self.kinds())
        self.assertEqual(self.live.read_text(), "old binary")
        self.assertEqual(sorted(p.name for p in self.live.parent.iterdir()), ["brauser"])
        self.assertIn("late-9", out)
        self.assertIn("untouched", out)

    def test_run_queued_during_the_build_stops_before_register(self):
        self.runner.on_cargo = lambda: self.add_run("late-2", "queued")
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertNotIn("node", self.kinds())
        self.assertIn("late-2", out)

    def test_allow_active_runs_covers_runs_started_during_the_build(self):
        self.runner.on_cargo = lambda: self.add_run("late-3", "running")
        code, out = self.deploy("--allow-active-runs")
        self.assertEqual(code, 0, out)
        self.assertIn("node", self.kinds())
        self.assertIn("late-3", out)

    def test_extension_id_unknown_stops_before_acting(self):
        self.manifest.unlink()
        self.assert_refused("--extension-id")

    def test_lock_step_failure_stops_before_pull(self):
        self.runner.fail.add("hygiene-apply")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertNotIn("pull", self.kinds())

    def test_pull_failure_stops(self):
        self.runner.fail.add("pull")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("pull failed", out)
        self.assertNotIn("cargo", self.kinds())

    def test_cargo_failure_stops(self):
        self.runner.fail.add("cargo")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("cargo build", out)
        self.assertEqual(self.live.read_text(), "old binary")
        self.assertNotIn("npm", self.kinds())
        self.assertNotIn("node", self.kinds())

    def test_register_failure_stops_before_hello(self):
        self.runner.fail.add("node")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertNotIn("binary", self.kinds())


class NodePreflight(Fixture):
    def test_unsupported_typescript_import_stops_before_builds(self):
        self.runner.node_version = "v22.15.0"
        self.runner.node_import_error = "ERR_UNKNOWN_FILE_EXTENSION: .ts"
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("v22.15.0", out)
        self.assertIn("ERR_UNKNOWN_FILE_EXTENSION", out)
        self.assertIn("nvm use 24.14.0", out)
        self.assertNotIn("cargo", self.kinds())
        self.assertNotIn("npm", self.kinds())
        self.assertEqual(self.live.read_text(), "old binary")
        self.assertEqual((self.repo / "extension" / "dist" / "build.txt").read_text(), OLD)

    def test_missing_node_stops_before_builds(self):
        self.runner.fail.add("node-preflight")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("Node preflight failed", out)
        self.assertIn("nvm install 24.14.0", out)
        self.assertNotIn("cargo", self.kinds())


class NpmCi(Fixture):
    def npm_calls(self):
        return [c["argv"] for c in self.runner.calls if c["argv"][0] == "npm"]

    def test_skipped_when_lockfile_unchanged_and_node_modules_present(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls(), [["npm", "run", "build:extension"]])
        self.assertIn("npm ci: skipped", out)

    def test_runs_when_the_pull_changed_package_lock(self):
        self.runner.lock_after_pull = "lock-v2"
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls()[0], ["npm", "ci", "--ignore-scripts"])
        self.assertIn("package-lock.json differs from the last npm ci", out)
        self.assertEqual(self.recorded_lock(), hashlib.sha256(b"lock-v2").hexdigest())

    def test_runs_when_nothing_is_recorded(self):
        self.state.unlink()
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls()[0][:2], ["npm", "ci"])
        self.assertIn("no npm ci recorded yet", out)
        self.assertEqual(self.recorded_lock(), hashlib.sha256(b"lock-v1").hexdigest())

    def test_runs_when_node_modules_is_missing(self):
        shutil.rmtree(self.repo / "node_modules")
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls()[0][:2], ["npm", "ci"])
        self.assertIn("node_modules is missing", out)

    def test_retry_after_a_failed_deploy_still_runs_npm_ci(self):
        # The first deploy pulls a new lockfile, then the build fails before npm ci.
        self.runner.lock_after_pull = "lock-v2"
        self.runner.fail.add("cargo")
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertEqual(self.npm_calls(), [])
        # The retry pulls nothing (HEAD is unchanged) but node_modules is stale.
        self.runner.fail.clear()
        self.runner.lock_after_pull = None
        self.runner.calls.clear()
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("already up to date", out)
        self.assertEqual(self.npm_calls()[0], ["npm", "ci", "--ignore-scripts"])
        self.assertEqual(self.recorded_lock(), hashlib.sha256(b"lock-v2").hexdigest())
        # And the next deploy skips it.
        self.runner.calls.clear()
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls(), [["npm", "run", "build:extension"]])

    def test_never_diffs_the_pull(self):
        self.runner.lock_after_pull = "lock-v2"
        self.deploy()
        self.assertNotIn("diff", self.kinds())

    def test_npm_ci_failure_stops(self):
        self.runner.lock_after_pull = "lock-v2"
        self.runner.fail.add("npm")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertNotIn("node", self.kinds())
        # The receipt goes before npm ci starts, so a failed install is not
        # mistaken for a finished one (brwr-dl1).
        self.assertNotIn("package_lock_sha256", json.loads(self.state.read_text()))

    def test_interrupted_npm_ci_is_rerun_even_when_node_modules_reappeared(self):
        # node_modules is missing, the lockfile matches the receipt, and npm ci
        # fails after partly recreating node_modules (brwr-dl1).
        shutil.rmtree(self.repo / "node_modules")

        def partial_install(argv):
            if argv[:2] == ["npm", "ci"]:
                (self.repo / "node_modules").mkdir(exist_ok=True)
                return dl.Result(1, b"", b"npm ci interrupted")
            return None
        self.runner.npm_hook = partial_install
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertTrue((self.repo / "node_modules").is_dir())
        self.runner.npm_hook = None
        self.runner.calls.clear()
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.npm_calls()[0], ["npm", "ci", "--ignore-scripts"])
        self.assertEqual(self.recorded_lock(), hashlib.sha256(b"lock-v1").hexdigest())


class SelfUpdate(Fixture):
    """A pull that changes deploy-live itself continues in the new copy."""

    def test_a_pull_that_changes_deploy_live_reexecs_the_new_copy(self):
        self.runner.script_after_pull = "deploy-live v2"
        code, out = self.deploy("--allow-active-runs")
        self.assertEqual(code, 0, out)
        self.assertEqual(len(self.reexecs), 1, out)
        script = str(self.repo / "scripts" / "dev" / "deploy-live")
        self.assertEqual(self.reexecs[0][:2], [sys.executable, script])
        args = self.reexecs[0][2:]
        self.assertIn("--allow-active-runs", args)
        self.assertEqual(args[-2:], ["--resumed-from", OLD])
        self.assertIn("deploy-live changed in this pull", out)
        # Nothing after the pull ran in the old copy.
        self.assertNotIn("cargo", self.kinds())
        self.assertNotIn("npm", self.kinds())

    def test_an_unchanged_deploy_live_carries_on(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(self.reexecs, [])

    def test_a_resumed_deploy_never_reexecs_and_reports_the_whole_range(self):
        self.runner.head = NEW
        self.runner.after_pull = NEW
        (self.repo / "scripts" / "dev" / "deploy-live").write_text("deploy-live v3")
        code, out = self.deploy("--resumed-from", OLD)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.reexecs, [])
        self.assertIn(f"commits:   {OLD[:12]}..{NEW[:12]}", out)
        self.assertIn("Reload Brauser", out)

    def test_a_resumed_deploy_does_not_pull_again(self):
        # The copy it runs is the one the first pull brought; a second pull
        # could bring yet another deploy-live it would not run (Codex, #229).
        self.runner.head = NEW
        self.runner.after_pull = "2222222222222222222222222222222222222222"
        self.runner.script_after_pull = "deploy-live v3"
        code, out = self.deploy("--resumed-from", OLD)
        self.assertEqual(code, 0, out)
        self.assertNotIn("pull", self.kinds())
        self.assertEqual(self.reexecs, [])
        self.assertIn(f"commits:   {OLD[:12]}..{NEW[:12]}", out)

    def test_resumed_from_must_be_a_full_commit(self):
        code, out = self.deploy("--resumed-from", "HEAD~1")
        self.assertEqual(code, 1, out)
        self.assertIn("--resumed-from", out)

    def test_a_dry_run_never_reexecs(self):
        self.runner.script_after_pull = "deploy-live v2"
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.reexecs, [])


class ExtensionId(Fixture):
    def test_single_origin(self):
        self.assertEqual(dl.extension_id_from_manifest(self.manifest), EXT_ID)

    def test_several_origins_are_ambiguous(self):
        self.write_manifest([f"chrome-extension://{EXT_ID}/", f"chrome-extension://{'a' * 32}/"])
        with self.assertRaisesRegex(dl.DeployError, "2 allowed_origins"):
            dl.extension_id_from_manifest(self.manifest)

    def test_no_origin(self):
        self.write_manifest([])
        with self.assertRaisesRegex(dl.DeployError, "0 allowed_origins"):
            dl.extension_id_from_manifest(self.manifest)

    def test_malformed_origin(self):
        self.write_manifest(["chrome-extension://NOT-AN-ID/"])
        with self.assertRaisesRegex(dl.DeployError, "not a Chrome extension origin"):
            dl.extension_id_from_manifest(self.manifest)

    def test_unreadable_manifest(self):
        self.manifest.write_text("{")
        with self.assertRaisesRegex(dl.DeployError, "cannot read"):
            dl.extension_id_from_manifest(self.manifest)

    def test_missing_manifest(self):
        self.manifest.unlink()
        with self.assertRaisesRegex(dl.DeployError, "--extension-id"):
            dl.extension_id_from_manifest(self.manifest)

    def test_flag_overrides_the_manifest(self):
        self.manifest.unlink()
        other = "b" * 32
        code, out = self.deploy("--extension-id", other)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.call("node")["argv"][2], other)
        self.assertEqual(self.call("binary")["argv"][1], f"chrome-extension://{other}/")

    def test_bad_flag_is_rejected(self):
        code, out = self.deploy("--extension-id", "xyz")
        self.assertNotEqual(code, 0, out)
        self.assertIn("not a Chrome extension ID", out)
        self.assertEqual(self.runner.acting_calls(), [])


class Hello(Fixture):
    def test_frame_is_little_endian_length_then_json(self):
        data = dl.hello_frame(8, "deploy-1")
        (length,) = struct.unpack("<I", data[:4])
        self.assertEqual(length, len(data) - 4)
        self.assertEqual(json.loads(data[4:]),
                         {"type": "hello", "protocol_version": 8, "request_id": "deploy-1"})

    def test_parse_frame(self):
        self.assertEqual(dl.parse_frame(frame({"a": 1}) + b"trailing"), {"a": 1})
        for bad in (b"", b"\x05\x00", struct.pack("<I", 10) + b"{}", struct.pack("<I", 2) + b"no"):
            with self.assertRaises(dl.DeployError):
                dl.parse_frame(bad)

    def test_protocol_version_from_generated_rs(self):
        self.assertEqual(dl.protocol_version(self.repo), 8)
        (self.repo / "protocol" / "src" / "generated.rs").write_text("nothing here\n")
        with self.assertRaisesRegex(dl.DeployError, "PROTOCOL_VERSION"):
            dl.protocol_version(self.repo)

    def test_binary_gets_the_origin_and_one_hello_frame(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        c = self.call("binary")
        self.assertEqual(c["argv"], [str(self.repo / "target/debug/brauser"),
                                     f"chrome-extension://{EXT_ID}/"])
        sent = dl.parse_frame(c["input"])
        self.assertEqual(sent["type"], "hello")
        self.assertEqual(sent["protocol_version"], 8)
        self.assertTrue(sent["request_id"])

    def test_verify_accepts_the_new_build(self):
        reply = {"type": "hello_result", "protocol_version": 8, "host_version": f"0.1.0+{NEW[:12]}"}
        self.assertEqual(dl.verify_hello(reply, 8, NEW), f"0.1.0+{NEW[:12]}")

    def test_protocol_mismatch_fails(self):
        self.runner.protocol = 7
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("protocol_version 7", out)

    def test_old_build_fails(self):
        self.runner.host_version = f"0.1.0+{OLD[:12]}"
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn(f"+{NEW[:12]}", out)

    def test_dirty_build_fails(self):
        self.runner.host_version = f"0.1.0+{NEW[:12]}-dirty"
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("-dirty", out)

    def test_error_reply_fails(self):
        self.runner.hello_type = "error"
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("error", out)

    def test_binary_failure(self):
        self.runner.fail.add("binary")
        code, out = self.deploy()
        self.assertNotEqual(code, 0, out)
        self.assertIn("hello", out)


class DryRun(Fixture):
    def test_prints_steps_and_acts_on_nothing(self):
        before = (self.repo / "extension" / "dist" / "build.txt").read_text()
        state = self.state.read_text()
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.runner.acting_calls(), [], out)
        self.assertNotIn("os.replace", self.kinds())
        self.assertEqual(self.state.read_text(), state)
        self.assertEqual(self.live.read_text(), "old binary")
        self.assertFalse(self.staged.exists())
        self.assertIn("target/deploy-staging", out)
        for text in ("git -C", "pull --ff-only", "cargo build --locked -p brauser",
                     "npm run build:extension", "register-dev-host.mjs", EXT_ID, "hello"):
            self.assertIn(text, out)
        self.assertIn("dry run", out)
        self.assertEqual((self.repo / "extension" / "dist" / "build.txt").read_text(), before)

    def test_summary_says_npm_ci_would_run(self):
        shutil.rmtree(self.repo / "node_modules")
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("npm ci: would run (node_modules is missing)", out)
        self.assertNotIn("npm ci: ran", out)

    def test_dry_run_says_npm_ci_would_run_for_a_changed_lockfile(self):
        (self.repo / "package-lock.json").write_text("lock-v2")
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("npm ci: would run (package-lock.json differs from the last npm ci)", out)

    def test_reports_locks_without_clearing_them(self):
        self.runner.hygiene_out = "  clear   /r/.git/index.lock: stale (dry run)\n"
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        argv = self.call("hygiene")["argv"]
        self.assertIn("--locks-only", argv)
        self.assertNotIn("--apply", argv)
        self.assertIn("stale (dry run)", out)

    def test_still_refuses(self):
        self.runner.branch = "feat/x"
        code, out = self.deploy("--dry-run")
        self.assertNotEqual(code, 0, out)


def iso(unix: float) -> str:
    import datetime
    return datetime.datetime.fromtimestamp(unix, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class ExpectedSha(Fixture):
    """--expected-sha deploys exactly that commit of origin/main, or nothing."""

    def test_deploys_the_expected_commit_instead_of_pulling(self):
        code, out = self.deploy("--expected-sha", NEW[:12])
        self.assertEqual(code, 0, out)
        kinds = self.kinds()
        self.assertNotIn("pull", kinds)
        self.assertLess(kinds.index("fetch"), kinds.index("merge"))
        self.assertLess(kinds.index("merge"), kinds.index("cargo"))
        self.assertEqual(self.call("merge")["argv"][-2:], ["--ff-only", NEW])
        self.assertIn(f"0.1.0+{NEW[:12]}", out)
        self.assertIn(f"expected {NEW[:12]}", out)

    def test_main_moved_past_it_deploys_exactly_the_expected_commit(self):
        code, out = self.deploy("--expected-sha", MID)
        self.assertEqual(code, 0, out)
        self.assertEqual(self.call("merge")["argv"][-1], MID)
        self.assertIn(f"0.1.0+{MID[:12]}", out)
        self.assertIn(f"{OLD[:12]}..{MID[:12]}", out)
        self.assertIn(f"origin main is at {NEW[:12]}", out)

    def test_a_commit_not_on_origin_main_is_refused_before_building(self):
        self.runner.history = [OLD, NEW]
        code, out = self.deploy("--expected-sha", MID[:12])
        self.assertEqual(code, 1, out)
        self.assertIn(MID[:12], out)
        self.assertNotIn("merge", self.kinds())
        self.assertNotIn("cargo", self.kinds())
        self.assertEqual(self.live.read_text(), "old binary")

    def test_a_checkout_already_past_the_expected_commit_is_refused(self):
        self.runner.head = NEW
        code, out = self.deploy("--expected-sha", MID[:12])
        self.assertEqual(code, 1, out)
        self.assertIn("newer", out)
        self.assertNotIn("merge", self.kinds())
        self.assertNotIn("cargo", self.kinds())

    def test_already_at_the_expected_commit_builds_it(self):
        self.runner.head = MID
        code, out = self.deploy("--expected-sha", MID[:12])
        self.assertEqual(code, 0, out)
        self.assertNotIn("merge", self.kinds())
        self.assertIn(f"0.1.0+{MID[:12]}", out)

    def test_a_ref_named_like_the_sha_is_refused(self):
        # A branch or tag called "abcdef0" wins over the short SHA in rev-parse.
        self.runner.refs = {NEW[:7]: MID}
        code, out = self.deploy("--expected-sha", NEW[:7])
        self.assertEqual(code, 1, out)
        self.assertIn("resolves to", out)
        self.assertNotIn("merge", self.kinds())
        self.assertNotIn("cargo", self.kinds())

    def test_a_malformed_sha_is_refused_before_acting(self):
        for bad in ("xyz", "abc", "ABCDEF0", NEW + "0"):
            code, out = self.deploy("--expected-sha", bad)
            self.assertEqual(code, 1, out)
            self.assertIn("--expected-sha", out)
            self.assertEqual(self.runner.acting_calls(), [], out)

    def test_a_resumed_deploy_checks_head_is_the_expected_commit(self):
        self.runner.head = NEW
        code, out = self.deploy("--expected-sha", MID[:12], "--resumed-from", OLD)
        self.assertEqual(code, 1, out)
        self.assertIn(MID[:12], out)
        self.assertNotIn("cargo", self.kinds())


class TruthfulDryRun(Fixture):
    def test_reports_local_and_origin_main(self):
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("git -C", " ".join(self.call("ls-remote")["argv"]))
        self.assertIn(f"local HEAD {OLD[:12]}", out)
        self.assertIn(f"origin main {NEW[:12]}", out)
        self.assertIn(f"would pull {OLD[:12]}..{NEW[:12]}", out)
        self.assertNotIn("already up to date", out)
        self.assertEqual(self.runner.acting_calls(), [], out)

    def test_up_to_date_only_when_origin_main_is_head(self):
        self.runner.remote_head = OLD
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn(f"up to date with origin main at {OLD[:12]}", out)

    def test_an_unreachable_origin_is_said_not_guessed(self):
        self.runner.fail.add("ls-remote")
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("origin main: unavailable", out)
        self.assertNotIn("up to date", out)

    def test_dry_run_with_head_expected_but_not_origin_tip_is_conditional(self):
        code, out = self.deploy("--dry-run", "--expected-sha", OLD[:12])
        self.assertEqual(code, 0, out)
        self.assertIn("only if it is on origin/main", out)

    def test_dry_run_with_expected_sha_names_what_it_would_deploy(self):
        code, out = self.deploy("--dry-run", "--expected-sha", MID[:12])
        self.assertEqual(code, 0, out)
        self.assertNotIn("fetch", [self.runner.kind(c["argv"]) for c in self.runner.acting_calls()])
        self.assertIn(f"would deploy {MID[:12]}", out)
        self.assertEqual(self.runner.acting_calls(), [], out)

    def test_dry_run_does_not_claim_the_extension_is_unchanged(self):
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertNotIn("unchanged", out)
        self.assertIn("known after the build", out)


class StaleRuns(Fixture):
    """A run whose service is gone cannot be ended by the restart; it is
    reported, with what will recover it, and does not hold the deploy."""

    def test_a_running_run_whose_service_is_gone_is_reported_and_does_not_block(self):
        self.add_run("r-dead", "running", service_pid=4242, heartbeat_at=iso(self.now - 720))
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("r-dead", out)
        self.assertIn("stale", out)
        self.assertIn("service pid 4242 is not running", out)
        self.assertIn("heartbeat 12m ago", out)
        self.assertIn("service_restarted", out)

    def test_a_queued_run_whose_service_is_gone_does_not_block(self):
        self.add_run("q-dead", "queued", service_pid=4243)
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertIn("q-dead", out)
        self.assertIn("stale", out)

    def test_a_run_whose_service_is_alive_still_blocks_and_says_its_heartbeat(self):
        self.alive = {4244}
        self.add_run("r-live", "running", service_pid=4244, heartbeat_at=iso(self.now - 3))
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertIn("r-live", out)
        self.assertIn("heartbeat 3s ago", out)
        self.assertEqual(self.runner.acting_calls(), [], out)

    def test_a_run_naming_no_service_pid_blocks(self):
        self.add_run("r-unknown", "running")
        code, out = self.deploy()
        self.assertEqual(code, 1, out)
        self.assertIn("r-unknown", out)

    def test_dry_run_reports_stale_runs(self):
        self.add_run("r-dead", "running", service_pid=4242)
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("stale", out)
        self.assertIn("r-dead", out)

    def test_pid_alive_default(self):
        self.assertTrue(dl.pid_alive(os.getpid()))
        self.assertFalse(dl.pid_alive(0))
        self.assertFalse(dl.pid_alive(-1))


class Drain(Fixture):
    """--drain pauses new runs after the builds, waits (bounded) for the
    admitted ones, publishes, and always lifts the pause."""

    def setUp(self):
        super().setUp()
        self.live.write_text("old binary that knows deploy-drain.json")

    def marker(self):
        return self.support / "deploy-drain.json"

    def test_waits_for_active_runs_then_publishes_and_lifts_the_pause(self):
        self.alive = {77}
        self.add_run("r-1", "running", service_pid=77, heartbeat_at=iso(self.now))
        seen = []

        def finish():
            seen.append(json.loads(self.marker().read_text()))
            status = self.support / "runs" / "r-1" / "status.json"
            status.write_text(json.dumps({"version": 1, "run_id": "r-1", "state": "succeeded"}))
        self.on_sleep = finish
        code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        self.assertTrue(seen, out)
        self.assertGreater(seen[0]["until_unix"], self.now - sum(self.slept))
        self.assertEqual(seen[0]["version"], 1)
        self.assertFalse(self.marker().exists())
        self.assertEqual(self.live.read_text(), f"binary {NEW}")
        kinds = self.kinds()
        self.assertLess(kinds.index("cargo"), kinds.index("publish"))
        self.assertIn("r-1", out)

    def test_the_pause_starts_after_the_builds(self):
        states = []
        self.runner.on_cargo = lambda: states.append(self.marker().exists())
        code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        self.assertEqual(states, [False])
        self.assertFalse(self.marker().exists())

    def test_a_run_that_does_not_finish_in_time_refuses_and_lifts_the_pause(self):
        self.alive = {78}
        self.add_run("r-slow", "running", service_pid=78)
        code, out = self.deploy("--drain", "--drain-timeout", "60")
        self.assertEqual(code, 1, out)
        self.assertIn("r-slow", out)
        self.assertIn("60s", out)
        self.assertFalse(self.marker().exists())
        self.assertEqual(self.live.read_text(), "old binary that knows deploy-drain.json")
        self.assertNotIn("publish", self.kinds())
        self.assertLessEqual(sum(self.slept), 60 + dl.DRAIN_POLL)

    def test_the_pause_is_lifted_when_a_later_step_fails(self):
        self.runner.fail.add("node")
        code, out = self.deploy("--drain")
        self.assertEqual(code, 1, out)
        self.assertFalse(self.marker().exists())

    def test_stale_runs_do_not_hold_the_drain(self):
        self.add_run("r-dead", "running", service_pid=4242)
        code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.slept, [dl.DRAIN_POLL])  # only the settle after pausing

    def test_settles_one_poll_after_pausing_before_the_first_scan(self):
        # A start_run that passed the pause check just before the marker
        # landed may still be writing its status.json; wait it out.
        scans = []
        real = dl.active_runs

        def scan(*a, **k):
            scans.append((self.marker().exists(), sum(self.slept)))
            return real(*a, **k)
        with mock.patch.object(dl, "active_runs", side_effect=scan):
            code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        paused = [slept for exists, slept in scans if exists]
        self.assertTrue(paused, scans)
        self.assertGreaterEqual(paused[0], dl.DRAIN_POLL)

    def test_a_run_admitted_as_the_pause_lands_is_waited_for(self):
        self.alive = {91}
        added = []

        def admit():
            if not added:  # appears during the settle, after the marker
                added.append(self.add_run("r-race", "running", service_pid=91))
            else:
                (self.support / "runs" / "r-race" / "status.json").write_text(
                    json.dumps({"version": 1, "run_id": "r-race", "state": "succeeded"}))
        self.on_sleep = admit
        code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        self.assertIn("waiting for: r-race", out)

    def test_dry_run_writes_no_marker(self):
        self.alive = {79}
        self.add_run("r-1", "running", service_pid=79)
        code, out = self.deploy("--dry-run", "--drain")
        self.assertEqual(code, 0, out)
        self.assertFalse(self.marker().exists())
        self.assertIn("would pause new runs", out)
        self.assertEqual(self.slept, [])

    def test_a_missing_support_dir_needs_no_pause(self):
        shutil.rmtree(self.support)
        code, out = self.deploy("--drain")
        self.assertEqual(code, 0, out)
        self.assertFalse(self.support.exists())

    def test_marker_name_and_cap_match_the_service(self):
        source = (SCRIPT.parents[2] / "host" / "src" / "service" / "drain.rs").read_text(
            encoding="utf-8").replace("\r\n", "\n")
        self.assertIn(f'pub const DRAIN_FILE: &str = "{dl.DRAIN_FILE}";', source)
        self.assertIn(f"pub const DRAIN_MAX: Duration = Duration::from_secs({dl.DRAIN_MAX});",
                      source)
        self.assertIn("until_unix: u64", source)

    def test_a_live_build_that_ignores_the_pause_refuses_drain_before_acting(self):
        self.live.write_text("old binary from before the pause")
        code, out = self.deploy("--drain")
        self.assertEqual(code, 1, out)
        self.assertIn("predates", out)
        self.assertEqual(self.runner.acting_calls(), [], out)

    def test_drain_timeout_is_bounded(self):
        code, out = self.deploy("--drain", "--drain-timeout", str(dl.DRAIN_TIMEOUT_MAX))
        self.assertEqual(code, 0, out)
        self.assertLessEqual(dl.DRAIN_TIMEOUT_MAX + dl.DRAIN_GRACE, dl.DRAIN_MAX)
        for bad in ("0", "-5", str(dl.DRAIN_TIMEOUT_MAX + 1), "x"):
            code, out = self.deploy("--drain", "--drain-timeout", bad)
            self.assertNotEqual(code, 0, out)


class RemoteMain(Fixture):
    """git ls-remote is bounded and never prompts."""

    def test_ls_remote_has_a_timeout_and_no_terminal_prompt(self):
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        call = self.call("ls-remote")
        self.assertEqual(call["timeout"], dl.LS_REMOTE_TIMEOUT)
        self.assertEqual(call["env"]["GIT_TERMINAL_PROMPT"], "0")

    def test_a_timed_out_ls_remote_is_unavailable_not_a_hang(self):
        self.runner.ls_remote_result = dl.Result(124, b"", b"timed out after 30.0s")
        code, out = self.deploy("--dry-run")
        self.assertEqual(code, 0, out)
        self.assertIn("origin main: unavailable (timed out after 30.0s)", out)

    def test_system_runner_adds_env_and_gives_captured_calls_no_stdin(self):
        res = dl.system_runner(
            [sys.executable, "-c",
             "import os, sys; print(os.environ['DL_X'], os.environ['GIT_OPTIONAL_LOCKS'],"
             " repr(sys.stdin.read()))"],
            cwd=self.tmp, env={"DL_X": "y"}, timeout=30)
        self.assertEqual(res.returncode, 0, res.err)
        self.assertEqual(res.text.split(), ["y", "0", "''"])
        self.assertNotIn("DL_X", dl.CHILD_ENV)


class DryRunAncestry(Fixture):
    """A dry run with --expected-sha checks it against the local origin/main
    ref (read-only) and says so as of the last fetch."""

    def test_on_origin_main_as_of_the_last_fetch(self):
        code, out = self.deploy("--dry-run", "--expected-sha", MID)
        self.assertEqual(code, 0, out)
        self.assertIn("on the local origin/main ref: yes as of the last fetch", out)
        self.assertIn(f"would deploy {MID[:12]} if it is on origin/main (yes as of the last "
                      "fetch)", out)
        self.assertEqual(self.runner.acting_calls(), [], out)

    def test_not_on_origin_main_as_of_the_last_fetch(self):
        self.runner.remote_head = MID  # the local origin/main stops before NEW
        code, out = self.deploy("--dry-run", "--expected-sha", NEW)
        self.assertEqual(code, 0, out)
        self.assertIn("not as of the last fetch; the fetch decides", out)

    def test_an_unknown_commit_is_said_unknown(self):
        code, out = self.deploy("--dry-run", "--expected-sha", "9" * 40)
        self.assertEqual(code, 0, out)
        self.assertIn("unknown: not a commit here until a fetch brings it", out)


class DrainSignals(Fixture):
    """SIGTERM or SIGHUP (a closed terminal) still lifts the pause: a real
    child deploy-live, paused mid-drain in its own temp dir, is killed."""

    def run_child(self, sig):
        child = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--drain-child"],
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            ready = json.loads(child.stdout.readline() or "{}")
            self.assertIn("marker", ready, child.stderr.read() if child.poll() is not None else "")
            marker = Path(ready["marker"])
            self.addCleanup(shutil.rmtree, ready["tmp"], True)
            self.assertTrue(marker.exists())
            child.send_signal(sig)
            rest, err = child.communicate(timeout=30)
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
        self.assertEqual(child.returncode, 0, err)
        result = json.loads(rest)
        return marker, result

    def test_sigterm_and_sighup_lift_the_pause_and_exit_128_plus_the_signal(self):
        for sig in (signal.SIGTERM, signal.SIGHUP):
            with self.subTest(signal=sig.name):
                marker, result = self.run_child(sig)
                self.assertFalse(marker.exists(), result["out"])
                self.assertEqual(result["code"], 128 + sig, result["out"])
                self.assertIn(f"stopped by {sig.name}", result["out"])
                self.assertIn("lifted the pause on new runs", result["out"])
                self.assertNotIn("publish", result["kinds"])

    def test_the_trap_is_removed_after_the_deploy(self):
        before = {sig: signal.getsignal(sig) for sig in dl.TRAPPED_SIGNALS}
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual({sig: signal.getsignal(sig) for sig in dl.TRAPPED_SIGNALS}, before)


class TrapHardening(Fixture):
    """The trap's edges: a signal while the marker is written, a second
    signal or a dead terminal during cleanup, and nohup's SIG_IGN."""

    def setUp(self):
        super().setUp()
        self.live.write_text("old binary that knows deploy-drain.json")
        self.marker = self.support / "deploy-drain.json"
        self.before = {sig: signal.getsignal(sig) for sig in dl.TRAPPED_SIGNALS}
        self.addCleanup(lambda: [signal.signal(sig, h) for sig, h in self.before.items()])

    def leftovers(self):
        return sorted(p.name for p in self.support.iterdir() if "drain" in p.name)

    def test_a_signal_just_after_the_marker_lands_still_lifts_it(self):
        real = os.replace

        def replace(src, dst):
            real(src, dst)
            if str(dst).endswith("deploy-drain.json"):
                raise dl.Signalled(signal.SIGTERM)
        with mock.patch.object(dl.os, "replace", side_effect=replace):
            code, out = self.deploy("--drain")
        self.assertEqual(code, 128 + signal.SIGTERM, out)
        self.assertEqual(self.leftovers(), [], out)

    def test_a_signal_while_the_marker_is_written_leaves_no_temporary(self):
        def replace(src, dst):
            raise dl.Signalled(signal.SIGHUP)
        with mock.patch.object(dl.os, "replace", side_effect=replace):
            with self.assertRaises(dl.Signalled):
                dl.write_drain(self.support, self.now + 60, self.now)
        self.assertEqual(self.leftovers(), [])

    def test_once_stopping_further_signals_are_ignored(self):
        dl.trap_signals()
        with self.assertRaises(dl.Signalled):
            dl._raise_signalled(signal.SIGTERM, None)
        for sig in dl.TRAPPED_SIGNALS:
            self.assertEqual(signal.getsignal(sig), signal.SIG_IGN)

    def test_a_dead_terminal_during_cleanup_still_lifts_the_pause_and_exits_129(self):
        class DeadTerminal(io.StringIO):
            dead = False

            def write(self, text):
                if self.dead:
                    raise OSError(5, "Input/output error")
                return super().write(text)

        out = DeadTerminal()

        def hang_up():
            out.dead = True
            raise dl.Signalled(signal.SIGHUP)
        self.alive = {4321}
        self.add_run("r-hold", "running", service_pid=4321)
        self.on_sleep = hang_up
        self.slept = []
        code = dl.main(["--repo", str(self.repo), "--support-dir", str(self.support),
                        "--native-host-manifest", str(self.manifest), "--drain"],
                       runner=self.runner, out=out, home=self.home,
                       running_script=self.running_script, reexec=self.reexecs.append,
                       pid_alive=lambda pid: pid in self.alive, clock=lambda: self.now,
                       sleep=self.sleep)
        self.assertEqual(code, 128 + signal.SIGHUP)
        self.assertFalse(self.marker.exists())

    def test_an_inherited_sig_ign_is_kept_so_nohup_still_works(self):
        signal.signal(signal.SIGHUP, signal.SIG_IGN)
        seen = []
        self.runner.on_cargo = lambda: seen.append(
            (signal.getsignal(signal.SIGHUP), signal.getsignal(signal.SIGTERM)))
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assertEqual(seen[0][0], signal.SIG_IGN)
        self.assertIs(seen[0][1], dl._raise_signalled)
        self.assertEqual(signal.getsignal(signal.SIGHUP), signal.SIG_IGN)


class NetworkGit(Fixture):
    """Every git call that reaches the remote is bounded and never prompts,
    over HTTPS or ssh."""

    def assert_bounded(self, kind):
        call = self.call(kind)
        self.assertIsNotNone(call["timeout"], kind)
        self.assertEqual(call["env"]["GIT_TERMINAL_PROMPT"], "0", kind)
        self.assertIn("BatchMode=yes", call["env"]["GIT_SSH_COMMAND"], kind)

    def test_pull_ls_remote_and_fetch_are_bounded_and_non_interactive(self):
        code, out = self.deploy()
        self.assertEqual(code, 0, out)
        self.assert_bounded("pull")
        self.assert_bounded("ls-remote")
        self.runner.calls.clear()
        self.runner.head = OLD
        code, out = self.deploy("--expected-sha", MID)
        self.assertEqual(code, 0, out)
        self.assert_bounded("fetch")

    def test_a_configured_ssh_command_is_kept_and_made_batch(self):
        with mock.patch.dict(os.environ, {"GIT_SSH_COMMAND": "ssh -i /k"}):
            env = dl.network_env()
        self.assertEqual(env["GIT_SSH_COMMAND"], "ssh -i /k -o BatchMode=yes")


def _drain_child() -> None:
    """--drain-child: start a --drain deploy that waits on a run which never
    finishes, print the marker path, and block in the drain's sleep until
    the parent test signals; then print the outcome as JSON."""
    import time as real_time
    case = Drain("test_drain_timeout_is_bounded")
    case.setUp()
    case.alive = {4321}
    case.add_run("r-hold", "running", service_pid=4321)
    told = []

    def block():
        if not told:
            told.append(True)
            print(json.dumps({"marker": str(case.marker()), "tmp": str(case.tmp)}), flush=True)
        real_time.sleep(60)
    case.on_sleep = block
    code, out = case.deploy("--drain")
    print(json.dumps({"code": code, "out": out, "kinds": case.kinds()}), flush=True)


class DefaultRepo(Fixture):
    def test_main_worktree_of_the_scripts_repo(self):
        self.assertEqual(dl.default_repo(self.runner, Path("/anything")), self.repo)

    def test_default_paths_come_from_home(self):
        self.assertEqual(dl.default_support_dir(self.home),
                         self.home / "Library/Application Support/Brauser")
        self.assertEqual(dl.default_manifest(self.home),
                         self.home / "Library/Application Support/Google/Chrome/NativeMessagingHosts"
                         / "com.rauser.brauser.json")


if __name__ == "__main__":
    if sys.argv[1:] == ["--drain-child"]:
        _drain_child()
    else:
        unittest.main()
