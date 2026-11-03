#!/usr/bin/env python3
"""Check the Brauser Improvement Feed's layout (BRAUSER_IMPROVEMENT_FEED.md).

The feed is one file per entry under docs/improvement-feed/, so concurrent pull
requests that each add an entry never touch the same lines and merge cleanly
even under GitHub's plain merge, which ignores .gitattributes merge drivers.
BRAUSER_IMPROVEMENT_FEED.md is the fixed entry point and holds no entries.

Checked:
- BRAUSER_IMPROVEMENT_FEED.md exists, keeps its `# Brauser Improvement Feed`
  title, and has no dated `## YYYY-MM-DD` entry headings;
- every file in docs/improvement-feed/ is named `YYYY-MM-DD-<slug>.md`
  (slug: lowercase letters, digits, single hyphens), starts with exactly
  `## <same date> — <title>`, has no other `## ` heading, and ends with one
  newline.

Usage: python3 scripts/check_improvement_feed.py [--root DIR]
Exits 1 and prints one line per problem. scripts/check_current_state.py calls
check() too, so CI runs it on every pull request.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys

ENTRY_POINT = "BRAUSER_IMPROVEMENT_FEED.md"
FEED_DIR = pathlib.PurePosixPath("docs/improvement-feed")
TITLE = "# Brauser Improvement Feed"
NAME_PATTERN = re.compile(r"^(\d{4}-\d{2}-\d{2})-[a-z0-9]+(?:-[a-z0-9]+)*\.md$")
HEADING_PATTERN = re.compile(r"^## (\d{4}-\d{2}-\d{2}) — \S.*$")
DATED_HEADING = re.compile(r"^## \d{4}-\d{2}-\d{2}\b", re.MULTILINE)


def check(root: pathlib.Path) -> list[str]:
    errors: list[str] = []
    entry_point = root / ENTRY_POINT
    try:
        text = entry_point.read_text(encoding="utf-8")
    except OSError as error:
        return [f"{ENTRY_POINT}: cannot read it: {error}"]
    if text.splitlines()[:1] != [TITLE]:
        errors.append(f"{ENTRY_POINT}: first line must be {TITLE!r}")
    if DATED_HEADING.search(text):
        errors.append(f"{ENTRY_POINT}: holds a dated entry; put each entry in its own file under {FEED_DIR}/")

    feed = root / FEED_DIR
    if not feed.is_dir():
        return errors + [f"{FEED_DIR}/ is missing"]
    for path in sorted(feed.iterdir()):
        where = f"{FEED_DIR}/{path.name}"
        name = NAME_PATTERN.match(path.name)
        if not path.is_file() or name is None:
            errors.append(f"{where}: not a feed entry; name entries YYYY-MM-DD-<slug>.md (lowercase slug)")
            continue
        try:
            body = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as error:
            errors.append(f"{where}: cannot read it: {error}")
            continue
        lines = body.splitlines()
        heading = HEADING_PATTERN.match(lines[0]) if lines else None
        if heading is None:
            errors.append(f"{where}: first line must be '## YYYY-MM-DD — Title'")
        elif heading.group(1) != name.group(1):
            errors.append(f"{where}: heading date {heading.group(1)} does not match the file name")
        if any(line.startswith("## ") for line in lines[1:]):
            errors.append(f"{where}: holds more than one entry heading; one entry per file")
        if not body.endswith("\n") or body.endswith("\n\n"):
            errors.append(f"{where}: must end with exactly one newline")
    return errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).resolve().parent.parent)
    args = parser.parse_args(argv)
    errors = check(args.root)
    for error in errors:
        print(error, file=sys.stderr)
    return int(bool(errors))


if __name__ == "__main__":
    raise SystemExit(main())
