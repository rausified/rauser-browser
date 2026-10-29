#!/usr/bin/env python3
"""Assert that Brauser's backlog is a faithful rendering of its beads queue.

Beads (`.beads/issues.jsonl`) is authoritative. `BOSUN_FEATURE_BACKLOG.md`'s
item lists are generated from it by `scripts/render_backlog.py` between
`BEGIN/END GENERATED` markers, and the overnight loop reads them between
launches, so a stale rendering steers a cycle wrong. This check:

- re-renders the generated sections and fails when they differ from the file;
- validates the queue (JSON, unique ids, backlog metadata types, dependencies
  that name real beads, `backlog_section` values that name a real block);
- checks the hand-written text around the blocks: no open checkbox items
  (open work lives in beads), no cited id that is not in the queue, and no
  closed bead named in a Proposed section's prose.

It reads only the two files and needs no `bd`, so it runs inside a cycle
before `bd` has rebuilt its database, and on the scaffold's committed copies
(`--issues`, `--backlog`). Exit status is 0 when everything holds, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import render_backlog  # noqa: E402
from render_backlog import BacklogError, LIVE_STATUSES  # noqa: E402

DEFAULT_PREFIX = "brwr"
SECTION_HEADER_PATTERN = re.compile(r"^(#{2,6})\s+(.*)$")
PROPOSED_SECTION_PATTERN = re.compile(r"^proposed\b", re.IGNORECASE)
OPEN_CHECKBOX_PATTERN = re.compile(r"^\s*[-*]\s+\[ \]\s")


class Violation:
    """One way the backlog fails to reflect the queue."""

    def __init__(self, kind: str, detail: str, *, line: int | None = None) -> None:
        self.kind = kind
        self.detail = detail
        self.line = line

    def as_dict(self) -> dict[str, object]:
        payload: dict[str, object] = {"kind": self.kind, "detail": self.detail}
        if self.line is not None:
            payload["line"] = self.line
        return payload

    def __str__(self) -> str:
        where = f"{self.line}: " if self.line is not None else ""
        return f"{where}[{self.kind}] {self.detail}"


def summarize(text: str, width: int = 72) -> str:
    collapsed = " ".join(text.split())
    return collapsed if len(collapsed) <= width else collapsed[: width - 1] + "…"


def check_generated(
    issues: dict[str, dict[str, object]],
    backlog: str,
    id_pattern: re.Pattern[str] | None = None,
) -> list[Violation]:
    id_pattern = id_pattern or re.compile(rf"\b{DEFAULT_PREFIX}-[0-9a-z]+\b")
    try:
        sections = render_backlog.section_keys(backlog)
        stale = render_backlog.stale_sections(backlog, issues)
        skeleton = render_backlog.strip_generated(backlog)
    except BacklogError as error:
        return [Violation("backlog-markers", str(error))]

    violations: list[Violation] = []
    if stale:
        violations.append(
            Violation(
                "generated-section-stale",
                f"generated section(s) {', '.join(stale)} do not match the queue; "
                f"run python3 scripts/render_backlog.py and commit the result "
                f"(never hand-edit a GENERATED block)",
            )
        )

    for issue_id, issue in sorted(issues.items()):
        section = render_backlog.meta(issue, "backlog_section")
        status = str(issue.get("status") or "")
        if status in LIVE_STATUSES and isinstance(section, str) and section not in sections:
            violations.append(
                Violation(
                    "unknown-backlog-section",
                    f"{issue_id} has backlog_section={section!r}, but the backlog has no "
                    f"such GENERATED block (it is shown under 'new'); known: "
                    f"{', '.join(sorted(sections))}",
                )
            )
        for dependency in issue.get("dependencies") or []:
            target = dependency.get("depends_on_id") if isinstance(dependency, dict) else None
            if isinstance(target, str) and id_pattern.fullmatch(target) and target not in issues:
                violations.append(
                    Violation("unknown-dependency", f"{issue_id} depends on {target}, which is not in the queue")
                )

    # The whole file, generated text included: an item may cite a blocker.
    for number, line in enumerate(render_backlog.split_lines(backlog), start=1):
        for issue_id in dict.fromkeys(id_pattern.findall(line)):
            if issue_id not in issues:
                violations.append(
                    Violation("unknown-issue-id", f"{issue_id} is cited in the backlog but is not in the queue", line=number)
                )

    top_section = "(preamble)"
    for number, line in enumerate(render_backlog.split_lines(skeleton), start=1):
        header = SECTION_HEADER_PATTERN.match(line)
        if header and len(header.group(1)) == 2:
            top_section = header.group(2).strip()
        if OPEN_CHECKBOX_PATTERN.match(line):
            violations.append(
                Violation(
                    "hand-written-open-item",
                    f"open item outside a GENERATED block (skeleton line {number}): "
                    f"{summarize(line)}; file it as a bead with backlog metadata instead",
                )
            )
        if PROPOSED_SECTION_PATTERN.search(top_section):
            for issue_id in dict.fromkeys(id_pattern.findall(line)):
                issue = issues.get(issue_id)
                if issue is not None and str(issue.get("status") or "") not in LIVE_STATUSES:
                    violations.append(
                        Violation(
                            "closed-issue-in-proposed",
                            f"{issue_id} is {issue.get('status')} but is named in "
                            f"'{top_section}' prose (skeleton line {number}); move the note "
                            f"to Shipped or Notes",
                        )
                    )
    return violations


def main(argv: list[str] | None = None) -> int:
    repo_root = pathlib.Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--issues", type=pathlib.Path, default=repo_root / ".beads" / "issues.jsonl",
                        help="committed beads export (default: %(default)s)")
    parser.add_argument("--backlog", type=pathlib.Path, default=repo_root / "BOSUN_FEATURE_BACKLOG.md",
                        help="generated backlog (default: %(default)s)")
    parser.add_argument("--prefix", default=DEFAULT_PREFIX, help="beads issue id prefix (default: %(default)s)")
    parser.add_argument("--json", action="store_true", help="emit a JSON report")
    args = parser.parse_args(argv)

    def fail(message: str) -> int:
        if args.json:
            print(json.dumps({"status": "error", "message": message}, indent=2))
        else:
            print(f"queue consistency check ERROR: {message}", file=sys.stderr)
        return 1

    for path in (args.issues, args.backlog):
        if not path.is_file():
            # A missing export is exactly how a night's queue reasoning is lost.
            return fail(f"required file not found: {path}")
    try:
        issues = render_backlog.load_issues(args.issues)
    except BacklogError as error:
        return fail(str(error))
    id_pattern = re.compile(rf"\b{re.escape(args.prefix)}-[0-9a-z]+\b")
    violations = check_generated(issues, args.backlog.read_text(encoding="utf-8"), id_pattern)
    live = sum(1 for issue in issues.values() if str(issue.get("status") or "") in LIVE_STATUSES)

    if args.json:
        print(json.dumps(
            {
                "status": "failed" if violations else "passed",
                "issues_total": len(issues),
                "issues_live": live,
                "violations": [violation.as_dict() for violation in violations],
            },
            indent=2, sort_keys=True,
        ))
        return 1 if violations else 0

    if violations:
        print(f"queue consistency check FAILED: {len(violations)} problem(s) in "
              f"{args.backlog.name} against {args.issues.name}")
        for violation in violations:
            print(f"  {violation}")
        print()
        print("Beads is authoritative and the backlog's item lists are generated from it:")
        print("edit the bead (one JSONL row, or bd update in this workspace), then run")
        print("python3 scripts/render_backlog.py. After a merge or rebase conflict in either")
        print("file, run python3 scripts/render_backlog.py --resolve.")
        return 1

    print(f"queue consistency check passed: {live} live issue(s), generated sections up to date")
    return 0


if __name__ == "__main__":
    sys.exit(main())
