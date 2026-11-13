#!/usr/bin/env python3
"""Check the one-file-per-entry logs: design history and the testing log.

docs/history.md and TESTING.md's "Learning log" were appended to by almost
every PR, and two PRs appending at the same place conflict. They are now
frozen, and each new entry is its own file, the way the improvement feed is
(scripts/check_improvement_feed.py), so concurrent PRs never touch the same
lines, even under GitHub's plain merge:

- design history: docs/history/YYYY-MM-DD-<slug>.md, after docs/history.md;
- testing log: docs/testing-log/YYYY-MM-DD-<slug>.md, after TESTING.md's
  "Learning log".

The frozen files keep their text (DESIGN.md links to docs/history.md's
section anchors). Nothing commits a combined view, because a committed
aggregate would conflict exactly as the old files did; print one instead.

Checked, for each log:
- every entry file is named `YYYY-MM-DD-<slug>.md`, starts with exactly
  `## <same date> — <title>`, has no other `## ` heading, and ends with one
  newline;
- the frozen file gains no entry dated after FROZEN_AFTER (a `##`/`###`
  dated heading in docs/history.md, a `- YYYY-MM-DD` line in the Learning
  log).

Usage:
  python3 scripts/check_split_docs.py [--root DIR]        check; exit 1 with one line per problem
  python3 scripts/check_split_docs.py --print history     print docs/history.md, then its entries
  python3 scripts/check_split_docs.py --print testing-log print the Learning log, then its entries
scripts/check_current_state.py calls check(), so CI runs it on every pull
request, docs-only ones included.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

import check_improvement_feed

FROZEN_AFTER = "2026-11-13"
LEARNING_LOG = "## Learning log"


def _learning_log(text: str) -> str:
    """TESTING.md's Learning log section, heading included."""
    lines = text.replace("\r\n", "\n").split("\n")
    try:
        start = lines.index(LEARNING_LOG)
    except ValueError:
        raise ValueError(f"TESTING.md has no {LEARNING_LOG!r} section") from None
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return "\n".join(lines[start:end]).rstrip("\n") + "\n"


LOGS = {
    "history": {
        "frozen": "docs/history.md",
        "dir": pathlib.PurePosixPath("docs/history"),
        "section": lambda text: text.replace("\r\n", "\n"),
        "dated": re.compile(r"^#{2,3} (\d{4}-\d{2}-\d{2})\b", re.MULTILINE),
    },
    "testing-log": {
        "frozen": "TESTING.md",
        "dir": pathlib.PurePosixPath("docs/testing-log"),
        "section": _learning_log,
        "dated": re.compile(r"^- (\d{4}-\d{2}-\d{2})\b", re.MULTILINE),
    },
}


def check(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    for name, log in LOGS.items():
        frozen = log["frozen"]
        try:
            section = log["section"]((root / frozen).read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            errors.append(f"{frozen}: {error}")
            continue
        for match in log["dated"].finditer(section):
            if match.group(1) > FROZEN_AFTER:
                errors.append(
                    f"{frozen}: holds a {name} entry dated {match.group(1)}; the {name} is frozen, "
                    f"so put each new entry in its own file under {log['dir']}/"
                )
        errors.extend(check_improvement_feed.check_entries(root, log["dir"], f"{name} entry"))
    return errors


def render(root: pathlib.Path, name: str) -> str:
    log = LOGS[name]
    parts = [log["section"]((root / log["frozen"]).read_text(encoding="utf-8")).rstrip("\n")]
    for path in sorted((root / log["dir"]).glob("*.md")):
        parts.append(path.read_text(encoding="utf-8").replace("\r\n", "\n").rstrip("\n"))
    return "\n\n".join(parts) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parent.parent)
    parser.add_argument("--print", dest="show", choices=sorted(LOGS))
    args = parser.parse_args(argv)
    if args.show:
        sys.stdout.write(render(args.root, args.show))
        return 0
    errors = check(args.root)
    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
