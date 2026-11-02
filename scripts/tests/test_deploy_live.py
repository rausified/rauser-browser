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
import struct
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

    # Calls whose first words match one of these act on the system.
    ACTING = ("pull", "cargo", "npm", "node", "binary", "hygiene-apply", "publish")

    def kind(self, argv):
        if argv[0] == "os.replace":  # recorded by Fixture's os.replace wrapper
            return "publish" if argv[2].endswith("target/debug/brauser") else "os.replace"
        if argv[0] == "git":
            return argv[3] if argv[1] == "-C" else argv[1]
        if argv[0] == sys.executable and argv[1].endswith("worktree-hygiene"):
            return "hygiene-apply" if "--apply" in argv else "hygiene"
        if argv[0].endswith("target/debug/brauser"):
            return "binary"
        return argv[0]

    def acting_calls(self):
        out = []
        for c in self.calls:
            k = self.kind(c["argv"])
            if k in self.ACTING:
                out.append(c)
        return out

    def __call__(self, argv, *, cwd, input=None, stream=False, timeout=None):
        argv = [str(a) for a in argv]
        self.calls.append({"argv": argv, "cwd": cwd, "input": input, "stream": stream})
        k = self.kind(argv)
        if k in self.fail:
            return dl.Result(1, b"", f"{k} failed".encode())
        if k == "rev-parse" and "--show-toplevel" in argv:
            text = f"{self.toplevel}\n{self.git_dir}\n{self.common_dir}\n"
            return dl.Result(0, text.encode())
        if k == "rev-parse" and argv[-1] == "HEAD":
            return dl.Result(0, (self.head + "\n").encode())
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
            return dl.Result(0, b"Updating\n")
        if k == "diff":
            return dl.Result(0, self.changed_files.encode())
        if k == "cargo":
            target = Path(argv[argv.index("--target-dir") + 1]) if "--target-dir" in argv \
                else self.repo / "target"
            (target / "debug").mkdir(parents=True, exist_ok=True)
            (target / "debug" / "brauser").write_text(f"binary {self.head}")
            if self.on_cargo:
                self.on_cargo()
        if k in ("hygiene", "hygiene-apply"):
            return dl.Result(0, self.hygiene_out.encode())
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

    def add_run(self, run_id, state):
        d = self.support / "runs" / run_id
        d.mkdir()
        (d / "status.json").write_text(json.dumps({"version": 1, "run_id": run_id, "state": state}))
        return d

    def deploy(self, *argv):
        out = io.StringIO()
        code = dl.main(["--repo", str(self.repo), "--support-dir", str(self.support),
                        "--native-host-manifest", str(self.manifest), *argv],
                       runner=self.runner, out=out, home=self.home)
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
        self.assertIn("Reload Brauser at chrome://extensions", out)

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
        with mock.patch.object(dl, "active_runs", side_effect=lambda _s: next(answers)):
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
        self.assertEqual(self.recorded_lock(), hashlib.sha256(b"lock-v1").hexdigest())


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
    unittest.main()
