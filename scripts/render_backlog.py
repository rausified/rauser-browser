#!/usr/bin/env python3
"""Render Brauser's backlog view from the beads queue.

The queue lives in bd's Dolt database, synced through this repository's
refs/dolt/data (AGENTS.md "Work queue"); nothing in git holds it. The
backlog, BOSUN_FEATURE_BACKLOG.md, is a rendering and never committed: the
hand-written frame (docs/backlog/frame.md) plus the one-file-per-entry notes
(docs/backlog/notes/*.md), with each generated block filled from the beads:

    <!-- BEGIN GENERATED section=<key>[ show-closed]: ... -->
    ...rendered from the beads, never written by hand...
    <!-- END GENERATED section=<key> -->

Everything outside those markers (the preamble, section intros, Notes, cycle
observations, the older Shipped history) is hand-written and kept as is.

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

Usage (scripts/dev/queue backlog wraps this):
    python3 scripts/render_backlog.py                 # write target/queue/BOSUN_FEATURE_BACKLOG.md
    python3 scripts/render_backlog.py --print         # print it
    python3 scripts/render_backlog.py --issues FILE   # render a given bd export
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys

LIVE_STATUSES = frozenset({"open", "in_progress", "blocked", "deferred"})
REQUIRED_SECTIONS = ("new", "in-progress", "shipped")
# The two files that used to be committed; never commit them again.
QUEUE_PATH = ".beads/issues.jsonl"
BACKLOG_PATH = "BOSUN_FEATURE_BACKLOG.md"
REPO = pathlib.Path(__file__).resolve().parent.parent
FRAME_PATH = "docs/backlog/frame.md"
NOTES_DIR = "docs/backlog/notes"
RENDER_PATH = "target/queue/BOSUN_FEATURE_BACKLOG.md"
NOTES_HEADING = "## Cycle notes (docs/backlog/notes/)"
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


_MISSING = object()
_GO_ESCAPES = {"<": "\\u003c", ">": "\\u003e", "&": "\\u0026", "\u2028": "\\u2028", "\u2029": "\\u2029"}
_GO_ESCAPE_PATTERN = re.compile("[<>&\u2028\u2029]")


def dump_record(record: dict[str, object]) -> str:
    """Serialize a bead the way `bd export` (Go's encoding/json) writes it."""
    text = json.dumps(record, ensure_ascii=False, separators=(",", ":"))
    return _GO_ESCAPE_PATTERN.sub(lambda match: _GO_ESCAPES[match.group(0)], text)


def export_order(record: dict[str, object]) -> object:
    """`bd export`'s line order: priority, then newest `created_at`, then id."""
    priority = record.get("priority")
    created = record.get("created_at")
    return (
        priority if isinstance(priority, int) and not isinstance(priority, bool) else 1 << 30,
        _Descending(created if isinstance(created, str) else ""),
        str(record.get("id")),
    )


class _Descending:
    def __init__(self, value: str) -> None:
        self.value = value

    def __lt__(self, other: "_Descending") -> bool:
        return self.value > other.value

    def __eq__(self, other: object) -> bool:
        return isinstance(other, _Descending) and self.value == other.value


def _timestamp(record: dict[str, object] | None) -> str | None:
    """A normalized `updated_at` that sorts in time order, or None."""
    value = record.get("updated_at") if record else None
    if not isinstance(value, str):
        return None
    match = re.fullmatch(
        r"(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})(\.\d+)?(Z|[+-]\d{2}:\d{2})", value
    )
    if not match:
        return None
    import datetime  # local: only the merge path needs it

    try:
        stamp = datetime.datetime.fromisoformat(
            f"{match.group(1)}T{match.group(2)}.{(match.group(3) or '.')[1:].ljust(6, '0')[:6]}"
            f"{'+00:00' if match.group(4) == 'Z' else match.group(4)}"
        )
    except ValueError:  # a calendar-invalid date such as 2026-02-30
        return None
    return stamp.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")


def _after_both(ours: dict[str, object], theirs: dict[str, object]) -> str | None:
    """An `updated_at` one second after the later of the two sides', or None.

    A combined bead differs from both inputs, so it must be newer than both:
    `bd import`'s updated_at guard keeps a database row whose timestamp ties
    the imported one, which would silently drop the other side's fields when
    a workspace hydrates its database from the merged queue. The value only
    depends on the inputs, so every merge of the same versions agrees.
    """
    import datetime

    stamps = [_timestamp(ours), _timestamp(theirs)]
    if None in stamps:
        return None
    latest = datetime.datetime.strptime(max(stamps), "%Y-%m-%dT%H:%M:%S.%f")
    bumped = latest.replace(microsecond=0) + datetime.timedelta(seconds=1)
    return bumped.strftime("%Y-%m-%dT%H:%M:%SZ")


def _merge_values(base: object, ours: object, theirs: object, newer: str | None) -> tuple[object, bool]:
    """Merge one field three ways. Returns (value, irreconcilable)."""
    if ours == theirs:
        return ours, False
    if ours == base:
        return theirs, False
    if theirs == base:
        return ours, False
    if isinstance(ours, dict) and isinstance(theirs, dict) and (base is _MISSING or isinstance(base, dict)):
        return _merge_objects(base if isinstance(base, dict) else {}, ours, theirs, newer)
    if newer == "ours":
        return ours, False
    if newer == "theirs":
        return theirs, False
    return ours, True


def _merge_objects(
    base: dict[str, object], ours: dict[str, object], theirs: dict[str, object], newer: str | None
) -> tuple[dict[str, object], bool]:
    # Keep the struct's key order: the newer side's keys, with the other side's
    # extra keys slotted in after the key that precedes them there.
    first, second = (theirs, ours) if newer == "theirs" else (ours, theirs)
    keys = list(first)
    for index, key in enumerate(second):
        if key not in keys:
            previous = next((k for k in reversed(list(second)[:index]) if k in keys), None)
            keys.insert(keys.index(previous) + 1 if previous is not None else 0, key)
    merged: dict[str, object] = {}
    irreconcilable = False
    for key in keys:
        value, bad = _merge_values(
            base.get(key, _MISSING), ours.get(key, _MISSING), theirs.get(key, _MISSING), newer
        )
        irreconcilable = irreconcilable or bad
        if value is not _MISSING:
            merged[key] = value
    return merged, irreconcilable


def merge_record(
    base: dict[str, object] | None, ours: dict[str, object], theirs: dict[str, object]
) -> tuple[dict[str, object] | None, str]:
    """Merge one bead edited on both sides, field by field.

    A field changed on one side takes that side; `metadata` merges key by key
    the same way. A field both sides changed differently takes the side with
    the newer `updated_at`. `merge_jsonl` stamps a combined bead one second
    after both sides (`_after_both`).
    Returns (record, note); record is None when both sides changed the same
    field and their `updated_at` is equal or unreadable, which nothing can
    order.
    """
    ours_time, theirs_time = _timestamp(ours), _timestamp(theirs)
    if ours_time is None or theirs_time is None or ours_time == theirs_time:
        newer = None
    else:
        newer = "theirs" if theirs_time > ours_time else "ours"
    # Both sides carry the newer updated_at, so the field merge keeps it (in
    # its struct position) instead of judging it.
    stamp_source = theirs if newer == "theirs" else ours
    ours, theirs = dict(ours), dict(theirs)
    for side in (ours, theirs):
        if "updated_at" in stamp_source:
            side["updated_at"] = stamp_source["updated_at"]
        else:
            side.pop("updated_at", None)
    merged, irreconcilable = _merge_objects(base or {}, ours, theirs, newer)
    if irreconcilable:
        return None, "both sides changed the same field, and updated_at cannot say which is newer"
    if newer is None:
        return merged, "both sides changed different fields; combined them"
    return merged, f"combined both sides' fields; where both changed one, took {newer} (newer updated_at)"


def _records(text: str, label: str) -> dict[str, tuple[str, dict[str, object]]]:
    records: dict[str, tuple[str, dict[str, object]]] = {}
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
        records[issue_id] = (line, record)
    return records


def merge_jsonl(base: str, ours: str, theirs: str) -> tuple[str, list[str], list[str]]:
    """Merge three versions of the queue bead by bead.

    Returns (text, conflicts, notes). Per bead id:

    - changed on one side only, or the same on both: that version;
    - added on one side: kept; deleted on one side and unchanged on the
      other: deleted;
    - deleted on one side and changed on the other: the changed bead is kept
      (and noted), so a delete never silently discards an edit;
    - changed differently on both sides (or added on both): merged field by
      field (`merge_record`); only two different values for one field with an
      equal or unreadable `updated_at` is a conflict, written with
      ours/theirs markers around that bead's two lines.

    When only one side changed the file at all, that side's text is returned
    byte for byte, as git itself would. Otherwise lines are written in `bd
    export`'s order (priority, newest `created_at`, id); a bead taken whole
    from one side keeps that side's exact bytes, and a combined bead is
    serialized the way `bd export` writes it. Records compare as JSON, so key
    order never matters. No bead is dropped unless one side deleted it and
    the other left it as it was in base.
    """
    if theirs == base or ours == theirs:
        return ours, [], []
    if ours == base:
        return theirs, [], []
    base_records = _records(base, "base")
    ours_records = _records(ours, "ours")
    theirs_records = _records(theirs, "theirs")
    rows: list[tuple[object, list[str]]] = []
    conflicts: list[str] = []
    notes: list[str] = []

    for issue_id in {*ours_records, *theirs_records, *base_records}:
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
        elif o is None or t is None:
            chosen = o or t
            side = "ours" if t is None else "theirs"
            notes.append(f"{issue_id}: deleted on one side but changed on {side}; kept the changed bead")
        else:
            record, note = merge_record(b_val, o[1], t[1])
            if record is None:
                conflicts.append(issue_id)
                notes.append(f"{issue_id}: {note}")
                lines = ["<<<<<<< ours", o[0], "=======", t[0], ">>>>>>> theirs"]
                rows.append((export_order(o[1]), lines))
                continue
            notes.append(f"{issue_id}: {note}")
            if record == o_val:
                chosen = o
            elif record == t_val:
                chosen = t
            else:
                stamp = _after_both(o[1], t[1])
                if stamp is not None and "updated_at" in record:
                    record["updated_at"] = stamp
                rows.append((export_order(record), [dump_record(record)]))
                continue
        if chosen is not None:
            rows.append((export_order(chosen[1]), [chosen[0]]))
    rows.sort(key=lambda row: row[0])
    return "".join(line + "\n" for _, lines in rows for line in lines), sorted(conflicts), notes


# --------------------------------------------------------------------------
# Git helper


def git(*args: str, cwd: pathlib.Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8")
    if check and result.returncode != 0:
        raise BacklogError(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result


def write(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


# --------------------------------------------------------------------------
# The frame and its notes


def compose(frame: str, notes_dir: pathlib.Path | None) -> str:
    """The frame followed by every note under NOTES_DIR, in file-name order.

    A note is one Markdown file per entry (cycle observations, handoffs), so
    two branches that each add one never touch the same file. README.md in
    the directory explains the convention and is not a note.
    """
    frame = frame.replace("\r\n", "\n")
    notes = sorted(
        path for path in (notes_dir.glob("*.md") if notes_dir and notes_dir.is_dir() else [])
        if path.name != "README.md"
    )
    if not notes:
        return frame
    parts = [frame.rstrip("\n"), "", NOTES_HEADING, ""]
    for path in notes:
        parts += [path.read_text(encoding="utf-8").replace("\r\n", "\n").strip("\n"), ""]
    return "\n".join(parts)


# --------------------------------------------------------------------------
# CLI


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--issues", type=pathlib.Path,
                        help="a bd export (default: `scripts/dev/queue export` of this checkout's database)")
    parser.add_argument("--frame", type=pathlib.Path, default=REPO / FRAME_PATH,
                        help="the hand-written frame (default: %(default)s)")
    parser.add_argument("--notes", type=pathlib.Path, default=REPO / NOTES_DIR,
                        help="one-file-per-entry notes appended to the frame (default: %(default)s)")
    where = parser.add_mutually_exclusive_group()
    where.add_argument("--out", type=pathlib.Path, default=REPO / RENDER_PATH,
                       help="where to write the rendering (default: %(default)s, git-ignored)")
    where.add_argument("--print", action="store_true", help="print the rendering instead")
    args = parser.parse_args(argv)

    try:
        if args.issues is None:
            import work_queue  # local: it imports this module

            try:
                args.issues = work_queue.export(work_queue.repo_root())
            except work_queue.QueueError as error:
                raise BacklogError(str(error)) from error
        issues = load_issues(args.issues)
        rendered = render(compose(args.frame.read_text(encoding="utf-8"), args.notes), issues)
        if args.print:
            sys.stdout.write(rendered)
        else:
            write(args.out, rendered)
            print(f"rendered {args.out}")
        return 0
    except (BacklogError, OSError) as error:
        print(f"render_backlog: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
