#!/usr/bin/env python3
"""Render BOSUN_FEATURE_BACKLOG.md's queue sections from .beads/issues.jsonl.

The beads queue is authoritative. The backlog's item lists are generated from
it between marker comments, so that no branch hand-edits them and two branches
that each add a bead never conflict on hand-written Markdown:

    <!-- BEGIN GENERATED section=<key>[ show-closed]: ... -->
    ...rendered from the beads, never edited by hand...
    <!-- END GENERATED section=<key> -->

Everything outside those markers (the preamble, section intros, Notes, cycle
observations, the older Shipped history) is hand-written and left untouched.

Where a bead goes is decided by its fields:

- status `in_progress` -> the `in-progress` block; so does a blocked or
  deferred bead whose `backlog_section` metadata is `in-progress`.
- metadata `backlog_section` naming another block -> that block. A closed
  bead stays there (checked) only when the block is marked `show-closed`.
- any other live bead (no section, an unknown one, or an unclaimed bead
  still marked `in-progress`) -> the `new` block, so nothing live is hidden.
- a closed bead with metadata `backlog_shipped` -> the `shipped` block,
  newest `closed_at` first.

An item's text is metadata `backlog_text` verbatim when present, else the
title, the lead of the description, and the priority and labels. Items are
ordered by `backlog_order`, then priority, `created_at` and id, so output never
depends on the JSONL's line order.

Usage:
    python3 scripts/render_backlog.py            # rewrite the backlog in place
    python3 scripts/render_backlog.py --check    # exit 1 when it is stale
    python3 scripts/render_backlog.py --resolve  # finish a merge/rebase conflict
    python3 scripts/render_backlog.py --install-merge-driver
    python3 scripts/render_backlog.py --merge-jsonl BASE OURS THEIRS  # git driver
"""

from __future__ import annotations

import argparse
import difflib
import json
import pathlib
import re
import shlex
import subprocess
import sys
import tempfile

LIVE_STATUSES = frozenset({"open", "in_progress", "blocked", "deferred"})
REQUIRED_SECTIONS = ("new", "in-progress", "shipped")
QUEUE_PATH = ".beads/issues.jsonl"
BACKLOG_PATH = "BOSUN_FEATURE_BACKLOG.md"
MERGE_DRIVER = "beads-jsonl"
LEAD_LIMIT = 280

BEGIN_PATTERN = re.compile(
    r"^<!-- BEGIN GENERATED section=([a-z0-9][a-z0-9-]*)( show-closed)?(?::[^\n]*?)? -->$"
)
END_PATTERN = re.compile(r"^<!-- END GENERATED section=([a-z0-9][a-z0-9-]*) -->$")
ANY_MARKER_PATTERN = re.compile(r"^<!-- (BEGIN|END) GENERATED\b")
STRING_METADATA = ("backlog_section", "backlog_text", "backlog_shipped")


class BacklogError(Exception):
    """The queue or the backlog's markers cannot be read."""


def begin_marker(section: str, show_closed: bool) -> str:
    flag = " show-closed" if show_closed else ""
    return (
        f"<!-- BEGIN GENERATED section={section}{flag}: "
        f"edit the beads, then run python3 scripts/render_backlog.py -->"
    )


def end_marker(section: str) -> str:
    return f"<!-- END GENERATED section={section} -->"


# --------------------------------------------------------------------------
# Queue


def parse_issues(text: str, source: str) -> dict[str, dict[str, object]]:
    """Parse a beads JSONL export into {id: record}, validating backlog metadata."""
    issues: dict[str, dict[str, object]] = {}
    for number, raw in enumerate(text.splitlines(), start=1):
        line = raw.strip()
        if not line:
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise BacklogError(f"{source}:{number}: not valid JSON: {error}") from error
        if not isinstance(record, dict):
            raise BacklogError(f"{source}:{number}: record is not a JSON object")
        issue_id = record.get("id")
        if not isinstance(issue_id, str) or not issue_id:
            raise BacklogError(f"{source}:{number}: record has no usable 'id'")
        if issue_id in issues:
            raise BacklogError(f"{source}:{number}: duplicate issue id {issue_id}")
        metadata = record.get("metadata")
        if metadata is not None and not isinstance(metadata, dict):
            raise BacklogError(f"{source}:{number}: {issue_id} metadata is not an object")
        metadata = metadata or {}
        for key in STRING_METADATA:
            if key in metadata and not isinstance(metadata[key], str):
                raise BacklogError(f"{source}:{number}: {issue_id} {key} must be a string")
        order = metadata.get("backlog_order")
        if order is not None and (isinstance(order, bool) or not isinstance(order, int)):
            raise BacklogError(f"{source}:{number}: {issue_id} backlog_order must be an integer")
        issues[issue_id] = record
    return issues


def load_issues(path: pathlib.Path) -> dict[str, dict[str, object]]:
    return parse_issues(path.read_text(encoding="utf-8"), str(path))


def meta(issue: dict[str, object], key: str) -> object:
    metadata = issue.get("metadata")
    return metadata.get(key) if isinstance(metadata, dict) else None


def status_of(issue: dict[str, object]) -> str:
    return str(issue.get("status") or "")


def is_live(issue: dict[str, object]) -> bool:
    return status_of(issue) in LIVE_STATUSES


# --------------------------------------------------------------------------
# Markers


class Block:
    def __init__(self, section: str, show_closed: bool, start: int, end: int) -> None:
        self.section = section
        self.show_closed = show_closed
        self.start = start  # index of the BEGIN line
        self.end = end  # index of the END line


def find_blocks(lines: list[str]) -> list[Block]:
    blocks: list[Block] = []
    seen: set[str] = set()
    open_block: tuple[str, bool, int] | None = None
    for index, line in enumerate(lines):
        if not ANY_MARKER_PATTERN.match(line):
            continue
        begin = BEGIN_PATTERN.match(line)
        end = END_PATTERN.match(line)
        where = f"line {index + 1}"
        if begin:
            if open_block is not None:
                raise BacklogError(f"{where}: BEGIN GENERATED nested inside section={open_block[0]}")
            section = begin.group(1)
            if section in seen:
                raise BacklogError(f"{where}: section={section} is generated more than once")
            seen.add(section)
            open_block = (section, bool(begin.group(2)), index)
        elif end:
            if open_block is None:
                raise BacklogError(f"{where}: END GENERATED without a BEGIN")
            if end.group(1) != open_block[0]:
                raise BacklogError(
                    f"{where}: END section={end.group(1)} does not match "
                    f"BEGIN section={open_block[0]}"
                )
            blocks.append(Block(open_block[0], open_block[1], open_block[2], index))
            open_block = None
        else:
            raise BacklogError(f"{where}: malformed GENERATED marker: {line}")
    if open_block is not None:
        raise BacklogError(f"section={open_block[0]} is not closed with an END GENERATED marker")
    missing = [section for section in REQUIRED_SECTIONS if section not in seen]
    if missing:
        raise BacklogError(f"required generated section(s) missing: {', '.join(missing)}")
    return blocks


def split_lines(text: str) -> list[str]:
    return text.replace("\r\n", "\n").replace("\r", "\n").split("\n")


def section_keys(text: str) -> set[str]:
    return {block.section for block in find_blocks(split_lines(text))}


# --------------------------------------------------------------------------
# Rendering


def sort_key(issue: dict[str, object]) -> tuple[object, ...]:
    order = meta(issue, "backlog_order")
    priority = issue.get("priority")
    return (
        (0, order) if isinstance(order, int) else (1, 0),
        priority if isinstance(priority, int) and not isinstance(priority, bool) else 99,
        str(issue.get("created_at") or ""),
        str(issue.get("id")),
    )


def description_lead(description: str) -> str:
    for paragraph in re.split(r"\n\s*\n", description.replace("\r\n", "\n")):
        lines = [line for line in paragraph.splitlines() if not line.lstrip().startswith("#")]
        lead = " ".join(" ".join(lines).split())
        if lead:
            break
    else:
        return ""
    if len(lead) <= LEAD_LIMIT:
        return lead
    cut = lead.rfind(" ", 0, LEAD_LIMIT)
    return lead[: cut if cut > 0 else LEAD_LIMIT].rstrip(" ,;:") + "…"


def fallback_text(issue: dict[str, object], *, show_assignee: bool) -> str:
    issue_id = str(issue["id"])
    title = " ".join(str(issue.get("title") or issue_id).split())
    if not title.endswith((".", "!", "?")):
        title += "."
    parts = [f"`{issue_id}` **{title}**"]
    lead = description_lead(str(issue.get("description") or ""))
    if lead:
        parts.append(lead)
    details = []
    priority = issue.get("priority")
    if isinstance(priority, int) and not isinstance(priority, bool):
        details.append(f"P{priority}")
    status = status_of(issue)
    if status in ("blocked", "deferred"):
        details.append(status)
    assignee = issue.get("assignee")
    if show_assignee and isinstance(assignee, str) and assignee:
        details.append(f"assignee: {assignee}")
    labels = issue.get("labels")
    if isinstance(labels, list) and labels:
        details.append("labels: " + ", ".join(sorted(str(label) for label in labels)))
    if details:
        parts.append(f"_({'; '.join(details)})_")
    return " ".join(parts)


def item_text(issue: dict[str, object], *, show_assignee: bool) -> str:
    text = meta(issue, "backlog_text")
    if not isinstance(text, str) or not text.strip():
        return fallback_text(issue, show_assignee=show_assignee)
    body = "\n".join(line.rstrip() for line in split_lines(text.strip("\n"))).strip()
    issue_id = str(issue["id"])
    if not body.startswith(f"`{issue_id}`"):
        body = f"`{issue_id}` {body}"
    return body


def place(issues: dict[str, dict[str, object]], blocks: list[Block]) -> dict[str, list[dict[str, object]]]:
    """Assign beads to blocks. Every live bead lands in exactly one non-shipped block."""
    by_key = {block.section: block for block in blocks}
    placed: dict[str, list[dict[str, object]]] = {block.section: [] for block in blocks}
    for issue in issues.values():
        status = status_of(issue)
        section = meta(issue, "backlog_section")
        if status == "closed" and isinstance(meta(issue, "backlog_shipped"), str):
            placed["shipped"].append(issue)
        if status == "in_progress":
            placed["in-progress"].append(issue)
            continue
        if section == "in-progress":
            if status in ("blocked", "deferred"):
                placed["in-progress"].append(issue)
            elif is_live(issue):
                placed["new"].append(issue)
            continue
        target = by_key.get(section) if isinstance(section, str) else None
        if target is not None and target.section not in ("shipped", "new"):
            if is_live(issue) or (status == "closed" and target.show_closed):
                placed[target.section].append(issue)
                continue
        if is_live(issue):
            placed["new"].append(issue)
    return placed


def render_block(block: Block, issues: list[dict[str, object]]) -> list[str]:
    lines: list[str] = []
    if block.section == "shipped":
        ordered = sorted(
            issues, key=lambda i: (str(i.get("closed_at") or ""), str(i["id"])), reverse=True
        )
        for issue in ordered:
            text = str(meta(issue, "backlog_shipped")).strip()
            lines.extend(f"- {text}".split("\n") if text else [])
            lines.append("")
        return lines
    for issue in sorted(issues, key=sort_key):
        box = "[x]" if status_of(issue) == "closed" else "[ ]"
        text = item_text(issue, show_assignee=block.section == "in-progress")
        lines.extend(f"- {box} {text}".split("\n"))
    return lines


def render(backlog: str, issues: dict[str, dict[str, object]]) -> str:
    """Return the backlog with every generated block re-rendered from the queue."""
    lines = split_lines(backlog)
    blocks = find_blocks(lines)
    placed = place(issues, blocks)
    out: list[str] = []
    cursor = 0
    for block in blocks:
        out.extend(lines[cursor : block.start])
        out.append(begin_marker(block.section, block.show_closed))
        out.extend(render_block(block, placed[block.section]))
        out.append(end_marker(block.section))
        cursor = block.end + 1
    out.extend(lines[cursor:])
    return "\n".join(out)


def strip_generated(backlog: str) -> str:
    """Return the hand-written skeleton: every generated block emptied."""
    lines = split_lines(backlog)
    blocks = find_blocks(lines)
    out: list[str] = []
    cursor = 0
    for block in blocks:
        out.extend(lines[cursor : block.start])
        out.append(begin_marker(block.section, block.show_closed))
        out.append(end_marker(block.section))
        cursor = block.end + 1
    out.extend(lines[cursor:])
    return "\n".join(out)


def stale_sections(backlog: str, issues: dict[str, dict[str, object]]) -> list[str]:
    """Sections whose generated content differs from a fresh render."""
    current = split_lines(backlog)
    fresh = split_lines(render(backlog, issues))
    return [
        block.section
        for block, fresh_block in zip(find_blocks(current), find_blocks(fresh))
        if current[block.start : block.end + 1] != fresh[fresh_block.start : fresh_block.end + 1]
    ] or (["(markers)"] if current != fresh else [])


# --------------------------------------------------------------------------
# Three-way merge of the JSONL queue, keyed by bead id


def _records(text: str, label: str) -> tuple[list[str], dict[str, tuple[str, object]]]:
    order: list[str] = []
    records: dict[str, tuple[str, object]] = {}
    for number, raw in enumerate(text.replace("\r\n", "\n").split("\n"), start=1):
        line = raw.strip()
        if not line:
            continue
        try:
            record = json.loads(line)
        except json.JSONDecodeError as error:
            raise BacklogError(f"{label}:{number}: not valid JSON: {error}") from error
        issue_id = record.get("id") if isinstance(record, dict) else None
        if not isinstance(issue_id, str) or not issue_id or issue_id in records:
            raise BacklogError(f"{label}:{number}: missing or duplicate id")
        order.append(issue_id)
        records[issue_id] = (line, record)
    return order, records


def merge_jsonl(base: str, ours: str, theirs: str) -> tuple[str, list[str]]:
    """Merge three versions of the queue record by record.

    A bead changed on one side only takes that side; the same change on both
    is taken once; different changes to one bead (or a delete against a
    change) are a conflict, written with ours/theirs markers on that bead's
    lines only. Records compare as JSON, so key order never matters. Output
    follows ours' line order, with beads only theirs has appended in theirs'
    order; each kept line is copied byte for byte from the side it came from.
    """
    _, base_records = _records(base, "base")
    ours_order, ours_records = _records(ours, "ours")
    theirs_order, theirs_records = _records(theirs, "theirs")
    out: list[str] = []
    conflicts: list[str] = []

    def resolve(issue_id: str) -> None:
        b = base_records.get(issue_id)
        o = ours_records.get(issue_id)
        t = theirs_records.get(issue_id)
        b_val, o_val, t_val = (x[1] if x else None for x in (b, o, t))
        if o_val == t_val:
            chosen = o
        elif o_val == b_val:
            chosen = t
        elif t_val == b_val:
            chosen = o
        else:
            conflicts.append(issue_id)
            out.append("<<<<<<< ours")
            if o:
                out.append(o[0])
            out.append("=======")
            if t:
                out.append(t[0])
            out.append(">>>>>>> theirs")
            return
        if chosen is not None:
            out.append(chosen[0])

    for issue_id in ours_order:
        resolve(issue_id)
    for issue_id in theirs_order:
        if issue_id not in ours_records:
            resolve(issue_id)
    return "".join(line + "\n" for line in out), conflicts


# --------------------------------------------------------------------------
# Git helpers


def git(*args: str, cwd: pathlib.Path, check: bool = True, input_text: str | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True, input=input_text,
        encoding="utf-8",
    )
    if check and result.returncode != 0:
        raise BacklogError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result


def repo_root(start: pathlib.Path) -> pathlib.Path:
    return pathlib.Path(git("rev-parse", "--show-toplevel", cwd=start).stdout.strip())


def stage(root: pathlib.Path, number: int, path: str) -> str:
    result = git("show", f":{number}:{path}", cwd=root, check=False)
    return result.stdout if result.returncode == 0 else ""


def unmerged(root: pathlib.Path) -> set[str]:
    out = git("diff", "--name-only", "--diff-filter=U", cwd=root).stdout
    return {line.strip() for line in out.splitlines() if line.strip()}


def merge_text(root: pathlib.Path, base: str, ours: str, theirs: str) -> tuple[str, int]:
    with tempfile.TemporaryDirectory() as tmp:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            path = pathlib.Path(tmp) / name
            path.write_text(text, encoding="utf-8", newline="\n")
            paths.append(str(path))
        result = git(
            "merge-file", "-p", "-L", "ours", "-L", "base", "-L", "theirs", *paths,
            cwd=root, check=False,
        )
        if result.returncode < 0 or result.returncode > 127:
            raise BacklogError(f"git merge-file failed: {result.stderr.strip()}")
        return result.stdout, result.returncode


def write(path: pathlib.Path, text: str) -> None:
    path.write_text(text, encoding="utf-8", newline="\n")


CONFLICT_MARKER_PATTERN = re.compile(r"^(<{7}|={7}|>{7}|\|{7})( |$)", re.MULTILINE)


def hand_resolved(path: pathlib.Path) -> bool:
    """True when a conflicted file's working copy no longer has conflict markers.

    A rerun after the user fixed the markers by hand must keep that fix rather
    than merge the index stages again.
    """
    try:
        return not CONFLICT_MARKER_PATTERN.search(path.read_text(encoding="utf-8"))
    except OSError:
        return False


def resolve(root: pathlib.Path) -> int:
    """Finish a merge, rebase or cherry-pick whose queue or backlog conflicted."""
    conflicted = unmerged(root)
    queue_file = root / QUEUE_PATH
    backlog_file = root / BACKLOG_PATH
    status = 0

    if QUEUE_PATH in conflicted and hand_resolved(queue_file):
        load_issues(queue_file)  # a hand resolution must still parse
        git("add", "--", QUEUE_PATH, cwd=root)
        print(f"{QUEUE_PATH}: kept your resolution")
    elif QUEUE_PATH in conflicted:
        text, conflicts = merge_jsonl(
            stage(root, 1, QUEUE_PATH), stage(root, 2, QUEUE_PATH), stage(root, 3, QUEUE_PATH)
        )
        write(queue_file, text)
        if conflicts:
            print(
                f"{QUEUE_PATH}: {len(conflicts)} bead(s) changed differently on both sides: "
                f"{', '.join(conflicts)}. Edit those lines (keep one record per bead), "
                f"then run this command again."
            )
            return 1
        git("add", "--", QUEUE_PATH, cwd=root)
        print(f"{QUEUE_PATH}: merged bead by bead")

    issues = load_issues(queue_file)

    if BACKLOG_PATH in conflicted and hand_resolved(backlog_file):
        write(backlog_file, render(backlog_file.read_text(encoding="utf-8"), issues))
        git("add", "--", BACKLOG_PATH, cwd=root)
        print(f"{BACKLOG_PATH}: kept your resolution, generated sections re-rendered")
    elif BACKLOG_PATH in conflicted:
        skeletons = [strip_generated(stage(root, number, BACKLOG_PATH)) for number in (1, 2, 3)]
        merged, prose_conflicts = merge_text(root, *skeletons)
        try:
            rendered = render(merged, issues)
        except BacklogError as error:
            write(backlog_file, merged)
            print(f"{BACKLOG_PATH}: a conflict crosses a GENERATED marker ({error}); fix the markers by hand, then rerun")
            return 1
        write(backlog_file, rendered)
        if prose_conflicts:
            print(
                f"{BACKLOG_PATH}: generated sections re-rendered, but {prose_conflicts} "
                f"hand-written conflict(s) remain; resolve the markers, then rerun"
            )
            status = 1
        else:
            git("add", "--", BACKLOG_PATH, cwd=root)
            print(f"{BACKLOG_PATH}: hand-written text merged, generated sections re-rendered")
    else:
        current = backlog_file.read_text(encoding="utf-8")
        rendered = render(current, issues)
        if rendered != current.replace("\r\n", "\n"):
            write(backlog_file, rendered)
            print(f"{BACKLOG_PATH}: generated sections re-rendered")
        if QUEUE_PATH in conflicted or git("diff", "--quiet", "--", BACKLOG_PATH, cwd=root, check=False).returncode:
            git("add", "--", BACKLOG_PATH, cwd=root)
    return status


def install_merge_driver(root: pathlib.Path) -> None:
    python = shlex.quote(pathlib.Path(sys.executable).as_posix())
    git("config", f"merge.{MERGE_DRIVER}.name", "beads JSONL merge, bead by bead", cwd=root)
    # A branch from before the renderer has no script in its working tree; a
    # failed driver would leave only ours' queue, so fall back to a line merge.
    git(
        "config", f"merge.{MERGE_DRIVER}.driver",
        f"if test -f scripts/render_backlog.py; then "
        f"{python} scripts/render_backlog.py --merge-jsonl %O %A %B; "
        f"else git merge-file -L ours -L base -L theirs %A %O %B; fi",
        cwd=root,
    )
    print(f"installed git merge driver '{MERGE_DRIVER}' for {QUEUE_PATH} in this clone")


def merge_driver(base: pathlib.Path, ours: pathlib.Path, theirs: pathlib.Path) -> int:
    """`git merge` driver entry point: write the merge into OURS, exit 1 on conflict."""
    texts = [path.read_text(encoding="utf-8") for path in (base, ours, theirs)]
    try:
        merged, conflicts = merge_jsonl(*texts)
    except BacklogError as error:
        # Not something we can merge record by record: fall back to the
        # ordinary line merge so neither side is silently dropped.
        print(f"beads-jsonl merge driver: {error}; using a line merge", file=sys.stderr)
        result = subprocess.run(
            ["git", "merge-file", "-L", "ours", "-L", "base", "-L", "theirs",
             str(ours), str(base), str(theirs)],
            capture_output=True, text=True,
        )
        return 0 if result.returncode == 0 else 1
    write(ours, merged)
    return 1 if conflicts else 0


# --------------------------------------------------------------------------
# CLI


def main(argv: list[str] | None = None) -> int:
    default_root = pathlib.Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--issues", type=pathlib.Path, default=default_root / QUEUE_PATH)
    parser.add_argument("--backlog", type=pathlib.Path, default=default_root / BACKLOG_PATH)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="exit 1 if the backlog is stale")
    mode.add_argument("--resolve", action="store_true",
                      help="finish a conflicted merge/rebase of the queue and backlog")
    mode.add_argument("--install-merge-driver", action="store_true",
                      help="register the bead-by-bead JSONL merge driver in this clone")
    mode.add_argument("--merge-jsonl", nargs=3, type=pathlib.Path,
                      metavar=("BASE", "OURS", "THEIRS"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    try:
        if args.merge_jsonl:
            return merge_driver(*args.merge_jsonl)
        if args.install_merge_driver:
            install_merge_driver(repo_root(pathlib.Path.cwd()))
            return 0
        if args.resolve:
            return resolve(repo_root(pathlib.Path.cwd()))
        issues = load_issues(args.issues)
        current = args.backlog.read_text(encoding="utf-8")
        rendered = render(current, issues)
        if args.check:
            if rendered == current.replace("\r\n", "\n"):
                print(f"{args.backlog.name}: generated sections are up to date")
                return 0
            sys.stdout.writelines(
                difflib.unified_diff(
                    current.replace("\r\n", "\n").splitlines(keepends=True),
                    rendered.splitlines(keepends=True),
                    fromfile=f"{args.backlog.name} (committed)",
                    tofile=f"{args.backlog.name} (rendered)",
                )
            )
            print(f"\n{args.backlog.name} is stale: run python3 scripts/render_backlog.py")
            return 1
        if rendered != current:
            write(args.backlog, rendered)
            print(f"{args.backlog.name}: generated sections re-rendered")
        else:
            print(f"{args.backlog.name}: already up to date")
        return 0
    except (BacklogError, OSError) as error:
        print(f"render_backlog: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
