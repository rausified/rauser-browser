#!/usr/bin/env python3
"""Keep the short project-status summaries aligned with shipped M1.5a work."""

from __future__ import annotations

import pathlib
import re
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent


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
    bodies = {
        "README.md drafting": readme + "\n" + readme_development,
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
    }
    folded_agents = normalized(agents)
    for fact, wording in required_agents_facts.items():
        if normalized(wording) not in folded_agents:
            errors.append(f"AGENTS.md Current state no longer records shipped {fact}")

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

    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
