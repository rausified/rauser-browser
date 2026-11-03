"""Tests for the bookkeeping-file merge drivers.

`.beads/issues.jsonl` (beads-jsonl), `BOSUN_FEATURE_BACKLOG.md`
(bosun-backlog) and the append-only logs (git's built-in union) are touched
by every worker cycle, queue PR and human PR. These tests reproduce the
conflict shapes seen on 2026-11-01 (one bead edited on both sides, new beads
on both sides, a generated backlog that diverged, log entries appended on
both sides) and check that a real `git merge` or `git rebase` with the
drivers installed by scripts/install-merge-drivers.sh finishes on its own.

Run with `npm run test:queue` (python3 -m unittest discover -s scripts/tests).
"""

from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest
import unittest.mock

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
ROOT = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import check_queue_consistency  # noqa: E402
import render_backlog  # noqa: E402
from render_backlog import dump_record, merge_jsonl  # noqa: E402

# The key order `bd export` writes (Go struct order), as seen in the queue.
BD_KEYS = (
    "_type", "id", "title", "description", "acceptance_criteria", "notes", "status",
    "priority", "issue_type", "assignee", "owner", "created_at", "created_by",
    "updated_at", "started_at", "closed_at", "close_reason", "metadata", "labels",
    "dependencies", "dependency_count", "dependent_count", "comment_count",
)


BD_EXPORT_FIXTURE = (
    "{\"_type\":\"issue\",\"id\":\"brwr-pnti\",\"title\":\"Decouple supervisor timing tests from durable disk flush latency\",\"description\":\"The baseline health gate fails existing supervisor wait_for assertions when a single fsync takes 5.6 seconds. Focused reruns reproduce a_long_task_report_is_shortened_not_failed and a_fifth_task_start_is_a_conflict with the run still Running after the ten-second wait. Retention fixtures also spend minutes syncing hundreds of artificial records. Keep production and record integration tests durable; make only explicit temporary unit-test stores omit disk flushes while retaining real atomic filesystem writes, permissions and errors.\",\"acceptance_criteria\":\"Existing supervisor, retention and full workspace tests pass without widening their timeouts or weakening state/error assertions; normal RunStore construction always syncs in production and the storage integration tests; ephemeral unit-test stores preserve atomic rename failures and private modes.\",\"notes\":\"Repair-only cycle after a red baseline. Open decisions resolved by Cookie as cycle coordinator: production durability remains mandatory; ordinary record storage tests use durable RunStore::open; only explicit temporary supervisor and retention fixtures omit sync; no time budgets or state assertions are relaxed. Completed the four-step plan: shared writer with cfg(test)-only ephemeral construction; opt-in temporary fixtures plus a real-filesystem regression for private modes, atomic rename refusal, temporary cleanup and preserved content; full verification; complete-diff self-review and documentation. Initial gate failed six supervisor tests. Focused task-capacity and long-report reruns timed out in Running; a local file fsync measured 5.6 seconds. First repaired full run exposed an independent one-second-watchdog versus shell-prompt-copy race, now split into persisted-limit and recorded-vocabulary tests using the existing RecordingRunner. The vocabulary check preserves the literal legal-review assertion and also compares the full prompt. Final gate passed: 1209 Rust tests including 993 host unit tests, 87 focused supervisor tests, 38 extension test files, normal and scripted-dialogs clippy, generated types, build, fmt, npm offline install, typecheck and lint. README supplemental checks passed: 191 Markdown corpus inputs/10696 renderings, seven script test files and 85 Python tests. Local IPC guards self-skipped under the sandbox; no browser certification claimed. Fresh complete-diff self-review found no remaining issues. Independent review/publication belong to the driver; no remote commands were used. Assigned feature work remains untouched, and brwr-e7b0 is blocked on offline browser/axe tooling. Logs: target/repair-final-gate/.\",\"status\":\"closed\",\"priority\":0,\"issue_type\":\"task\",\"assignee\":\"Cookie\",\"owner\":\"cookie-overnight@cookie.local\",\"created_at\":\"2026-11-01T17:27:33Z\",\"created_by\":\"Cookie\",\"updated_at\":\"2026-11-01T18:04:39Z\",\"started_at\":\"2026-11-01T17:29:11Z\",\"closed_at\":\"2026-11-01T18:04:39Z\",\"close_reason\":\"Repaired temporary supervisor fixtures and resumed-run prompt race; full required gate green in this repair cycle on base d5a156b9 (driver records final commit).\",\"metadata\":{\"backlog_section\":\"in-progress\",\"backlog_shipped\":\"**Category:** Testing \\u0026 Verification. **Separate supervisor waits from disk flush latency (2026-11-01).** Supervisor and retention fixtures explicitly choose unit-test-only temporary stores while keeping private writes, atomic replacements and filesystem errors. Production and normal storage tests still sync. Resumed-run limit and vocabulary checks are independent, retaining the one-second timeout and checking the full recorded-vocabulary prompt without racing a shell. **Metrics:** 1,209 Rust tests (+2), 87 supervisor cases, 38 extension test files and 85 Python tests; full required gate green, with sandbox IPC guards self-skipping.\"},\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-elx\",\"title\":\"Guard all service client connection tests in restricted runners\",\"description\":\"Opening health gate on 349b6ca0 fails five unit tests omitted from the sandbox IPC guard: client missing-service, launch failure and launch timeout, plus native missing-service and keyed-frame boundary. The additional scripted-dialogs gate exposes a sixth, without_spawn_seam_host_reports_service_unavailable. Each connects to a Unix socket even when no listener is expected; EPERM prevents the intended assertions.\",\"acceptance_criteria\":\"All six tests use the existing IPC guard without changing their assertions or production behavior; full required and scripted-dialogs gates pass with permission-denied IPC; simulated denial plus BRAUSER_REQUIRE_IPC_TESTS=1 fails each guarded test; TESTING.md explains missing-listener connections too.\",\"notes\":\"Opening gate on 349b6ca0 reproduced five unguarded Unix-socket test failures; added only the existing test guard. Simulated denial skips all five, and BRAUSER_REQUIRE_IPC_TESTS=1 fails exactly those five. Also reproduced existing brwr-647 deterministically at local 10:59:50 (5000 live vs 15000 expected), now repairing that fixture's live-plus-queue accounting in this health-only cycle. No production changes.\\nScripted-dialogs integration gate reproduced the same omission in without_spawn_seam_host_reports_service_unavailable; guard that sixth test too.\\nFinal full required gate and scripted-dialogs tests pass. All six added guards verified to skip on simulated denial and fail with BRAUSER_REQUIRE_IPC_TESTS=1. Existing socket assertions unchanged; all edits test-only. See target/overnight-health/final-repair/ for logs.\",\"status\":\"closed\",\"priority\":0,\"issue_type\":\"bug\",\"assignee\":\"cookie\",\"owner\":\"cookie-overnight@cookie.local\",\"created_at\":\"2026-10-27T22:00:56Z\",\"created_by\":\"cookie\",\"updated_at\":\"2026-10-27T22:07:52Z\",\"started_at\":\"2026-10-27T22:01:11Z\",\"closed_at\":\"2026-10-27T22:07:52Z\",\"close_reason\":\"Added existing IPC guard to five unit tests and the missing-service integration test; skip/fail paths and full gate verified (repair cycle on 349b6ca0).\",\"dependencies\":[{\"issue_id\":\"brwr-elx\",\"depends_on_id\":\"brwr-1hj\",\"type\":\"discovered-from\",\"created_at\":\"2026-10-27T22:00:56Z\",\"created_by\":\"cookie\",\"metadata\":\"{}\"}],\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-e7b0\",\"title\":\"Stop accessibility regressions across Brauser's UI\",\"description\":\"Run axe-core against built `panel.html`, `options.html` and `tasks.html` in a small state matrix covering setup, configured, disconnected and error states. Add keyboard assertions for reaching and activating each primary action, changing panel tabs, operating the task listbox, escaping transient UI and retaining visible focus; record unresolved automated checks as manual-review items rather than passes. Acceptance: Every enumerated state has zero critical or serious axe violations, every primary action is reachable and operable without a mouse, composite widgets expose the expected roles and selection state, focus is neither trapped nor lost after dynamic updates, and a seeded violation fails the gate. Incomplete axe checks remain visible, and the result makes no claim of full WCAG conformance or screen-reader certification.. Sources: AMOS_docs/AMOS Design Principles.md, ANCON news_aiSDLC, 2026-08-06 \u2014 Characterizing Visual Accessibility Issues in AI Developer Tools (https://arxiv.org/abs/2608.05116), https://www.w3.org/WAI/WCAG22/quickref/, https://github.com/dequelabs/axe-core. Verdict: yes.\",\"acceptance_criteria\":\"Every enumerated state has zero critical or serious axe violations, every primary action is reachable and operable without a mouse, composite widgets expose the expected roles and selection state, focus is neither trapped nor lost after dynamic updates, and a seeded violation fails the gate. Incomplete axe checks remain visible, and the result makes no claim of full WCAG conformance or screen-reader certification.\",\"notes\":\"2026-11-01 Cookie offline Linux environment audit: no Chrome/Chromium executable, no axe-core/browser automation installed, and npm offline cache has no axe-core, playwright, playwright-core or puppeteer. This assignment requires real browser execution and axe-core across built panel/options/tasks states; fake DOM checks cannot satisfy its acceptance. Needs a provisioned offline browser plus pinned axe-core/test tooling (or a worker authorized to acquire them). No accessibility conformance or acceptance claimed. Continue the ready assigned Tasks page refresh work meanwhile.\\n[2026-11-02 coordinator] Unblocked: tooling is provisioned. #206 pins playwright-core 1.64.1, @axe-core/playwright 4.14.0, axe-core 4.14.0 and adds scripts/a11y-browser.mjs (see TESTING.md 'Accessibility checks in a real browser'); constitution has Chromium at /opt/browsers/chromium and a seeded offline npm cache. Chromium cannot start inside the cycle sandbox, so a11yBrowser() skips there; add an npm script 'test:a11y' running the suite. Cookie's unsandboxed commit gate runs 'BRAUSER_REQUIRE_A11Y_TESTS=1 npm run --if-present test:a11y', so the suite is really exercised there and must pass; CI's macOS job should run it too.\",\"status\":\"open\",\"priority\":1,\"issue_type\":\"task\",\"owner\":\"cookie-overnight@cookie.local\",\"created_at\":\"2026-11-01T17:09:32Z\",\"created_by\":\"cookie\",\"updated_at\":\"2026-11-02T03:04:54Z\",\"external_ref\":\"scout:SCOUT-2026-10-30-brwr-02\",\"labels\":[\"assigned\",\"scout-proposal\"],\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-smr\",\"title\":\"Make the smoke's Summarize \\u0026 log run-card step deterministic under load\",\"description\":\"Not covered by brwr-smk (#130). PR #135 hit it twice locally under load: 'timed out after 30.2s waiting for a new run card after Summarize \\u0026 log', and once 'Panel shows: null' (possibly environmental: a codex review was rebuilding the extension at the same time). It passed 3 of 3 runs standalone. It is in PR 3's summarize/run-card smoke steps. Find the real cause (smoke wait or product race), fix with waitUntil-based waits or a test-first product fix, and prove it with 10+ runs under load.\",\"acceptance_criteria\":\"Cause named with evidence; 10+ consecutive headless smoke passes including under full CPU load, with before/after rates; full gate green.\",\"status\":\"closed\",\"priority\":1,\"issue_type\":\"bug\",\"assignee\":\"columbus\",\"owner\":\"50384921+jrausercisco@users.noreply.github.com\",\"created_at\":\"2026-10-28T00:40:00Z\",\"created_by\":\"jrausercisco\",\"updated_at\":\"2026-10-28T01:52:41Z\",\"closed_at\":\"2026-10-28T01:52:41Z\",\"close_reason\":\"Cause: extension/dist rebuilt under a running smoke (a codex review building in the same worktree), not CPU load or a product race. Reproduced exactly with one build:extension at the click (panel line 'can't read this page's content', no card) and at panel reopen (missing panel page). The smoke refuses to start during a build, detects a dist rebuild (inode/size/mtime), and the run-card wait fails at once with the panel's failure line. Before: 11/11 under load (6 at 1x, 5 at 3x) passed with no rebuild; after: 10/10 consecutive (5 idle, 5 under full load). After merging PR 4: cardIn (also t53's summarizeWithCodex) is on waitUntil with a get_run diagnosis; 11/11 more on the merged tree (5 idle, 6 under full load).\",\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-t61\",\"title\":\"Refuse incomplete note identity scans before autosave\",\"description\":\"Reviewing brwr-ezb found that historical note recovery treats Vault::note_candidates' truncated or skipped files as a complete identity source. If a conflicting artifact note sorts beyond the 4096-file or 16 MiB scan cap, an apparently unique candidate can redirect a save. Optional older-note discovery may remain best effort; primary historical key selection must refuse an incomplete scan.\",\"acceptance_criteria\":\"Reproduce a partial scan choosing a note despite omitted candidates; return correlated unavailable identity errors for loads and saves when the primary scan hits a bound or read failure, preserve all existing notes, and keep ordinary optional older-note discovery usable.\",\"status\":\"closed\",\"priority\":1,\"issue_type\":\"bug\",\"assignee\":\"Cookie\",\"owner\":\"cookie-overnight@cookie.local\",\"created_at\":\"2026-10-27T02:11:29Z\",\"created_by\":\"Cookie\",\"updated_at\":\"2026-10-27T02:17:27Z\",\"started_at\":\"2026-10-27T02:11:45Z\",\"closed_at\":\"2026-10-27T02:17:27Z\",\"close_reason\":\"Preserved shared and older note access across revisioned policy removal, narrowing and cache rebuilds; refused incomplete identity scans before autosave. Fresh complete diff reviewed; 332 Rust tests, 8 extension test files and full required gate green. Commit subject: fix: preserve notes after capture policy removal (SHA assigned by the driver).\",\"dependencies\":[{\"issue_id\":\"brwr-t61\",\"depends_on_id\":\"brwr-ezb\",\"type\":\"discovered-from\",\"created_at\":\"2026-10-27T02:11:29Z\",\"created_by\":\"Cookie\",\"metadata\":\"{}\"}],\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-hpo\",\"title\":\"HUMAN GATE: real macOS Chrome, live work tools, or John's accounts\",\"description\":\"Holding issue for steps that need John on macOS with real Chrome, a live Jira/SharePoint/OneDrive/GitHub account, or store/signing access ([human] items in BOSUN_FEATURE_BACKLOG.md). Cookie must not fake their evidence. Keep dependent items gated until John records the human step; do not close this holder to make items ready.\",\"status\":\"deferred\",\"priority\":1,\"issue_type\":\"chore\",\"owner\":\"cookie-overnight@cookie.local\",\"created_at\":\"2026-10-16T13:32:36Z\",\"created_by\":\"cookie\",\"updated_at\":\"2026-10-16T13:32:46Z\",\"labels\":[\"human\"],\"dependency_count\":0,\"dependent_count\":5,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-qn4\",\"title\":\"Explain why a page on a captured site is 'Not an artifact'\",\"description\":\"When the current origin has capture rules but none matches the page, the panel's artifact card should name the artifact types the site covers instead of a bare '\u25cb Not an artifact'. Example: 'github.com captures repositories, pull requests and issues for cisco-sbg; this page isn't one of them'. Motivation: John enabled GitHub, browsed pages the rules didn't cover, and read the bare label as capture being broken. Source: extension/panel.ts renderArtifact. Pairs with brwr-0t1 (near-miss detection).\",\"status\":\"open\",\"priority\":2,\"issue_type\":\"feature\",\"owner\":\"50384921+jrausercisco@users.noreply.github.com\",\"created_at\":\"2026-11-02T01:38:24Z\",\"created_by\":\"jrausercisco\",\"updated_at\":\"2026-11-02T01:38:24Z\",\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-eo7\",\"title\":\"Re-learn review: refuse damaged owned log markers in the strict scan\",\"description\":\"Codex review of PR #203 (P2), host/src/index.rs:1524-1528: the strict relearn scan still uses parse_log_lines, which silently drops a torn or malformed visit marker in an owned daily log. pass.complete only proves the byte scan finished, so truncating a valid kind=why marker yields an empty, issue-free plan that relearn-record can approve.\",\"acceptance_criteria\":\"Strict re-learning review refuses (or reports as an issue) any damaged owned marker instead of omitting it; regression covers a truncated kind=why marker through relearn-plan and relearn-record.\",\"status\":\"open\",\"priority\":2,\"issue_type\":\"bug\",\"owner\":\"endurance-traverse-02\",\"created_at\":\"2026-11-01T23:40:00Z\",\"created_by\":\"endurance-traverse-02\",\"updated_at\":\"2026-11-02T02:46:20Z\",\"metadata\":{\"backlog_text\":\"`brwr-eo7` **Re-learn review: refuse damaged owned log markers in the strict scan.**\\n  Codex P2 from PR #203 (`host/src/index.rs:1524`): the strict scan silently drops torn\\n  log markers, so an incomplete plan can be approved. Refuse damaged owned markers;\\n  fix before `brwr-dbp` relies on approvals.\",\"backlog_order\":41,\"backlog_section\":\"plan\"},\"labels\":[\"assigned\"],\"dependencies\":[{\"issue_id\":\"brwr-eo7\",\"depends_on_id\":\"brwr-hq8\",\"type\":\"discovered-from\",\"created_at\":\"2026-11-01T23:40:00Z\",\"created_by\":\"endurance-traverse-02\",\"metadata\":\"{}\"}],\"dependency_count\":0,\"dependent_count\":0,\"comment_count\":0}\n"
    "{\"_type\":\"issue\",\"id\":\"brwr-hrr\",\"title\":\"Damaged adopted-contract state: settings repair and recording-failure warnings (additive protocol change after brwr-sr3)\",\"description\":\"Remaining from brwr-hrr (items 1 and 3). Shipped in branch fix/adopted-contract-repair: hand-retired contracts (adopted minus current minus [[adapter_id_history]]) now take part in validate_adapter_transition_with_history; an unusable adopted-contracts.json refuses adapter additions (a damaged entry with an origin refuses them there only); and note loads/saves that the state decides refuse with NoteKeyRule::StateUnavailable, whose message names the file's path and says moving it aside restores notes on unrecognized pages.\\n\\nStill to do, as an additive protocol v6 change after brwr-sr3 (Open access PR 3) merges, rebased on its protocol/schema.json changes; bump to v7 only if a shape cannot be additive (extension and host ship from one install, so deny_unknown_fields on additions is fine). Existing channels do not fit: config_issue blocks every note/capture request and puts settings in setup mode; adapter_issue makes the extension suspend capture and offers Remove all adapter rules; adapter_warnings has only numeric fields; settings has no URL to learn of the issue through a note refusal.\\n\\nProposal: (1) config_result gains adopted_state: { issue: string|null, revision: string, warnings: string[] }. issue is set when the state file is unusable or holds a damaged entry; it names the file and says notes on unrecognized pages are paused to protect identity. warnings carries item 3: a failed state write or the 1024-contract / 1 MiB cap (today only an eprintln in ConfigStore::sync_adopted_contracts), so the user knows hand-removal protection is degraded; saves are not blocked. revision is the state file's revision. (2) New request forget_adopted_contracts { expected_revision } (the state revision from (1)). Under the config lock the host rereads the file; if the revision still matches it renames it aside to adopted-contracts.json.\\u003ctimestamp\\u003e.bak (the backup_invalid pattern for config.toml), then records the current contracts as a fresh file; a mismatch is conflict. Reply config_result. (3) Settings shows adopted_state.issue with a 'Forget recorded adapters' button explaining that notes from adapters removed by hand may need Use as artifact note again; the panel already shows the named refusal and may also show the issue. Update the refusal text (config::ADOPTED_CONTRACTS_REPAIR) to point at the button.\",\"acceptance_criteria\":\"Test-first; settings names the state file problem and offers the Forget recorded adapters repair (backed up, revision-checked under the config lock); a failed or capped state write shows a warning; full gate green.\",\"status\":\"closed\",\"priority\":2,\"issue_type\":\"bug\",\"assignee\":\"columbus\",\"owner\":\"50384921+jrausercisco@users.noreply.github.com\",\"created_at\":\"2026-10-27T12:40:00Z\",\"created_by\":\"jrausercisco\",\"updated_at\":\"2026-10-27T20:00:00Z\",\"closed_at\":\"2026-10-27T20:00:00Z\",\"close_reason\":\"Part 1 in #122 (frozen-ID guard, named refusal); part 2 on feat/adopted-state-repair: adopted_state on config_result, forget_adopted_contracts, settings repair and recording warnings (additive v6).\",\"dependencies\":[{\"issue_id\":\"brwr-hrr\",\"depends_on_id\":\"brwr-sr3\",\"type\":\"blocks\",\"created_at\":\"2026-10-27T18:00:00Z\",\"created_by\":\"jrausercisco\",\"metadata\":\"{}\"}],\"dependency_count\":1,\"dependent_count\":0,\"comment_count\":0}\n"
)


def row(issue_id: str, **fields: object) -> dict[str, object]:
    """A bead shaped like a `bd export` row, keys in bd's order."""
    record: dict[str, object] = {
        "_type": "issue",
        "id": issue_id,
        "title": f"Fix {issue_id}",
        "description": f"Description of {issue_id} with <angle> & ampersand.",
        "acceptance_criteria": "Done when the regression passes.",
        "status": "open",
        "priority": 2,
        "issue_type": "task",
        "owner": "endurance-traverse-02",
        "created_at": "2026-11-01T10:00:00Z",
        "created_by": "endurance-traverse-02",
        "updated_at": "2026-11-01T10:00:00Z",
        "metadata": {"backlog_section": "plan"},
        "dependency_count": 0,
        "dependent_count": 0,
        "comment_count": 0,
    }
    record.update(fields)
    return {key: record[key] for key in BD_KEYS if key in record and record[key] is not None}


def export(*records: dict[str, object]) -> str:
    """The queue file bd export would write for RECORDS."""
    ordered = sorted(records, key=render_backlog.export_order)
    return "".join(dump_record(record) + "\n" for record in ordered)


def claim(record: dict[str, object], when: str) -> dict[str, object]:
    """A worker claiming a bead: status, assignee, started_at."""
    return row(str(record["id"]), **{**record, "status": "in_progress", "assignee": "Cookie",
                                     "started_at": when, "updated_at": when})


class ExportFormatTests(unittest.TestCase):
    """The driver writes what `bd export` writes, so a merge is byte stable."""

    def test_real_bd_export_rows_round_trip_through_dump_record(self) -> None:
        # Rows copied from `bd export` (bd 1.3.0) output; a bd import + export
        # of the committed queue was byte-identical when this was written.
        lines = BD_EXPORT_FIXTURE.splitlines()
        records = [json.loads(line) for line in lines]
        self.assertEqual([dump_record(record) for record in records], lines)
        self.assertEqual(sorted(records, key=render_backlog.export_order), records)

    def test_dump_record_escapes_like_go(self) -> None:
        self.assertEqual(
            dump_record({"t": "a<b>&c\u2028é"}), '{"t":"a\\u003cb\\u003e\\u0026c\\u2028é"}'
        )

    def test_export_order_is_priority_then_newest_created_then_id(self) -> None:
        records = [
            row("brwr-b", priority=1, created_at="2026-10-01T00:00:00Z"),
            row("brwr-a", priority=1, created_at="2026-10-01T00:00:00Z"),
            row("brwr-new", priority=1, created_at="2026-11-01T00:00:00Z"),
            row("brwr-p0", priority=0, created_at="2026-01-01T00:00:00Z"),
        ]
        self.assertEqual(
            [r["id"] for r in sorted(records, key=render_backlog.export_order)],
            ["brwr-p0", "brwr-new", "brwr-a", "brwr-b"],
        )


class QueueMergeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.gate = row("brwr-gate", priority=0, created_at="2026-09-01T00:00:00Z")
        self.target = row("brwr-e7b0", priority=1)

    def merge(self, base, ours, theirs):
        return merge_jsonl(export(*base), export(*ours), export(*theirs))

    def test_same_bead_edited_on_both_sides_in_different_fields_combines(self) -> None:
        # Today's commonest shape: Cookie claims a bead while the coordinator
        # re-orders it in the backlog (one metadata key) on main.
        base = [self.gate, self.target]
        ours = [self.gate, claim(self.target, "2026-11-01T12:00:00Z")]
        reordered = row("brwr-e7b0", **{**self.target, "updated_at": "2026-11-01T11:00:00Z",
                                         "metadata": {"backlog_section": "plan", "backlog_order": 5}})
        text, conflicts, notes = self.merge(base, ours, [self.gate, reordered])
        self.assertEqual(conflicts, [])
        merged = json.loads(text.splitlines()[1])
        self.assertEqual(merged["status"], "in_progress")
        self.assertEqual(merged["assignee"], "Cookie")
        self.assertEqual(merged["metadata"], {"backlog_section": "plan", "backlog_order": 5})
        self.assertEqual(merged["updated_at"], "2026-11-01T12:00:01Z")
        self.assertEqual(list(merged), [k for k in BD_KEYS if k in merged])
        self.assertEqual(text, export(self.gate, merged))
        self.assertEqual(len(notes), 1)
        self.assertIn("brwr-e7b0", notes[0])

    def test_same_field_on_both_sides_takes_the_newer_updated_at(self) -> None:
        base = [self.target]
        older = row("brwr-e7b0", **{**self.target, "status": "blocked",
                                     "notes": "blocked on a11y tooling",
                                     "updated_at": "2026-11-01T11:00:00Z"})
        newer = row("brwr-e7b0", **{**self.target, "status": "closed",
                                     "close_reason": "shipped", "updated_at": "2026-11-02T03:04:54Z"})
        for ours, theirs, side in ((older, newer, "theirs"), (newer, older, "ours")):
            text, conflicts, notes = self.merge(base, [ours], [theirs])
            self.assertEqual(conflicts, [])
            merged = json.loads(text)
            self.assertEqual(merged["status"], "closed")
            # Combined, so newer than both (bd import keeps a tied local row).
            self.assertEqual(merged["updated_at"], "2026-11-02T03:04:55Z")
            # Fields only one side changed still arrive.
            self.assertEqual(merged["notes"], "blocked on a11y tooling")
            self.assertEqual(merged["close_reason"], "shipped")
            self.assertIn(f"took {side}", notes[0])

    def test_updated_at_compares_as_time_not_text(self) -> None:
        ours = row("brwr-e7b0", **{**self.target, "title": "A", "updated_at": "2026-11-01T12:00:00+02:00"})
        theirs = row("brwr-e7b0", **{**self.target, "title": "B", "updated_at": "2026-11-01T11:00:00Z"})
        text, conflicts, _ = self.merge([self.target], [ours], [theirs])
        self.assertEqual(conflicts, [])
        self.assertEqual(json.loads(text)["title"], "B")  # 11:00Z is after 10:00Z

    def test_a_record_taken_whole_keeps_its_updated_at(self) -> None:
        # Both sides end up equal to one input: no new timestamp is invented.
        changed = row("brwr-e7b0", **{**self.target, "title": "T", "updated_at": "2026-11-01T12:00:00Z"})
        text, _, _ = self.merge([self.target], [changed], [changed])
        self.assertEqual(json.loads(text)["updated_at"], "2026-11-01T12:00:00Z")

    def test_calendar_invalid_updated_at_is_unreadable_not_a_crash(self) -> None:
        ours = row("brwr-e7b0", **{**self.target, "title": "A", "updated_at": "2026-02-30T12:00:00Z"})
        theirs = row("brwr-e7b0", **{**self.target, "title": "B", "updated_at": "2026-11-01T12:00:00Z"})
        _, conflicts, _ = self.merge([self.target], [ours], [theirs])
        self.assertEqual(conflicts, ["brwr-e7b0"])

    def test_same_field_with_equal_updated_at_is_the_only_conflict(self) -> None:
        stamp = {"updated_at": "2026-11-01T12:00:00Z"}
        ours = row("brwr-e7b0", **{**self.target, **stamp, "title": "Ours"})
        theirs = row("brwr-e7b0", **{**self.target, **stamp, "title": "Theirs"})
        text, conflicts, _ = self.merge([self.gate, self.target], [self.gate, ours], [self.gate, theirs])
        self.assertEqual(conflicts, ["brwr-e7b0"])
        self.assertEqual(
            text,
            export(self.gate) + "<<<<<<< ours\n" + dump_record(ours) + "\n=======\n"
            + dump_record(theirs) + "\n>>>>>>> theirs\n",
        )

    def test_equal_updated_at_on_different_fields_still_combines(self) -> None:
        stamp = {"updated_at": "2026-11-01T12:00:00Z"}
        ours = row("brwr-e7b0", **{**self.target, **stamp, "title": "Ours"})
        theirs = row("brwr-e7b0", **{**self.target, **stamp, "priority": 0})
        text, conflicts, _ = self.merge([self.target], [ours], [theirs])
        self.assertEqual(conflicts, [])
        self.assertEqual((json.loads(text)["title"], json.loads(text)["priority"]), ("Ours", 0))

    def test_new_rows_on_both_sides_are_kept_in_bd_export_order(self) -> None:
        ours_new = row("brwr-r62", priority=2, created_at="2026-11-01T20:00:00Z")
        theirs_new = row("brwr-qn4", priority=1, created_at="2026-11-01T19:00:00Z")
        text, conflicts, _ = self.merge(
            [self.gate, self.target], [self.gate, self.target, ours_new],
            [self.gate, theirs_new, self.target],
        )
        self.assertEqual(conflicts, [])
        self.assertEqual(text, export(self.gate, self.target, ours_new, theirs_new))

    def test_both_add_the_same_bead_differently_combines_by_updated_at(self) -> None:
        ours = row("brwr-new", title="First", updated_at="2026-11-01T10:00:00Z")
        theirs = row("brwr-new", title="Second", updated_at="2026-11-01T10:05:00Z")
        text, conflicts, _ = self.merge([], [ours], [theirs])
        self.assertEqual((conflicts, json.loads(text)["title"]), ([], "Second"))

    def test_deleted_on_one_side_and_changed_on_the_other_keeps_the_change(self) -> None:
        changed = claim(self.target, "2026-11-01T12:00:00Z")
        for ours, theirs in (([self.gate], [self.gate, changed]), ([self.gate, changed], [self.gate])):
            text, conflicts, notes = self.merge([self.gate, self.target], ours, theirs)
            self.assertEqual((text, conflicts), (export(self.gate, changed), []))
            self.assertIn("deleted on one side", notes[0])

    def test_deleted_on_one_side_and_unchanged_on_the_other_is_deleted(self) -> None:
        text, conflicts, _ = self.merge([self.gate, self.target], [self.gate],
                                        [self.gate, self.target, row("brwr-x")])
        self.assertEqual(text, export(self.gate, row("brwr-x")))

    def test_one_side_unchanged_returns_the_other_byte_for_byte(self) -> None:
        unsorted = dump_record(self.target) + "\n" + dump_record(self.gate) + "\n"
        base = export(self.gate, self.target)
        self.assertEqual(merge_jsonl(base, unsorted, base), (unsorted, [], []))
        self.assertEqual(merge_jsonl(base, base, unsorted), (unsorted, [], []))

    def test_whole_rows_keep_their_exact_bytes(self) -> None:
        # A row taken from one side is copied, not re-serialized.
        odd = '{"id": "brwr-odd", "status": "open", "priority": 2, "created_at": "2026-01-01T00:00:00Z"}'
        base = export(self.target)
        theirs = base + odd + "\n"
        ours = export(claim(self.target, "2026-11-01T12:00:00Z"))
        text, _, _ = merge_jsonl(base, ours, theirs)
        self.assertIn(odd + "\n", text)

    def test_no_bead_is_ever_dropped(self) -> None:
        # Every combination of absent / base / changed-A / changed-B for one
        # bead: the bead disappears only when one side deleted it and the
        # other left it exactly as in base.
        base_row = self.target
        a = row("brwr-e7b0", **{**self.target, "title": "A", "updated_at": "2026-11-01T11:00:00Z"})
        b = row("brwr-e7b0", **{**self.target, "status": "closed", "updated_at": "2026-11-01T12:00:00Z"})
        versions = {"absent": None, "base": base_row, "a": a, "b": b}
        for base_name in ("absent", "base"):
            for ours_name, ours in versions.items():
                for theirs_name, theirs in versions.items():
                    if base_name == "absent" and "base" in (ours_name, theirs_name):
                        continue
                    base_rows = [] if base_name == "absent" else [base_row]
                    text, conflicts, _ = self.merge(
                        base_rows, [r for r in (ours,) if r], [r for r in (theirs,) if r]
                    )
                    deleted = (
                        {ours_name, theirs_name} == {"absent", "base"}
                        or (ours_name, theirs_name) == ("absent", "absent")
                    )
                    with self.subTest(base=base_name, ours=ours_name, theirs=theirs_name):
                        self.assertEqual("brwr-e7b0" not in text, deleted)
                        self.assertEqual(conflicts, [])


SKELETON = """# Bosun Feature Backlog

Intro prose.

## Known Issues

<!-- BEGIN GENERATED section=known show-closed: edit the beads, then run python3 scripts/render_backlog.py -->
<!-- END GENERATED section=known -->

## Proposed

### Plan work

Plan intro.

<!-- BEGIN GENERATED section=plan: edit the beads, then run python3 scripts/render_backlog.py -->
<!-- END GENERATED section=plan -->

### New beads

<!-- BEGIN GENERATED section=new: edit the beads, then run python3 scripts/render_backlog.py -->
<!-- END GENERATED section=new -->

## In Progress

<!-- BEGIN GENERATED section=in-progress: edit the beads, then run python3 scripts/render_backlog.py -->
<!-- END GENERATED section=in-progress -->

## Shipped

<!-- BEGIN GENERATED section=shipped: edit the beads, then run python3 scripts/render_backlog.py -->
<!-- END GENERATED section=shipped -->

## Notes

Closing note.
"""


def rendered(queue: str, skeleton: str = SKELETON) -> str:
    return render_backlog.render(skeleton, render_backlog.parse_issues(queue, "q"))


class BacklogMergeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.target = row("brwr-e7b0", priority=1)
        self.base_q = export(self.target)
        self.ours_q = export(claim(self.target, "2026-11-01T12:00:00Z"), row("brwr-r62"))
        self.theirs_q = export(self.target, row("brwr-qn4", metadata={"backlog_section": "known"}))
        self.base = rendered(self.base_q)
        self.ours = rendered(self.ours_q, SKELETON.replace("Intro prose.", "Intro prose, edited by Cookie."))
        self.theirs = rendered(self.theirs_q, SKELETON.replace("Closing note.", "Closing note, edited on main."))

    def merge(self, queues):
        return render_backlog.merge_backlog(self.base, self.ours, self.theirs, queues, cwd=ROOT)

    def test_diverged_generated_blocks_are_rendered_from_the_merged_queue(self) -> None:
        text, status, why = self.merge((self.base_q, self.ours_q, self.theirs_q))
        self.assertEqual((status, why), (0, None))
        merged_q, _, _ = merge_jsonl(self.base_q, self.ours_q, self.theirs_q)
        expected_skeleton = SKELETON.replace("Intro prose.", "Intro prose, edited by Cookie.").replace(
            "Closing note.", "Closing note, edited on main.")
        self.assertEqual(text, rendered(merged_q, expected_skeleton))
        issues = render_backlog.parse_issues(merged_q, "q")
        self.assertEqual(check_queue_consistency.check_generated(issues, text), [])
        for issue_id in ("brwr-e7b0", "brwr-r62", "brwr-qn4"):
            self.assertEqual(text.count(f"`{issue_id}`"), 1)

    def test_unknown_commits_merge_the_prose_and_leave_the_file_for_resolve(self) -> None:
        text, status, why = self.merge(None)
        self.assertEqual(status, 1)
        self.assertIn("cannot tell", why)
        self.assertIn("edited by Cookie", text)
        self.assertIn("edited on main", text)
        self.assertNotIn("<<<<<<<", text)

    def test_a_queue_conflict_leaves_the_backlog_for_resolve(self) -> None:
        stamp = {"updated_at": "2026-11-01T12:00:00Z"}
        ours_q = export(row("brwr-e7b0", **{**self.target, **stamp, "title": "Ours"}))
        theirs_q = export(row("brwr-e7b0", **{**self.target, **stamp, "title": "Theirs"}))
        _, status, why = self.merge((self.base_q, ours_q, theirs_q))
        self.assertEqual(status, 1)
        self.assertIn("brwr-e7b0", why)

    def test_the_same_prose_edited_twice_is_a_conflict_with_fresh_blocks(self) -> None:
        self.ours = rendered(self.ours_q, SKELETON.replace("Plan intro.", "Ours."))
        self.theirs = rendered(self.theirs_q, SKELETON.replace("Plan intro.", "Theirs."))
        text, status, _ = self.merge((self.base_q, self.ours_q, self.theirs_q))
        self.assertEqual(status, 1)
        self.assertIn("<<<<<<< ours\nOurs.\n=======\nTheirs.\n>>>>>>> theirs", text)
        self.assertIn("`brwr-qn4`", text)


def git(cwd: pathlib.Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ, GIT_AUTHOR_NAME="t", GIT_AUTHOR_EMAIL="t@example.com",
               GIT_COMMITTER_NAME="t", GIT_COMMITTER_EMAIL="t@example.com")
    for key in [key for key in env if key.startswith("GITHEAD_")]:
        del env[key]
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, env=env)
    if check and result.returncode != 0:
        raise AssertionError(f"git {' '.join(args)} failed:\n{result.stdout}\n{result.stderr}")
    return result


PENDING = "# Pending AMOS Documentation Updates\n\n- **Brauser — base entry:** Base text.\n"
COOKIE = "# Cookie Feature Notes\n\n## 2026-10-16 — Base entry\n\nBase paragraph.\n"


QUEUE_ATTRIBUTES = ".beads/issues.jsonl merge=beads-jsonl\nBOSUN_FEATURE_BACKLOG.md merge=bosun-backlog\n"


class GitMergeDriverTests(unittest.TestCase):
    """A real repository with the committed .gitattributes and installed drivers."""

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = pathlib.Path(self.tmp.name)
        git(self.repo, "init", "-q", "-b", "main")
        git(self.repo, "config", "core.autocrlf", "false")
        (self.repo / "scripts").mkdir()
        (self.repo / ".beads").mkdir()
        for name in ("render_backlog.py", "check_queue_consistency.py", "install-merge-drivers.sh"):
            shutil.copy(SCRIPTS / name, self.repo / "scripts" / name)
        # Main's union rules plus the beads branch's two queue drivers; the
        # queue files live on that branch now (AGENTS.md "Work queue").
        (self.repo / ".gitattributes").write_text(
            (ROOT / ".gitattributes").read_text(encoding="utf-8") + QUEUE_ATTRIBUTES, encoding="utf-8"
        )
        # The scripts' default queue location is this repository's root.
        environ = unittest.mock.patch.dict(os.environ, {"BRAUSER_QUEUE_DIR": str(self.repo)})
        environ.start()
        self.addCleanup(environ.stop)
        self.target = row("brwr-e7b0", priority=1)
        self.commit("base", [self.target], pending=PENDING, cookie=COOKIE)
        self.install()

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def install(self) -> subprocess.CompletedProcess[str]:
        if os.name == "nt":  # no POSIX sh on the PATH; same Python entry point
            command = [sys.executable, "scripts/render_backlog.py", "--install-merge-driver"]
        else:
            command = ["sh", "scripts/install-merge-drivers.sh", sys.executable]
        result = subprocess.run(command, cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result

    def commit(self, message: str, records, *, prose: dict[str, str] | None = None,
               pending: str | None = None, cookie: str | None = None) -> None:
        queue = export(*records)
        (self.repo / ".beads" / "issues.jsonl").write_text(queue, encoding="utf-8", newline="\n")
        skeleton = SKELETON
        for old, new in (prose or {}).items():
            skeleton = skeleton.replace(old, new)
        (self.repo / "BOSUN_FEATURE_BACKLOG.md").write_text(
            rendered(queue, skeleton), encoding="utf-8", newline="\n")
        for name, text in (("PENDING_AMOS_DOC_UPDATES.md", pending), ("CookieFeature.md", cookie)):
            if text is not None:
                (self.repo / name).write_text(text, encoding="utf-8", newline="\n")
        git(self.repo, "add", "-A")
        git(self.repo, "commit", "-qm", message)

    def todays_branches(self) -> None:
        """Cookie's cycle branch and main, diverged the way they did on 2026-11-01."""
        git(self.repo, "checkout", "-qb", "cookie")
        self.commit(
            "cycle 1", [claim(self.target, "2026-11-01T12:00:00Z"), row("brwr-r62")],
            prose={"Intro prose.": "Intro prose, edited by Cookie."},
            pending=PENDING + "- **Brauser — Cookie entry (2026-11-01):** Cookie text.\n",
            cookie=COOKIE + "\n## 2026-11-01 — Cookie entry\n\nCookie paragraph.\n",
        )
        git(self.repo, "checkout", "-q", "main")
        reordered = row("brwr-e7b0", **{**self.target, "updated_at": "2026-11-01T11:00:00Z",
                                         "metadata": {"backlog_section": "plan", "backlog_order": 5}})
        self.commit(
            "queue PR", [reordered, row("brwr-qn4", metadata={"backlog_section": "known"})],
            prose={"Closing note.": "Closing note, edited on main."},
            pending=PENDING + "- **Brauser — Main entry (2026-11-01):** Main text.\n",
            cookie=COOKIE + "\n## 2026-11-01 — Main entry\n\nMain paragraph.\n",
        )

    def assert_clean_result(self) -> None:
        self.assertEqual(git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout, "")
        self.assertEqual(git(self.repo, "status", "--porcelain").stdout, "")
        check = subprocess.run(
            [sys.executable, "scripts/check_queue_consistency.py"],
            cwd=self.repo, capture_output=True, text=True,
        )
        self.assertEqual(check.returncode, 0, check.stdout + check.stderr)
        queue = (self.repo / ".beads" / "issues.jsonl").read_text(encoding="utf-8")
        records = {json.loads(line)["id"]: json.loads(line) for line in queue.splitlines()}
        self.assertEqual(set(records), {"brwr-e7b0", "brwr-r62", "brwr-qn4"})
        self.assertEqual(records["brwr-e7b0"]["status"], "in_progress")
        self.assertEqual(records["brwr-e7b0"]["metadata"]["backlog_order"], 5)
        self.assertEqual(queue, export(*records.values()))
        backlog = (self.repo / "BOSUN_FEATURE_BACKLOG.md").read_text(encoding="utf-8")
        self.assertIn("edited by Cookie", backlog)
        self.assertIn("edited on main", backlog)
        for name, entries in (("PENDING_AMOS_DOC_UPDATES.md", ("Cookie text.", "Main text.", "Base text.")),
                              ("CookieFeature.md", ("Cookie paragraph.", "Main paragraph.", "Base paragraph."))):
            text = (self.repo / name).read_text(encoding="utf-8")
            self.assertNotIn("<<<<<<<", text)
            for entry in entries:
                self.assertEqual(text.count(entry), 1, f"{name}: {entry}")

    def test_install_is_idempotent_and_registers_both_drivers(self) -> None:
        self.install()
        for name in ("beads-jsonl", "bosun-backlog"):
            values = git(self.repo, "config", "--get-all", f"merge.{name}.driver").stdout.splitlines()
            self.assertEqual(len(values), 1, values)
            self.assertIn("scripts/render_backlog.py", values[0])

    def test_attributes_name_the_drivers_for_each_bookkeeping_file(self) -> None:
        expected = {
            ".beads/issues.jsonl": "beads-jsonl",
            "BOSUN_FEATURE_BACKLOG.md": "bosun-backlog",
            "PENDING_AMOS_DOC_UPDATES.md": "union",
            "CookieFeature.md": "union",
        }
        for path, driver in expected.items():
            out = git(self.repo, "check-attr", "merge", "--", path).stdout.strip()
            self.assertEqual(out, f"{path}: merge: {driver}")

    def test_cookies_git_merge_of_main_finishes_without_conflicts(self) -> None:
        self.todays_branches()
        git(self.repo, "checkout", "-q", "cookie")
        merge = git(self.repo, "merge", "--no-edit", "main", check=False)
        self.assertEqual(merge.returncode, 0, merge.stdout + merge.stderr)
        self.assertEqual(len(git(self.repo, "log", "-1", "--format=%P").stdout.split()), 2)
        self.assert_clean_result()

    def test_merging_cookie_into_main_finishes_without_conflicts(self) -> None:
        self.todays_branches()
        merge = git(self.repo, "merge", "--no-edit", "cookie", check=False)
        self.assertEqual(merge.returncode, 0, merge.stdout + merge.stderr)
        self.assert_clean_result()

    def test_git_pull_of_main_finishes_without_conflicts(self) -> None:
        self.todays_branches()
        git(self.repo, "checkout", "-q", "cookie")
        pull = git(self.repo, "pull", "--no-rebase", "--no-edit", ".", "main", check=False)
        self.assertEqual(pull.returncode, 0, pull.stdout + pull.stderr)
        self.assert_clean_result()

    def test_merge_works_without_githead_in_the_environment(self) -> None:
        # Newer git may stop exporting GITHEAD_<oid>; GIT_REFLOG_ACTION still
        # names the merged branch. Hide GITHEAD_ from the driver to prove it.
        self.todays_branches()
        git(self.repo, "checkout", "-q", "cookie")
        for name in ("beads-jsonl", "bosun-backlog"):
            driver = git(self.repo, "config", f"merge.{name}.driver").stdout.strip()
            git(self.repo, "config", f"merge.{name}.driver",
                "for v in $(env | sed -n 's/^\\(GITHEAD_[0-9a-f]*\\)=.*/\\1/p'); do unset $v; done; " + driver)
        merge = git(self.repo, "merge", "--no-edit", "main", check=False)
        self.assertEqual(merge.returncode, 0, merge.stdout + merge.stderr)
        self.assert_clean_result()

    def test_rebasing_cookie_onto_main_finishes_without_conflicts(self) -> None:
        self.todays_branches()
        git(self.repo, "checkout", "-q", "cookie")
        rebase = git(self.repo, "rebase", "main", check=False)
        self.assertEqual(rebase.returncode, 0, rebase.stdout + rebase.stderr)
        self.assert_clean_result()

    def test_cherry_pick_falls_back_to_resolve(self) -> None:
        # A cherry-pick tells the driver nothing about the picked commit, so
        # the backlog stays conflicted (with no markers) and --resolve renders it.
        self.todays_branches()
        git(self.repo, "checkout", "-q", "cookie")
        pick = git(self.repo, "cherry-pick", "main", check=False)
        self.assertNotEqual(pick.returncode, 0)
        unmerged = git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout.split()
        self.assertEqual(unmerged, ["BOSUN_FEATURE_BACKLOG.md"])
        result = subprocess.run([sys.executable, "scripts/render_backlog.py", "--resolve"],
                                cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        git(self.repo, "-c", "core.editor=true", "cherry-pick", "--continue")
        self.assert_clean_result()

    def test_an_unorderable_bead_conflict_stops_and_resolve_finishes_it(self) -> None:
        stamp = {"updated_at": "2026-11-01T12:00:00Z"}
        git(self.repo, "checkout", "-qb", "side")
        self.commit("side", [row("brwr-e7b0", **{**self.target, **stamp, "title": "Side title"})])
        git(self.repo, "checkout", "-q", "main")
        self.commit("main", [row("brwr-e7b0", **{**self.target, **stamp, "title": "Main title"})])
        merge = git(self.repo, "merge", "--no-edit", "side", check=False)
        self.assertNotEqual(merge.returncode, 0)
        self.assertIn("brwr-e7b0", merge.stderr + merge.stdout)
        unmerged = set(git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout.split())
        self.assertEqual(unmerged, {".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md"})
        queue = self.repo / ".beads" / "issues.jsonl"
        self.assertIn("<<<<<<< ours", queue.read_text(encoding="utf-8"))
        queue.write_text(export(row("brwr-e7b0", **{**self.target, **stamp, "title": "Agreed"})),
                         encoding="utf-8", newline="\n")
        result = subprocess.run([sys.executable, "scripts/render_backlog.py", "--resolve"],
                                cwd=self.repo, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Agreed", (self.repo / "BOSUN_FEATURE_BACKLOG.md").read_text(encoding="utf-8"))
        self.assertEqual(git(self.repo, "diff", "--name-only", "--diff-filter=U").stdout, "")

    def test_union_keeps_both_versions_of_an_entry_edited_on_both_sides(self) -> None:
        # The logs are append-only; an in-place edit on both sides is the one
        # shape union cannot judge. It keeps both lines rather than dropping one.
        git(self.repo, "checkout", "-qb", "side")
        self.commit("side", [self.target], pending=PENDING.replace("Base text.", "Side wording."))
        git(self.repo, "checkout", "-q", "main")
        self.commit("main", [self.target], pending=PENDING.replace("Base text.", "Main wording."))
        merge = git(self.repo, "merge", "--no-edit", "side", check=False)
        self.assertEqual(merge.returncode, 0, merge.stdout + merge.stderr)
        text = (self.repo / "PENDING_AMOS_DOC_UPDATES.md").read_text(encoding="utf-8")
        self.assertIn("Main wording.", text)
        self.assertIn("Side wording.", text)
        self.assertNotIn("<<<<<<<", text)


if __name__ == "__main__":
    unittest.main()
