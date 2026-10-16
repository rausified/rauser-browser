#!/usr/bin/env python3
"""Assert that Brauser's two views of its work queue agree.

Beads (`.beads/issues.jsonl`) is machine-authoritative, but it only reaches an
overnight workspace at the next scaffold launch -- between launches the loop
reads and appends to `BOSUN_FEATURE_BACKLOG.md` instead. So the markdown is not
decoration: it is what actually steers a cycle. When the two disagree, work gets
proposed into no queue at all, or a queue item stays invisible to the loop that
is supposed to pick it up.

Both views live in the repo, so checking them costs a file read each and needs
no `bd` invocation -- which matters because this has to be runnable inside a
cycle, before `bd` has necessarily been restored from the JSONL.

Exit status is 0 when the views agree and 1 when they do not.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

DEFAULT_PREFIX = "brwr"
SHIPPED_SECTION_PATTERN = re.compile(r"\bshipped\b", re.IGNORECASE)
IN_PROGRESS_SECTION_PATTERN = re.compile(r"\bin progress\b", re.IGNORECASE)
PROPOSED_SECTION_PATTERN = re.compile(r"^proposed\b", re.IGNORECASE)
NOTES_SECTION_PATTERN = re.compile(r"^notes\b", re.IGNORECASE)
SECTION_HEADER_PATTERN = re.compile(r"^(#{2,6})\s+(.*)$")
OPEN_CHECKBOX_PATTERN = re.compile(r"^\s*[-*]\s+\[ \]\s")
DONE_CHECKBOX_PATTERN = re.compile(r"^\s*[-*]\s+\[[xX]\]\s")

# Statuses that mean "this issue still represents outstanding work". Anything
# else (closed, and any future terminal status) is not required to appear in the
# human-readable view.
LIVE_STATUSES = frozenset({"open", "in_progress", "blocked", "deferred"})


class Violation:
    """One disagreement between the queue and the markdown view."""

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


def load_issues(path: pathlib.Path) -> dict[str, dict[str, object]]:
    """Read the committed JSONL export into {id: issue}.

    Deliberately reads the export rather than calling `bd`: the export is the
    reviewable, committed form, and it is present even when the gitignored
    database has not been rebuilt yet.
    """
    issues: dict[str, dict[str, object]] = {}
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line:
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"{path}:{number}: not valid JSON: {error}") from error
        issue_id = record.get("id")
        if not isinstance(issue_id, str) or not issue_id:
            raise SystemExit(f"{path}:{number}: record has no usable 'id'")
        if issue_id in issues:
            raise SystemExit(f"{path}:{number}: duplicate issue id {issue_id}")
        issues[issue_id] = record
    return issues


class MarkdownItem:
    """An open checkbox line in the backlog, with the section that contains it.

    `owner` is the issue the item *is*; `referenced` are issues it merely names,
    such as a blocker cited in a "[BLOCKED BY ...]" prefix. Conflating the two
    makes every cross-reference look like a duplicate listing. The convention the
    backlog already follows is that an item leads with its own id, so the first
    id on the line is the owner.
    """

    def __init__(self, line: int, text: str, section: str, ids: list[str]) -> None:
        self.line = line
        self.text = text
        self.section = section
        self.owner = ids[0] if ids else None
        self.referenced = ids[1:]


def parse_backlog(
    path: pathlib.Path, id_pattern: re.Pattern[str]
) -> tuple[list[MarkdownItem], dict[str, list[tuple[int, str, str]]]]:
    """Return (open checkbox items, {issue id: [(line, section, top_section), ...]}).

    `section` is the nearest header; `top_section` is the enclosing `##` header,
    so a `###` subsection under Proposed still counts as Proposed.

    Mentions cover the whole file, not just checkboxes: the human-scoped fence
    names its issues in a prose paragraph rather than as items, and that is a
    legitimate way for an issue to be visible.
    """
    items: list[MarkdownItem] = []
    mentions: dict[str, list[tuple[int, str, str]]] = {}
    section = "(preamble)"
    top_section = section

    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        header = SECTION_HEADER_PATTERN.match(line)
        if header:
            section = header.group(2).strip()
            if len(header.group(1)) == 2:
                top_section = section

        for issue_id in dict.fromkeys(id_pattern.findall(line)):
            mentions.setdefault(issue_id, []).append((number, section, top_section))

        if OPEN_CHECKBOX_PATTERN.match(line):
            items.append(
                MarkdownItem(
                    line=number,
                    text=line.strip(),
                    section=section,
                    ids=list(dict.fromkeys(id_pattern.findall(line))),
                )
            )

    return items, mentions


def is_history_section(top_section: str) -> bool:
    return bool(
        SHIPPED_SECTION_PATTERN.search(top_section) or NOTES_SECTION_PATTERN.search(top_section)
    )


def summarize(text: str, width: int = 72) -> str:
    collapsed = " ".join(text.split())
    if len(collapsed) <= width:
        return collapsed
    return collapsed[: width - 1] + "…"


def check(
    issues: dict[str, dict[str, object]],
    items: list[MarkdownItem],
    mentions: dict[str, list[tuple[int, str, str]]],
) -> list[Violation]:
    violations: list[Violation] = []

    # 1. An open item with no issue id is work that exists in no queue. This is
    #    the failure that once let an overnight cycle propose three items into nothing.
    for item in items:
        if item.owner is None:
            violations.append(
                Violation(
                    "unfiled-markdown-item",
                    f"open item carries no issue id: {summarize(item.text)}",
                    line=item.line,
                )
            )

    # 2. A cited id that does not exist points at a queue that never had it, or
    #    at a typo that silently reads as "already tracked".
    for issue_id, places in sorted(mentions.items()):
        if issue_id not in issues:
            line = places[0][0]
            violations.append(
                Violation(
                    "unknown-issue-id",
                    f"{issue_id} is cited in the backlog but is not in the queue",
                    line=line,
                )
            )

    open_listings: dict[str, list[MarkdownItem]] = {}
    for item in items:
        if item.owner is not None:
            open_listings.setdefault(item.owner, []).append(item)

    for issue_id, listings in sorted(open_listings.items()):
        issue = issues.get(issue_id)
        if issue is None:
            continue  # already reported as unknown-issue-id
        status = str(issue.get("status") or "")

        # 3. Shipped work still presented as outstanding.
        if status not in LIVE_STATUSES:
            violations.append(
                Violation(
                    "closed-issue-listed-open",
                    f"{issue_id} is {status} in the queue but is still an open item here",
                    line=listings[0].line,
                )
            )

        # 4. The same issue open in two places invites two agents, or one agent
        #    twice, to build it.
        if len(listings) > 1:
            lines = ", ".join(str(listing.line) for listing in listings)
            violations.append(
                Violation(
                    "duplicate-open-listing",
                    f"{issue_id} appears as an open item {len(listings)} times (lines {lines})",
                    line=listings[0].line,
                )
            )

        # 5. Status disagreement between the queue and the section an item sits
        #    in. A claimed item left under "Proposed" reads as available work.
        in_progress_sections = [
            listing for listing in listings if IN_PROGRESS_SECTION_PATTERN.search(listing.section)
        ]
        if status == "in_progress" and not in_progress_sections:
            violations.append(
                Violation(
                    "status-section-mismatch",
                    f"{issue_id} is in_progress in the queue but no open item for it "
                    f"is under an 'In Progress' section (found under: "
                    f"{', '.join(sorted({listing.section for listing in listings}))})",
                    line=listings[0].line,
                )
            )
        if status == "open" and in_progress_sections:
            violations.append(
                Violation(
                    "status-section-mismatch",
                    f"{issue_id} is open in the queue but is listed under "
                    f"'{in_progress_sections[0].section}'",
                    line=in_progress_sections[0].line,
                )
            )

    # 6. Proposed is what the loop reads as "work worth picking up". A closed
    #    issue named there -- a checked-off item, a "resolved" note, a stale
    #    blocker reference -- keeps finished work in front of the loop, which
    #    then spends cycles re-reading or annotating it. Open checkbox lines are
    #    skipped: their owner is covered by closed-issue-listed-open, and a
    #    closed id they cite ("decided in pvt-x") is provenance, not stale work.
    open_item_lines = {item.line for item in items}
    for issue_id, places in sorted(mentions.items()):
        issue = issues.get(issue_id)
        if issue is None:
            continue
        status = str(issue.get("status") or "")
        if status in LIVE_STATUSES:
            continue
        for line, _section, top_section in places:
            if not PROPOSED_SECTION_PATTERN.search(top_section):
                continue
            if line in open_item_lines:
                continue
            violations.append(
                Violation(
                    "closed-issue-in-proposed",
                    f"{issue_id} is {status} in the queue but is still named under "
                    f"'{top_section}'; move the note to Shipped or Notes",
                    line=line,
                )
            )

    # 7. A live issue the markdown never mentions is invisible to a loop reading
    #    the markdown. Mentions under a history section (Shipped, or a dated
    #    Notes entry) do not count: they describe finished work, and a note
    #    saying an issue shipped would otherwise hide a still-open bead that
    #    `bd ready` keeps offering to the loop.
    for issue_id, issue in sorted(issues.items()):
        status = str(issue.get("status") or "")
        if status not in LIVE_STATUSES:
            continue
        places = mentions.get(issue_id, [])
        if any(not is_history_section(top) for _line, _section, top in places):
            continue
        history = sorted({top for _line, _section, top in places})
        title = summarize(str(issue.get("title") or ""), 56)
        if places:
            detail = (
                f"{issue_id} ({status}) is only mentioned under history sections "
                f"({', '.join(history)}), so the backlog does not present it as "
                f"outstanding; close it if it shipped, otherwise list it under "
                f"Proposed: {title}"
            )
        else:
            detail = f"{issue_id} ({status}) is not mentioned in the backlog at all: {title}"
        violations.append(Violation("issue-missing-from-backlog", detail))

    return violations


def main(argv: list[str] | None = None) -> int:
    repo_root = pathlib.Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--issues",
        type=pathlib.Path,
        default=repo_root / ".beads" / "issues.jsonl",
        help="committed beads export (default: %(default)s)",
    )
    parser.add_argument(
        "--backlog",
        type=pathlib.Path,
        default=repo_root / "BOSUN_FEATURE_BACKLOG.md",
        help="human-readable backlog (default: %(default)s)",
    )
    parser.add_argument(
        "--prefix",
        default=DEFAULT_PREFIX,
        help="beads issue id prefix (default: %(default)s)",
    )
    parser.add_argument("--json", action="store_true", help="emit a JSON report")
    args = parser.parse_args(argv)

    for path in (args.issues, args.backlog):
        if not path.is_file():
            # Absent files are a real finding, not a reason to pass quietly: a
            # missing export is exactly how a night's queue reasoning is lost.
            message = f"required file not found: {path}"
            if args.json:
                print(json.dumps({"status": "error", "message": message}, indent=2))
            else:
                print(f"queue consistency check ERROR: {message}", file=sys.stderr)
            return 1

    id_pattern = re.compile(rf"\b{re.escape(args.prefix)}-[0-9a-z]+\b")
    issues = load_issues(args.issues)
    items, mentions = parse_backlog(args.backlog, id_pattern)
    violations = check(issues, items, mentions)

    live = sorted(
        issue_id
        for issue_id, issue in issues.items()
        if str(issue.get("status") or "") in LIVE_STATUSES
    )

    if args.json:
        print(
            json.dumps(
                {
                    "status": "failed" if violations else "passed",
                    "issues_total": len(issues),
                    "issues_live": len(live),
                    "open_markdown_items": len(items),
                    "violations": [violation.as_dict() for violation in violations],
                },
                indent=2,
                sort_keys=True,
            )
        )
        return 1 if violations else 0

    if violations:
        print(
            f"queue consistency check FAILED: {len(violations)} disagreement(s) between "
            f"{args.backlog.name} and {args.issues.name}"
        )
        for violation in violations:
            print(f"  {violation}")
        print()
        print("Beads is authoritative: fix the markdown to match the queue, or file the")
        print("missing issue if the markdown is describing real work. If the JSONL is the")
        print("stale side (an issue closed or filed in a local database), refresh just that")
        print("record with scripts/bd_record_sync.py rather than a whole-file bd export.")
        return 1

    print(
        f"queue consistency check passed: {len(live)} live issue(s), "
        f"{len(items)} open markdown item(s), views agree"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
