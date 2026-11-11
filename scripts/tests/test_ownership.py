"""Tests for scripts/ownership.py (scripts/dev/overlap-check, scripts/dev/who-owns).

Every source (GitHub, bd, git worktrees, Orca) is a fixture: nothing here
touches the network, a real queue or a real Orca install.
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

import ownership  # noqa: E402


def pr(number, branch, files=(), body="Owner: claude session aaaa1111\r\nLane / coordinator: lane X\r\n", title="t",
       state="OPEN"):
    return {"number": number, "title": title, "headRefName": branch, "body": body, "state": state,
            "url": f"https://github.example/o/r/pull/{number}", "files": [{"path": f} for f in files]}


def bead(id_, status="in_progress", **fields):
    return {"id": id_, "title": f"title of {id_}", "status": status, **fields}


class FakeSources(ownership.Sources):
    """Fixture data in place of gh, bd, git and orca."""

    def __init__(self, prs=(), closed=(), beads=(), worktrees=(), orca=None, changed=(), branch="me/branch",
                 fail=()):
        self._prs, self._closed, self._beads = list(prs), list(closed), list(beads)
        self._worktrees, self._orca, self._changed, self._branch = list(worktrees), orca, list(changed), branch
        self.fail = set(fail)

    def _maybe_fail(self, what):
        if what in self.fail:
            raise ownership.SourceError(f"{what} is unavailable")

    def open_prs(self):
        self._maybe_fail("gh")
        return self._prs

    def prs_for_branch(self, branch):
        self._maybe_fail("gh")
        return [p for p in self._prs + self._closed if p["headRefName"] == branch]

    def pr(self, number):
        self._maybe_fail("gh")
        return next((p for p in self._prs + self._closed if p["number"] == number), None)

    def beads(self):
        self._maybe_fail("bd")
        return self._beads

    def worktrees(self):
        return self._worktrees

    def orca_worktrees(self):
        return self._orca

    def changed_files(self, base):
        return self._changed

    def current_branch(self):
        return self._branch


def run(main, argv, sources):
    out = io.StringIO()
    code = main(argv, sources=sources, out=out)
    return code, out.getvalue()


class OwnerLineTests(unittest.TestCase):
    def test_reads_plain_bold_and_listed_owner_lines(self):
        self.assertEqual(ownership.owner_line("Owner: claude session 1\n"), "claude session 1")
        self.assertEqual(ownership.owner_line("- **Owner:** codex in work/rb-x\r\n"), "codex in work/rb-x")

    def test_template_placeholders_and_comments_are_not_owners(self):
        self.assertIsNone(ownership.owner_line("Owner: <!-- the session -->\n"))
        self.assertIsNone(ownership.owner_line("Owner: <your session>\n"))
        self.assertIsNone(ownership.owner_line("```\nOwner: inside a fence\n```\n"))
        self.assertIsNone(ownership.owner_line(None))


class OverlapCheckTests(unittest.TestCase):
    def test_lists_prs_touching_the_paths_with_their_owner(self):
        sources = FakeSources(prs=[pr(7, "a/one", ["host/src/config.rs", "README.md"]), pr(8, "b/two", ["x.md"])])
        code, text = run(ownership.overlap_main, ["host/src/config.rs"], sources)
        self.assertEqual(code, 0)
        self.assertIn("OVERLAP", text)
        self.assertIn("PR #7 a/one", text)
        self.assertIn("Owner: claude session aaaa1111", text)
        self.assertIn("host/src/config.rs", text)
        self.assertNotIn("#8", text)

    def test_a_directory_path_matches_files_beneath_it(self):
        sources = FakeSources(prs=[pr(7, "a/one", ["scripts/dev/queue"])])
        _, text = run(ownership.overlap_main, ["scripts/dev/"], sources)
        self.assertIn("PR #7", text)
        _, text = run(ownership.overlap_main, ["scripts/de"], sources)
        self.assertNotIn("PR #7", text)

    def test_a_pr_with_no_owner_line_says_so(self):
        sources = FakeSources(prs=[pr(7, "a/one", ["f.py"], body="no owner here")])
        _, text = run(ownership.overlap_main, ["f.py"], sources)
        self.assertIn("Owner: (none named)", text)

    def test_in_progress_beads_mentioning_a_path_and_beads_a_matching_pr_cites(self):
        sources = FakeSources(
            prs=[pr(7, "lane/brwr-c3", ["host/src/x.rs"], title="Fix x (brwr-d4)")],
            beads=[bead("brwr-a1", description="change host/src/x.rs", assignee="session one"),
                   bead("brwr-b2", description="unrelated"),
                   bead("brwr-c3", notes="branch named"), bead("brwr-d4"),
                   bead("brwr-e5", status="open", description="host/src/x.rs")],
        )
        _, text = run(ownership.overlap_main, ["host/src/x.rs"], sources)
        self.assertIn("bead brwr-a1 (in_progress)", text)
        self.assertIn("assignee: session one", text)
        self.assertIn("mentions host/src/x.rs", text)
        self.assertIn("bead brwr-c3", text)
        self.assertIn("bead brwr-d4", text)
        self.assertIn("cited by PR #7", text)
        self.assertNotIn("brwr-b2", text)
        self.assertNotIn("brwr-e5", text)  # not in progress

    def test_branch_mode_uses_changed_files_and_skips_the_branchs_own_pr(self):
        sources = FakeSources(prs=[pr(7, "me/branch", ["a.py"]), pr(9, "other", ["a.py"])], changed=["a.py"])
        _, text = run(ownership.overlap_main, ["--branch"], sources)
        self.assertIn("PR #9", text)
        self.assertNotIn("PR #7", text)
        self.assertIn("me/branch", text)

    def test_no_overlap_is_said_plainly(self):
        _, text = run(ownership.overlap_main, ["a.py"], FakeSources(prs=[pr(7, "x", ["b.py"])]))
        self.assertIn("no overlap", text)
        self.assertNotIn("OVERLAP\n", text)

    def test_an_unavailable_source_is_reported_and_still_exits_zero(self):
        code, text = run(ownership.overlap_main, ["a.py"], FakeSources(fail={"gh", "bd"}))
        self.assertEqual(code, 0)
        self.assertIn("could not list open PRs: gh is unavailable", text)
        self.assertIn("could not list in-progress beads: bd is unavailable", text)

    def test_windows_separators_in_paths_are_normalized(self):
        _, text = run(ownership.overlap_main, ["host\\src\\x.rs"], FakeSources(prs=[pr(7, "x", ["host/src/x.rs"])]))
        self.assertIn("PR #7", text)


class WhoOwnsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.tmp.name).resolve()

    def tearDown(self):
        self.tmp.cleanup()

    def sources(self, **kw):
        defaults = dict(
            prs=[pr(12, "lane/feature", body="Owner: codex session 9\nfixes brwr-q1\n")],
            closed=[pr(5, "old/branch", state="MERGED", body="Owner: claude session old\n")],
            beads=[bead("brwr-q1", assignee="codex session 9", notes="worktree work/rb-feature")],
            worktrees=[{"path": str(self.path), "branch": "lane/feature", "head": "abc123"}],
            orca=[{"path": str(self.path), "branch": "refs/heads/lane/feature", "displayName": "Feature lane",
                   "workspaceStatus": "in-progress", "linkedPR": 12}],
        )
        defaults.update(kw)
        return FakeSources(**defaults)

    def test_a_pr_resolves_to_owner_worktree_orca_and_cited_beads(self):
        for target in ("12", "#12", "https://github.example/o/r/pull/12"):
            code, text = run(ownership.who_owns_main, [target], self.sources())
            self.assertEqual(code, 0, text)
            self.assertIn("PR #12", text)
            self.assertIn("owner: codex session 9", text)
            self.assertIn(f"worktree: {self.path}", text)
            self.assertIn("orca: Feature lane (in-progress)", text)
            self.assertIn("bead brwr-q1", text)

    def test_a_bead_resolves_to_assignee_notes_and_citing_prs(self):
        code, text = run(ownership.who_owns_main, ["brwr-q1"], self.sources())
        self.assertEqual(code, 0)
        self.assertIn("assignee: codex session 9", text)
        self.assertIn("worktree work/rb-feature", text)
        self.assertIn("PR #12", text)

    def test_a_worktree_path_resolves_through_its_branch(self):
        code, text = run(ownership.who_owns_main, [str(self.path)], self.sources())
        self.assertEqual(code, 0)
        self.assertIn("branch: lane/feature", text)
        self.assertIn("owner: codex session 9", text)

    def test_a_branch_resolves_to_its_prs_in_any_state(self):
        code, text = run(ownership.who_owns_main, ["old/branch"], self.sources())
        self.assertEqual(code, 0)
        self.assertIn("PR #5 (MERGED)", text)
        self.assertIn("owner: claude session old", text)

    def test_without_orca_it_degrades_and_says_so(self):
        _, text = run(ownership.who_owns_main, ["12"], self.sources(orca=None))
        self.assertIn("orca: not available", text)

    def test_nothing_found_exits_one(self):
        code, text = run(ownership.who_owns_main, ["no/such/branch"], self.sources())
        self.assertEqual(code, 1)
        self.assertIn("nothing owns", text)
        code, _ = run(ownership.who_owns_main, ["brwr-zz9"], self.sources())
        self.assertEqual(code, 1)

    def test_github_down_is_reported_not_fatal(self):
        code, text = run(ownership.who_owns_main, ["brwr-q1"], self.sources(fail={"gh"}))
        self.assertEqual(code, 0)
        self.assertIn("could not ask GitHub", text)


class OrcaParsingTests(unittest.TestCase):
    def test_reads_the_worktrees_list_from_orcas_envelope(self):
        payload = {"ok": True, "result": {"worktrees": [{"path": "/w", "branch": "refs/heads/b"}]}}
        self.assertEqual(ownership.parse_orca(payload), [{"path": "/w", "branch": "refs/heads/b"}])
        self.assertIsNone(ownership.parse_orca({"ok": False}))
        self.assertIsNone(ownership.parse_orca(["unexpected"]))

    def test_parses_git_worktree_porcelain(self):
        text = "worktree /a\nHEAD 111\nbranch refs/heads/main\n\nworktree /b\nHEAD 222\ndetached\n\n"
        self.assertEqual(ownership.parse_worktrees(text), [
            {"path": "/a", "head": "111", "branch": "main"}, {"path": "/b", "head": "222", "branch": None}])


if __name__ == "__main__":
    unittest.main()
