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
    }
    bodies = {"AGENTS.md": agents, "README.md": readme, "DESIGN.md": design}
    for filename, claim in stale_claims.items():
        body = bodies[filename]
        if normalized(claim) in normalized(body):
            errors.append(f"{filename} repeats the obsolete claim: {claim!r}")

    required_agents_facts = {
        "worker policy lease": "confirmed adapter document now travels in the worker's revisioned policy lease",
        "worker prefilter": "prefilters unrecognized or out-of-scope navigations",
        "host recognition": "host independently rechecks each delivered event",
        "artifact activity": "offset-qualified hourly artifact activity",
    }
    folded_agents = normalized(agents)
    for fact, wording in required_agents_facts.items():
        if normalized(wording) not in folded_agents:
            errors.append(f"AGENTS.md Current state no longer records shipped {fact}")

    if "records hourly artifact activity" not in normalized(readme):
        errors.append("README.md Status no longer records shipped artifact activity")
    if "hourly artifact activity merged" not in normalized(design):
        errors.append("DESIGN.md status no longer records shipped artifact activity")

    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
