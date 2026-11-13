#!/usr/bin/env python3
"""Keep the short project-status summaries aligned with shipped M1.5a work.

Also runs scripts/check_improvement_feed.py and scripts/check_split_docs.py,
so CI checks the feed's and the split logs' layout; checks that AGENTS.md's
Current state table is what docs/agents/current-state/ generates
(scripts/generate-current-state.mjs --check) and that AGENTS.md stays one
sentence per line (scripts/sentence-lines.mjs --check); and fails when the work queue is committed here: `.beads/issues.jsonl` and
`BOSUN_FEATURE_BACKLOG.md` are not in git: the queue lives in bd's Dolt database
and the backlog is rendered from docs/backlog/ (AGENTS.md "Work queue",
worker-contract.json `never_commit`). It also checks the backlog frame's
GENERATED markers.
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

import check_improvement_feed
import check_split_docs
import render_backlog


ROOT = pathlib.Path(__file__).resolve().parent.parent
QUEUE_FILES = (".beads/issues.jsonl", "BOSUN_FEATURE_BACKLOG.md")


def committed_queue_files(root: pathlib.Path) -> list[str]:
    """Queue files tracked in ROOT's checkout (worker-contract.json never_commit)."""
    try:
        result = subprocess.run(
            ["git", "ls-files", "--", *QUEUE_FILES], cwd=root, capture_output=True, text=True, check=False,
        )
    except OSError:
        return []
    return result.stdout.split() if result.returncode == 0 else []


def node_checks(root: pathlib.Path) -> list[str]:
    """The Node checks of AGENTS.md: its generated table and its line layout."""
    errors: list[str] = []
    for script, args in (
        ("scripts/generate-current-state.mjs", ["--check"]),
        ("scripts/sentence-lines.mjs", ["--check", "AGENTS.md"]),
    ):
        try:
            result = subprocess.run(
                ["node", script, *args], cwd=root, capture_output=True, text=True, check=False,
            )
        except OSError as error:
            errors.append(f"{script}: cannot run node: {error}")
            continue
        if result.returncode != 0:
            errors.extend(line for line in (result.stderr or result.stdout).splitlines() if line.strip())
            if not (result.stderr or result.stdout).strip():
                errors.append(f"{script} {' '.join(args)} failed with exit code {result.returncode}")
    return errors


def normalized(text: str) -> str:
    return " ".join(text.casefold().split())


def section(path: pathlib.Path, heading: str) -> str:
    text = path.read_text(encoding="utf-8")
    match = re.search(
        rf"^## {re.escape(heading)}\s*$\n(?P<body>.*?)(?=^## |\Z)",
        text,
        flags=re.MULTILINE | re.DOTALL,
    )
    if match is None:
        raise ValueError(f"{path.name} has no {heading!r} section")
    return match.group("body")


def status_line(path: pathlib.Path) -> str:
    text = path.read_text(encoding="utf-8")
    match = re.search(r"^> Status:\s*(?P<body>.+?)\s*$", text, flags=re.MULTILINE)
    if match is None:
        raise ValueError(f"{path.name} has no status line")
    return match.group("body")


def main() -> int:
    try:
        agents = section(ROOT / "AGENTS.md", "Current state")
        readme = section(ROOT / "README.md", "Status")
        readme_development = section(ROOT / "README.md", "Development")
        design = status_line(ROOT / "DESIGN.md")
        design_text = (ROOT / "DESIGN.md").read_text(encoding="utf-8")
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        return 1

    errors: list[str] = []
    stale_claims = {
        "AGENTS.md": (
            "worker lease, TypeScript prefilter, host recognition of delivered events, "
            "and artifact activity log remain unbuilt"
        ),
        "README.md": "Hourly artifact activity and the Recent view are still unbuilt",
        "DESIGN.md": "hourly artifact activity are next",
        "AGENTS.md delivery": "delivery-and-durability step",
        "README.md delivery": "delivery compaction, the index",
        "DESIGN.md delivery": "delivery and durability, indexing",
        "AGENTS.md index": "index and artifact-file generation",
        "README.md index": "the index and artifact files",
        "DESIGN.md index": "indexing, record keys",
        "AGENTS.md panel": "artifact panel card, Recent list, worklog view, record keys",
        "README.md panel": "artifact card, Recent view, worklog, record keys",
        "DESIGN.md panel": "record keys, artifact-facing views, and live-tool certification are next",
    }
    # Retired by protocol v6 (open access): the AI privacy list, the harness
    # token, and the per-tab grant wall ("can't see this tab"). Retired by the
    # service runners (open access PR 3): the harness lockdown's reviewed argv
    # and cleared environment.
    for label, claim in {
        "privacy list": "AI privacy list",
        "harness token": "harness token",
        "grant wall": "can't see this tab",
        "reviewed argv": "reviewed argv",
        "cleared environment": "cleared environment",
    }.items():
        for doc in ("AGENTS.md", "README.md", "DESIGN.md"):
            stale_claims[f"{doc} {label}"] = claim
    # Retired by Capture Site for unknown sites (`brwr-t40`): the host
    # structural drafter, per-rule review and recurring-pages hint are built.
    stale_claims["README.md drafting"] = "Unknown-site automatic drafting is still in development"
    stale_claims["AGENTS.md drafting"] = "Unknown-site automatic drafting is still in development"
    stale_claims["AGENTS.md drafter"] = (
        "host structural drafter, selected-rule enable flow and recurring-page hint remain unbuilt"
    )
    # Retired by Capture Site Improve with AI (`brwr-f45`): the agent drafter
    # is built, and the recurring-pages hint drafts structurally first.
    stale_claims["AGENTS.md agent drafting"] = (
        "agent drafting (`brwr-f45`) and re-learning (`brwr-6x2`) are not built"
    )
    stale_claims["DESIGN.md agent hint"] = "the hint's draft uses the agent when one is set up"
    # Retired by the task bar (2026-10-28): the free-text agent row is built,
    # so "the rest of the agent integration" no longer describes what is left.
    stale_claims["AGENTS.md agent integration"] = "the rest of the agent integration"
    bodies = {
        "README.md drafting": readme + "\n" + readme_development,
        "DESIGN.md agent hint": design_text,
        "AGENTS.md": agents,
        "README.md": readme,
        "DESIGN.md": design,
        "AGENTS.md delivery": agents,
        "README.md delivery": readme,
        "DESIGN.md delivery": design,
        "AGENTS.md index": agents,
        "README.md index": readme,
        "DESIGN.md index": design,
        "AGENTS.md panel": agents,
        "README.md panel": readme,
        "DESIGN.md panel": design,
    }
    for key in stale_claims:
        if key not in bodies:
            bodies[key] = {"AGENTS.md": agents, "README.md": readme, "DESIGN.md": design}[
                key.split(" ", 1)[0]
            ]
    for filename, claim in stale_claims.items():
        body = bodies[filename]
        if normalized(claim) in normalized(body):
            errors.append(f"{filename} repeats the obsolete claim: {claim!r}")

    required_agents_facts = {
        "worker policy lease": "enabled adapter document now travels in the worker's revisioned policy lease",
        "worker prefilter": "prefilters unrecognized or out-of-scope navigations",
        "host recognition": "host independently rechecks each delivered event",
        "artifact activity": "offset-qualified hourly artifact activity",
        "unknown-site capture": "capture site for unknown sites (`brwr-t40`",
        "agent drafting": "improve with ai (`brwr-f45`",
        "task bar": "| task bar | built:",
    }
    folded_agents = normalized(agents)
    for fact, wording in required_agents_facts.items():
        if normalized(wording) not in folded_agents:
            errors.append(f"AGENTS.md Current state no longer records shipped {fact}")

    if "improve with ai" not in normalized(readme):
        errors.append("README.md Status no longer records shipped Improve with AI")
    if "task bar" not in normalized(readme):
        errors.append("README.md Status no longer records the shipped task bar")
    if "records hourly artifact activity" not in normalized(readme):
        errors.append("README.md Status no longer records shipped artifact activity")
    if "delivers day-bounded batches" not in normalized(readme):
        errors.append("README.md Status no longer records shipped batched delivery")
    if "hourly artifact activity" not in normalized(design):
        errors.append("DESIGN.md status no longer records shipped artifact activity")
    if "batched durable delivery" not in normalized(design):
        errors.append("DESIGN.md status no longer records shipped delivery durability")
    if "rebuildable sqlite `index.db`" not in normalized(agents):
        errors.append("AGENTS.md Current state no longer records shipped artifact indexing")
    if "disposable sqlite artifact index" not in normalized(readme):
        errors.append("README.md Status no longer records shipped artifact indexing")
    if "rebuildable indexing" not in normalized(design):
        errors.append("DESIGN.md status no longer records shipped artifact indexing")
    if "cross-tool recent list" not in normalized(agents):
        errors.append("AGENTS.md Current state no longer records shipped Recent view")
    if "filtered cross-tool recent view" not in normalized(readme):
        errors.append("README.md Status no longer records shipped artifact-facing views")
    if "record keys, and artifact-facing views merged" not in normalized(design):
        errors.append("DESIGN.md status no longer records shipped record keys and views")

    errors.extend(check_improvement_feed.check(ROOT))
    errors.extend(check_split_docs.check(ROOT))
    errors.extend(node_checks(ROOT))
    frame = ROOT / render_backlog.FRAME_PATH
    try:
        text = frame.read_text(encoding="utf-8")
        if render_backlog.strip_generated(text) != text.replace("\r\n", "\n"):
            errors.append(f"{render_backlog.FRAME_PATH} has text inside a GENERATED block; the beads fill those")
    except (OSError, render_backlog.BacklogError) as error:
        errors.append(f"{render_backlog.FRAME_PATH}: {error}")
    for file in committed_queue_files(ROOT):
        errors.append(
            f"{file} is committed, but the work queue lives in bd's Dolt database: "
            f"`git rm --cached {file}`, and publish bead edits with bd and scripts/dev/queue sync "
            f"(AGENTS.md \"Work queue\")"
        )

    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
