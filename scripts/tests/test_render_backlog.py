"""Tests for scripts/render_backlog.py and the generated-backlog queue check.

Run with `npm run test:queue` (python3 -m unittest discover -s scripts/tests).
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import textwrap
import unittest

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))

import check_queue_consistency  # noqa: E402
import render_backlog  # noqa: E402


def bead(issue_id: str, **fields: object) -> dict[str, object]:
    record: dict[str, object] = {
        "id": issue_id,
        "title": f"Title of {issue_id}",
        "description": f"Description of {issue_id}.",
        "status": "open",
        "priority": 2,
        "created_at": "2026-10-01T00:00:00Z",
    }
    metadata = fields.pop("metadata", None)
    record.update(fields)
    if metadata is not None:
        record["metadata"] = metadata
    return record


def jsonl(*records: dict[str, object]) -> str:
    return "".join(json.dumps(record) + "\n" for record in records)


def block(section: str, *, show_closed: bool = False, body: str = "") -> str:
    flag = " show-closed" if show_closed else ""
    return (
        f"<!-- BEGIN GENERATED section={section}{flag}: "
        f"edit the beads, then run python3 scripts/render_backlog.py -->\n"
        f"{body}"
        f"<!-- END GENERATED section={section} -->\n"
    )


SKELETON = (
    "# Backlog\n\nIntro prose that names `brwr-gate`.\n\n"
    "## Known Issues\n\n" + block("known", show_closed=True) + "\n"
    "## Proposed\n\n### Plan work\n\nPlan intro.\n\n" + block("plan") + "\n"
    "### New beads\n\n" + block("new") + "\n"
    "## In Progress\n\n" + block("in-progress") + "\nTrailing prose.\n\n"
    "## Shipped\n\n" + block("shipped") + "\n- Older hand-written shipped entry.\n\n"
    "## Notes\n\nA note.\n"
)


def generated(text: str, section: str) -> str:
    begin = text.index(f"section={section}")
    begin = text.index("\n", begin) + 1
    end = text.index(f"<!-- END GENERATED section={section} -->")
    return text[begin:end]


class RenderTests(unittest.TestCase):
    def render(self, *records: dict[str, object], skeleton: str = SKELETON) -> str:
        issues = render_backlog.parse_issues(jsonl(*records), "issues.jsonl")
        return render_backlog.render(skeleton, issues)

    def test_backlog_text_is_rendered_verbatim_with_checkbox(self) -> None:
        text = "`brwr-a1` **Do the thing.**\n  Continuation line with `code`."
        out = self.render(
            bead("brwr-a1", metadata={"backlog_section": "plan", "backlog_text": text})
        )
        self.assertEqual(
            generated(out, "plan"),
            "- [ ] `brwr-a1` **Do the thing.**\n  Continuation line with `code`.\n",
        )

    def test_backlog_text_without_leading_id_gets_it(self) -> None:
        out = self.render(
            bead("brwr-a1", metadata={"backlog_section": "plan", "backlog_text": "**Thing.**"})
        )
        self.assertEqual(generated(out, "plan"), "- [ ] `brwr-a1` **Thing.**\n")

    def test_fallback_uses_title_description_lead_priority_and_labels(self) -> None:
        out = self.render(
            bead(
                "brwr-a1",
                title="Make it fast",
                description="## Why\n\nIt is slow   on big pages.\nReally.\n\nSecond paragraph.",
                priority=3,
                labels=["task-bar", "human"],
                metadata={"backlog_section": "plan"},
            )
        )
        self.assertEqual(
            generated(out, "plan"),
            "- [ ] `brwr-a1` **Make it fast.** It is slow on big pages. Really. "
            "_(P3; labels: human, task-bar)_\n",
        )

    def test_fallback_truncates_long_description_on_a_word(self) -> None:
        out = self.render(
            bead("brwr-a1", description="word " * 200, metadata={"backlog_section": "plan"})
        )
        line = generated(out, "plan")
        self.assertIn("…", line)
        self.assertLess(len(line), 420)
        self.assertNotIn("wor…", line)

    def test_order_is_backlog_order_then_priority_then_created_then_id(self) -> None:
        out = self.render(
            bead("brwr-zz", priority=1, metadata={"backlog_section": "plan"}),
            bead("brwr-c3", metadata={"backlog_section": "plan", "backlog_order": 20}),
            bead("brwr-b2", metadata={"backlog_section": "plan", "backlog_order": 10}),
            bead("brwr-aa", priority=1, metadata={"backlog_section": "plan"}),
            bead("brwr-ab", priority=1, created_at="2026-09-01T00:00:00Z",
                 metadata={"backlog_section": "plan"}),
        )
        ids = [line.split("`")[1] for line in generated(out, "plan").splitlines()]
        self.assertEqual(ids, ["brwr-b2", "brwr-c3", "brwr-ab", "brwr-aa", "brwr-zz"])

    def test_input_order_does_not_change_output(self) -> None:
        records = [
            bead("brwr-a1", metadata={"backlog_section": "plan"}),
            bead("brwr-b2", metadata={"backlog_section": "plan"}),
            bead("brwr-c3", status="in_progress"),
        ]
        self.assertEqual(self.render(*records), self.render(*reversed(records)))

    def test_closed_bead_leaves_proposed_blocks_but_stays_checked_in_show_closed(self) -> None:
        out = self.render(
            bead("brwr-p1", status="closed", metadata={"backlog_section": "plan"}),
            bead("brwr-k1", status="closed",
                 metadata={"backlog_section": "known", "backlog_text": "`brwr-k1` **Fixed.**"}),
            bead("brwr-k2", metadata={"backlog_section": "known", "backlog_text": "**Open.**"}),
        )
        self.assertEqual(generated(out, "plan"), "")
        self.assertEqual(
            generated(out, "known"), "- [x] `brwr-k1` **Fixed.**\n- [ ] `brwr-k2` **Open.**\n"
        )

    def test_in_progress_status_always_renders_in_progress(self) -> None:
        out = self.render(
            bead("brwr-a1", status="in_progress", assignee="cookie",
                 metadata={"backlog_section": "plan"}),
        )
        self.assertEqual(generated(out, "plan"), "")
        self.assertIn("`brwr-a1`", generated(out, "in-progress"))
        self.assertIn("assignee: cookie", generated(out, "in-progress"))

    def test_blocked_bead_placed_in_progress_stays_there_but_open_one_does_not(self) -> None:
        out = self.render(
            bead("brwr-b1", status="blocked", metadata={"backlog_section": "in-progress"}),
            bead("brwr-o1", status="open", metadata={"backlog_section": "in-progress"}),
        )
        self.assertIn("`brwr-b1`", generated(out, "in-progress"))
        self.assertNotIn("`brwr-o1`", generated(out, "in-progress"))
        self.assertIn("`brwr-o1`", generated(out, "new"))

    def test_live_bead_without_section_or_unknown_section_goes_to_new(self) -> None:
        out = self.render(
            bead("brwr-a1"),
            bead("brwr-b2", metadata={"backlog_section": "nonexistent"}),
            bead("brwr-c3", status="closed"),
        )
        self.assertIn("`brwr-a1`", generated(out, "new"))
        self.assertIn("`brwr-b2`", generated(out, "new"))
        self.assertNotIn("brwr-c3", out)

    def test_every_live_bead_is_rendered_exactly_once(self) -> None:
        records = [
            bead("brwr-a1", status=status, metadata=meta)
            for status in ("open", "blocked", "deferred", "in_progress")
            for meta in (None, {"backlog_section": "plan"}, {"backlog_section": "known"},
                         {"backlog_section": "in-progress"}, {"backlog_section": "zzz"})
        ]
        for record in records:
            out = self.render(record)
            self.assertEqual(out.count("`brwr-a1`"), 1, record)

    def test_shipped_block_lists_closed_beads_with_entry_newest_first(self) -> None:
        out = self.render(
            bead("brwr-s1", status="closed", closed_at="2026-10-01T00:00:00Z",
                 metadata={"backlog_shipped": "**Category:** Bug Fix. **Old (2026-10-01).** x"}),
            bead("brwr-s2", status="closed", closed_at="2026-10-02T00:00:00Z",
                 metadata={"backlog_shipped": "**Category:** Bug Fix. **New (2026-10-02).** y"}),
            bead("brwr-s3", status="closed", closed_at="2026-10-03T00:00:00Z"),
        )
        self.assertEqual(
            generated(out, "shipped"),
            "- **Category:** Bug Fix. **New (2026-10-02).** y\n\n"
            "- **Category:** Bug Fix. **Old (2026-10-01).** x\n\n",
        )

    def test_hand_written_text_outside_blocks_is_untouched(self) -> None:
        stale = SKELETON.replace(
            block("plan"), block("plan", body="- [ ] stale hand edit\n")
        )
        out = self.render(bead("brwr-a1", metadata={"backlog_section": "plan"}), skeleton=stale)
        self.assertEqual(render_backlog.strip_generated(out), SKELETON)
        self.assertNotIn("stale hand edit", out)

    def test_rendering_is_idempotent(self) -> None:
        records = [bead("brwr-a1", metadata={"backlog_section": "plan"}), bead("brwr-b2")]
        once = self.render(*records)
        self.assertEqual(self.render(*records, skeleton=once), once)

    def test_marker_errors_are_reported(self) -> None:
        issues = render_backlog.parse_issues("", "x")
        for broken, message in (
            (SKELETON.replace("<!-- END GENERATED section=plan -->\n", ""), "nested"),
            (SKELETON.replace("<!-- END GENERATED section=shipped -->\n", ""), "not closed"),
            (SKELETON.replace("## Notes", "<!-- END GENERATED section=new -->\n## Notes"), "without a BEGIN"),
            (SKELETON + block("plan"), "more than once"),
            (SKELETON.replace("<!-- END GENERATED section=plan -->", "<!-- END GENERATED section=new -->", 1), "does not match"),
            (SKELETON.replace(block("new"), ""), "required"),
        ):
            with self.subTest(message=message):
                with self.assertRaisesRegex(render_backlog.BacklogError, message):
                    render_backlog.render(broken, issues)

    def test_parse_issues_rejects_duplicates_and_bad_metadata(self) -> None:
        with self.assertRaisesRegex(render_backlog.BacklogError, "duplicate"):
            render_backlog.parse_issues(jsonl(bead("brwr-a1"), bead("brwr-a1")), "x")
        with self.assertRaisesRegex(render_backlog.BacklogError, "backlog_order"):
            render_backlog.parse_issues(
                jsonl(bead("brwr-a1", metadata={"backlog_order": "ten"})), "x"
            )
        with self.assertRaisesRegex(render_backlog.BacklogError, "backlog_text"):
            render_backlog.parse_issues(
                jsonl(bead("brwr-a1", metadata={"backlog_text": ["x"]})), "x"
            )

    def test_crlf_input_renders_like_lf(self) -> None:
        issues = render_backlog.parse_issues(jsonl(bead("brwr-a1")), "x")
        self.assertEqual(
            render_backlog.render(SKELETON.replace("\n", "\r\n"), issues),
            render_backlog.render(SKELETON, issues),
        )


class MergeJsonlTests(unittest.TestCase):
    def merge(self, base, ours, theirs):
        return render_backlog.merge_jsonl(jsonl(*base), jsonl(*ours), jsonl(*theirs))

    def test_both_sides_add_different_beads_at_the_end(self) -> None:
        a, b, c = bead("brwr-a1"), bead("brwr-b2"), bead("brwr-c3")
        text, conflicts = self.merge([a], [a, b], [a, c])
        self.assertEqual(conflicts, [])
        self.assertEqual(text, jsonl(a, b, c))

    def test_one_side_changes_a_bead_the_other_reorders(self) -> None:
        a, b = bead("brwr-a1"), bead("brwr-b2")
        a2 = dict(a, status="closed")
        text, conflicts = self.merge([a, b], [b, a], [a2, b])
        self.assertEqual(conflicts, [])
        self.assertEqual(text, jsonl(b, a2))

    def test_same_change_on_both_sides_is_not_a_conflict(self) -> None:
        a = bead("brwr-a1")
        a2 = dict(a, status="closed")
        self.assertEqual(self.merge([a], [a2], [a2]), (jsonl(a2), []))

    def test_equal_records_with_different_key_order_are_the_same(self) -> None:
        a = bead("brwr-a1")
        reordered = dict(reversed(list(a.items())))
        a2 = dict(a, status="closed")
        self.assertEqual(self.merge([a], [reordered], [a2]), (jsonl(a2), []))

    def test_deletion_against_unchanged_deletes(self) -> None:
        a, b = bead("brwr-a1"), bead("brwr-b2")
        self.assertEqual(self.merge([a, b], [a], [a, b]), (jsonl(a), []))

    def test_conflicting_change_is_marked_and_reported(self) -> None:
        a = bead("brwr-a1")
        ours, theirs = dict(a, status="closed"), dict(a, status="blocked")
        text, conflicts = self.merge([a], [ours], [theirs])
        self.assertEqual(conflicts, ["brwr-a1"])
        self.assertIn("<<<<<<< ours\n" + json.dumps(ours) + "\n=======\n", text)
        self.assertIn(json.dumps(theirs) + "\n>>>>>>> theirs\n", text)

    def test_delete_against_modify_conflicts(self) -> None:
        a = bead("brwr-a1")
        text, conflicts = self.merge([a], [], [dict(a, status="closed")])
        self.assertEqual(conflicts, ["brwr-a1"])

    def test_both_add_same_id_differently_conflicts(self) -> None:
        text, conflicts = self.merge([], [bead("brwr-a1")], [bead("brwr-a1", title="other")])
        self.assertEqual(conflicts, ["brwr-a1"])


def git(cwd: pathlib.Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.com",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.com")
    return subprocess.run(["git", *args], cwd=cwd, check=check, capture_output=True,
                          text=True, env=env)


class GitIntegrationTests(unittest.TestCase):
    """Two branches each add a bead and edit prose: the classic backlog conflict."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "core.autocrlf", "false")
        (self.repo / "scripts").mkdir()
        (self.repo / ".beads").mkdir()
        for name in ("render_backlog.py", "check_queue_consistency.py"):
            (self.repo / "scripts" / name).write_text(
                (SCRIPTS / name).read_text(encoding="utf-8"), encoding="utf-8"
            )
        (self.repo / ".gitattributes").write_text(
            ".beads/issues.jsonl merge=beads-jsonl\n", encoding="utf-8"
        )
        self.write_queue(bead("brwr-a1", metadata={"backlog_section": "plan"}))
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "base")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def run_script(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, "scripts/render_backlog.py", *args],
            cwd=self.repo, capture_output=True, text=True,
        )

    def write_queue(self, *records: dict[str, object], prose: str = "Plan intro.") -> None:
        gate = bead("brwr-gate", status="deferred")
        (self.repo / ".beads" / "issues.jsonl").write_text(jsonl(gate, *records), encoding="utf-8")
        (self.repo / "BOSUN_FEATURE_BACKLOG.md").write_text(
            SKELETON.replace("Plan intro.", prose), encoding="utf-8"
        )
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)

    def branch_pair(self) -> None:
        base = bead("brwr-a1", metadata={"backlog_section": "plan"})
        git(self.repo, "checkout", "-qb", "side")
        self.write_queue(base, bead("brwr-s1", metadata={"backlog_section": "plan"}),
                         prose="Plan intro, edited on side.")
        git(self.repo, "commit", "-qam", "side")
        git(self.repo, "checkout", "-q", "main")
        self.write_queue(base, bead("brwr-m1", metadata={"backlog_section": "plan"}))
        git(self.repo, "commit", "-qam", "main")

    def assert_merged(self) -> None:
        text = (self.repo / "BOSUN_FEATURE_BACKLOG.md").read_text(encoding="utf-8")
        self.assertIn("Plan intro, edited on side.", text)
        for issue_id in ("brwr-a1", "brwr-s1", "brwr-m1"):
            self.assertEqual(text.count(f"`{issue_id}`"), 1)
        self.assertNotIn("<<<<<<<", text)
        check = subprocess.run(
            [sys.executable, "scripts/check_queue_consistency.py"],
            cwd=self.repo, capture_output=True, text=True,
        )
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)

    def test_resolve_after_a_conflicted_merge_without_the_driver(self) -> None:
        self.branch_pair()
        merge = git(self.repo, "merge", "side", check=False)
        self.assertNotEqual(merge.returncode, 0)
        result = self.run_script("--resolve")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout, "")
        self.assert_merged()

    def test_resolve_during_a_rebase(self) -> None:
        self.branch_pair()
        git(self.repo, "checkout", "-q", "side")
        rebase = git(self.repo, "rebase", "main", check=False)
        self.assertNotEqual(rebase.returncode, 0)
        result = self.run_script("--resolve")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_merged()

    def test_installed_jsonl_driver_merges_the_queue_then_resolve_rerenders(self) -> None:
        result = self.run_script("--install-merge-driver")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.branch_pair()
        git(self.repo, "merge", "side", check=False)
        unmerged = git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout.split()
        self.assertNotIn(".beads/issues.jsonl", unmerged)
        result = self.run_script("--resolve")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_merged()

    def test_installed_driver_falls_back_to_a_line_merge_without_the_script(self) -> None:
        # A branch from before the renderer existed has no script in its
        # working tree; the driver must not leave only ours' queue behind.
        result = self.run_script("--install-merge-driver")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.branch_pair()
        script = self.repo / "scripts" / "render_backlog.py"
        saved = script.read_text(encoding="utf-8")
        script.unlink()
        try:
            git(self.repo, "merge", "side", check=False)
            queue = (self.repo / ".beads" / "issues.jsonl").read_text(encoding="utf-8")
            self.assertIn('"brwr-s1"', queue)
            self.assertIn('"brwr-m1"', queue)
        finally:
            script.write_text(saved, encoding="utf-8")
        result = self.run_script("--resolve")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assert_merged()

    def test_resolve_keeps_a_real_prose_conflict(self) -> None:
        base = bead("brwr-a1", metadata={"backlog_section": "plan"})
        git(self.repo, "checkout", "-qb", "side")
        self.write_queue(base, prose="Side wording.")
        git(self.repo, "commit", "-qam", "side")
        git(self.repo, "checkout", "-q", "main")
        self.write_queue(base, prose="Main wording.")
        git(self.repo, "commit", "-qam", "main")
        git(self.repo, "merge", "side", check=False)
        result = self.run_script("--resolve")
        self.assertEqual(result.returncode, 1)
        self.assertIn("BOSUN_FEATURE_BACKLOG.md", result.stdout + result.stderr)
        text = (self.repo / "BOSUN_FEATURE_BACKLOG.md").read_text(encoding="utf-8")
        self.assertIn("<<<<<<<", text)
        self.assertIn("`brwr-a1`", text)


class QueueCheckTests(unittest.TestCase):
    def check(self, backlog: str, *records: dict[str, object]) -> list[str]:
        issues = render_backlog.parse_issues(jsonl(*records), "issues.jsonl")
        return [v.kind for v in check_queue_consistency.check_generated(issues, backlog)]

    def fresh(self, *records: dict[str, object]) -> str:
        return render_backlog.render(SKELETON, render_backlog.parse_issues(jsonl(*records), "x"))

    def test_fresh_render_passes(self) -> None:
        records = [bead("brwr-gate", status="deferred"),
                   bead("brwr-a1", metadata={"backlog_section": "plan"})]
        self.assertEqual(self.check(self.fresh(*records), *records), [])

    def test_stale_generated_block_fails(self) -> None:
        records = [bead("brwr-gate", status="deferred"), bead("brwr-a1")]
        stale = self.fresh(records[0])
        self.assertEqual(self.check(stale, *records), ["generated-section-stale"])

    def test_hand_written_open_item_fails(self) -> None:
        records = [bead("brwr-gate", status="deferred")]
        text = self.fresh(*records).replace("A note.", "- [ ] `brwr-gate` hand item")
        self.assertIn("hand-written-open-item", self.check(text, *records))

    def test_unknown_id_in_prose_fails(self) -> None:
        self.assertIn("unknown-issue-id", self.check(self.fresh(), bead("brwr-x")))

    def test_unknown_section_and_dependency_fail(self) -> None:
        records = [
            bead("brwr-gate", status="deferred"),
            bead("brwr-a1", metadata={"backlog_section": "plann"},
                 dependencies=[{"issue_id": "brwr-a1", "depends_on_id": "brwr-zz"}]),
        ]
        kinds = self.check(self.fresh(*records), *records)
        self.assertIn("unknown-backlog-section", kinds)
        self.assertIn("unknown-dependency", kinds)

    def test_closed_issue_named_in_proposed_prose_fails(self) -> None:
        records = [bead("brwr-gate", status="closed")]
        text = self.fresh(*records).replace("Plan intro.", "Plan intro after `brwr-gate`.")
        self.assertIn("closed-issue-in-proposed", self.check(text, *records))

    def test_cli_keeps_issues_and_backlog_arguments(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            records = [bead("brwr-gate", status="deferred")]
            (root / "q.jsonl").write_text(jsonl(*records), encoding="utf-8")
            (root / "b.md").write_text(self.fresh(*records), encoding="utf-8")
            args = ["--issues", str(root / "q.jsonl"), "--backlog", str(root / "b.md")]
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(check_queue_consistency.main(args), 0)
                (root / "b.md").write_text(SKELETON, encoding="utf-8")
                self.assertEqual(check_queue_consistency.main(args), 1)


if __name__ == "__main__":
    unittest.main()
