#!/usr/bin/env python3
"""Render BOSUN_FEATURE_BACKLOG.md's queue sections from .beads/issues.jsonl.

Both files live on the `beads` branch, not on main (AGENTS.md "Work queue");
by default this reads and writes them in the clone's queue worktree
(`scripts/dev/queue path`, or $BRAUSER_QUEUE_DIR). The beads queue is
authoritative. The backlog's item lists are generated from
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
    python3 scripts/render_backlog.py            # rewrite the queue worktree's backlog
    python3 scripts/render_backlog.py --check    # exit 1 when it is stale
    python3 scripts/render_backlog.py --resolve  # finish a conflict (run in the queue worktree)
    python3 scripts/render_backlog.py --install-merge-driver
    python3 scripts/render_backlog.py --merge-jsonl BASE OURS THEIRS    # git driver
    python3 scripts/render_backlog.py --merge-backlog BASE OURS THEIRS  # git driver
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
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
BACKLOG_DRIVER = "bosun-backlog"
RENDERER_ENV = "BRAUSER_RENDER_BACKLOG"  # work_queue.RENDERER_ENV
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

    # A merge driver that fails outright (a stale interpreter path, a crash)
    # leaves ours' bytes with no markers; that is not a hand resolution, and
    # keeping it would silently drop theirs' beads.
    ours_untouched = (
        queue_file.is_file()
        and queue_file.read_text(encoding="utf-8") == stage(root, 2, QUEUE_PATH)
        and stage(root, 2, QUEUE_PATH) != stage(root, 3, QUEUE_PATH)
    )
    if QUEUE_PATH in conflicted and hand_resolved(queue_file) and not ours_untouched:
        load_issues(queue_file)  # a hand resolution must still parse
        git("add", "--", QUEUE_PATH, cwd=root)
        print(f"{QUEUE_PATH}: kept your resolution")
    elif QUEUE_PATH in conflicted:
        text, conflicts, notes = merge_jsonl(
            stage(root, 1, QUEUE_PATH), stage(root, 2, QUEUE_PATH), stage(root, 3, QUEUE_PATH)
        )
        write(queue_file, text)
        for note in notes:
            print(f"{QUEUE_PATH}: {note}")
        if conflicts:
            print(
                f"{QUEUE_PATH}: {len(conflicts)} bead(s) changed differently on both sides: "
                f"{', '.join(conflicts)}. Edit those lines (keep one record per bead), "
                f"then run this command again (to keep ours exactly, `git add` the file yourself)."
            )
            return 1
        git("add", "--", QUEUE_PATH, cwd=root)
        print(f"{QUEUE_PATH}: merged bead by bead")

    issues = load_issues(queue_file)

    # Likewise a backlog driver that never ran leaves ours' bytes behind.
    backlog_untouched = (
        backlog_file.is_file()
        and backlog_file.read_text(encoding="utf-8") == stage(root, 2, BACKLOG_PATH)
        and stage(root, 2, BACKLOG_PATH) != stage(root, 3, BACKLOG_PATH)
    )
    if BACKLOG_PATH in conflicted and hand_resolved(backlog_file) and not backlog_untouched:
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
                f"hand-written conflict(s) remain; resolve the markers, then rerun "
                f"(to keep ours exactly, `git add` the file yourself)"
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
    """Register both merge drivers `.gitattributes` names, in this clone only.

    Idempotent: rerunning rewrites the same two config entries (and picks up
    a new interpreter path). Each driver falls back to git's ordinary line
    merge when the branch being merged into has no renderer script, so a
    failed driver can never leave only ours' side behind.
    """
    python = shlex.quote(pathlib.Path(sys.executable).as_posix())
    fallback = "git merge-file -L ours -L base -L theirs %A %O %B"
    # The queue worktree (the beads branch) has no scripts/, so there the
    # driver runs the renderer `scripts/dev/queue` names in the environment.
    for name, label, flag, path in (
        (MERGE_DRIVER, "beads JSONL merge, bead by bead", "--merge-jsonl", QUEUE_PATH),
        (BACKLOG_DRIVER, "backlog merge, generated sections re-rendered", "--merge-backlog", BACKLOG_PATH),
    ):
        git("config", f"merge.{name}.name", label, cwd=root)
        git(
            "config", f"merge.{name}.driver",
            f"r=scripts/render_backlog.py; test -f \"$r\" || r=\"${{{RENDERER_ENV}:-}}\"; "
            f"if test -n \"$r\" && test -f \"$r\"; then "
            f"{python} \"$r\" {flag} %O %A %B; "
            f"else {fallback}; fi",
            cwd=root,
        )
        print(f"installed git merge driver '{name}' for {path} in this clone")


def merge_driver(base: pathlib.Path, ours: pathlib.Path, theirs: pathlib.Path) -> int:
    """`git merge` driver for the queue: write the merge into OURS, exit 1 on conflict."""
    texts = [path.read_text(encoding="utf-8") for path in (base, ours, theirs)]
    try:
        merged, conflicts, notes = merge_jsonl(*texts)
    except BacklogError as error:
        # Not something we can merge record by record: fall back to the
        # ordinary line merge so neither side is silently dropped.
        print(f"beads-jsonl merge driver: {error}; using a line merge", file=sys.stderr)
        return _line_merge(base, ours, theirs)
    for note in notes:
        print(f"beads-jsonl merge driver: {note}", file=sys.stderr)
    write(ours, merged)
    if conflicts:
        print(
            f"beads-jsonl merge driver: {len(conflicts)} bead(s) need a hand merge: "
            f"{', '.join(conflicts)}; then run python3 scripts/render_backlog.py --resolve",
            file=sys.stderr,
        )
    return 1 if conflicts else 0


def _line_merge(base: pathlib.Path, ours: pathlib.Path, theirs: pathlib.Path) -> int:
    result = subprocess.run(
        ["git", "merge-file", "-L", "ours", "-L", "base", "-L", "theirs",
         str(ours), str(base), str(theirs)],
        capture_output=True, text=True,
    )
    return 0 if result.returncode == 0 else 1


def _show(root: pathlib.Path, rev: str, path: str) -> str:
    result = git("show", f"{rev}:{path}", cwd=root, check=False)
    return result.stdout if result.returncode == 0 else ""


def _commit(root: pathlib.Path, name: str) -> str | None:
    result = git("rev-parse", "-q", "--verify", f"{name}^{{commit}}", cwd=root, check=False)
    return result.stdout.strip() if result.returncode == 0 else None


def _candidates(root: pathlib.Path) -> list[tuple[str, str]]:
    """Commits that may be the side being merged in, with how to find its base.

    git tells a merge driver nothing about the commits it merges, only the
    three versions of one file. `git merge` and `git pull` export the merged
    head as GITHEAD_<oid> (older git) and name it in GIT_REFLOG_ACTION (a
    pull merges FETCH_HEAD); a rebase lists the commit it is picking last in
    rebase-merge/done. Each guess is checked against the driver's own input
    before it is used.
    """
    found: list[tuple[str, str]] = []
    for key in sorted(os.environ):
        if key.startswith("GITHEAD_"):
            found.append(("merge", key[len("GITHEAD_"):]))
    action = os.environ.get("GIT_REFLOG_ACTION", "").split()
    if action[:1] == ["merge"]:
        found.extend(("merge", name) for name in action[1:] if not name.startswith("-"))
    if action[:1] == ["pull"]:
        found.append(("merge", "FETCH_HEAD"))
    done = pathlib.Path(git("rev-parse", "--git-path", "rebase-merge/done", cwd=root).stdout.strip())
    if not done.is_absolute():
        done = root / done
    if done.is_file():
        steps = [line.split() for line in done.read_text(encoding="utf-8").splitlines()]
        steps = [step for step in steps if step and not step[0].startswith("#")]
        if steps and steps[-1][0] in {"pick", "p", "reword", "r", "edit", "e", "fixup", "f", "squash", "s"}:
            names = [token for token in steps[-1][1:] if not token.startswith("-")]
            if names:
                found.append(("pick", names[0]))
    return found


def merge_sides(root: pathlib.Path, base: str, ours: str, theirs: str) -> tuple[str, str, str] | None:
    """The base, ours and theirs queues behind the backlog git is merging.

    Ours is HEAD in a merge, a rebase and a cherry-pick alike. Theirs and the
    base are only accepted when that commit's backlog is byte for byte the
    file git handed the driver (and there is exactly one merge base), so a
    wrong guess falls back rather than rendering from the wrong queue.
    Returns None when the commits cannot be established.
    """
    def same(rev: str, text: str) -> bool:
        return _show(root, rev, BACKLOG_PATH).replace("\r\n", "\n") == text

    if not same("HEAD", ours):
        return None
    for kind, name in _candidates(root):
        commit = _commit(root, name)
        if commit is None or not same(commit, theirs):
            continue
        if kind == "merge":
            result = git("merge-base", "--all", "HEAD", commit, cwd=root, check=False)
            bases = result.stdout.split() if result.returncode == 0 else []
        else:
            parent = _commit(root, f"{commit}^")
            bases = [parent] if parent else []
        if len(bases) == 1 and same(bases[0], base):
            return tuple(_show(root, rev, QUEUE_PATH) for rev in (bases[0], "HEAD", commit))  # type: ignore[return-value]
    return None


def fill_blocks(skeleton: str, source: str) -> str:
    """The skeleton with each generated block's lines copied from SOURCE."""
    source_lines = split_lines(source)
    bodies = {block.section: source_lines[block.start + 1 : block.end] for block in find_blocks(source_lines)}
    lines = split_lines(skeleton)
    out: list[str] = []
    cursor = 0
    for block in find_blocks(lines):
        out.extend(lines[cursor : block.start + 1])
        out.extend(bodies.get(block.section, []))
        out.append(lines[block.end])
        cursor = block.end + 1
    out.extend(lines[cursor:])
    return "\n".join(out)


def merge_backlog(
    base: str, ours: str, theirs: str, queues: tuple[str, str, str] | None, *, cwd: pathlib.Path
) -> tuple[str, int, str | None]:
    """Merge three backlogs: hand-written text three ways, generated blocks re-rendered.

    The generated blocks are never text-merged. They are rendered from the
    merged queue, which is `merge_jsonl` of the three queues behind this
    merge (the same merge the queue's own driver makes). Returns (text,
    status, why): status 0 is a clean merge; 1 leaves the file conflicted,
    either for a hand-written conflict (markers in the text) or because the
    merged queue is unknown or itself conflicted, in which case the blocks
    hold ours' rendering and `--resolve` re-renders them.
    """
    base, ours, theirs = (text.replace("\r\n", "\n") for text in (base, ours, theirs))
    skeletons = [strip_generated(text) for text in (base, ours, theirs)]
    merged, prose_conflicts = merge_text(cwd, *skeletons)
    issues = None
    why = None
    if queues is None:
        why = "cannot tell which commits are being merged"
    else:
        try:
            queue, conflicts, _ = merge_jsonl(*queues)
            if conflicts:
                why = f"bead(s) {', '.join(conflicts)} need a hand merge"
            else:
                issues = parse_issues(queue, QUEUE_PATH)
        except BacklogError as error:
            why = f"the queue does not merge bead by bead ({error})"
    try:
        if issues is not None:
            return render(merged, issues), 1 if prose_conflicts else 0, None
        return fill_blocks(merged, ours), 1, why
    except BacklogError as error:
        # A hand-written conflict that swallowed a GENERATED marker.
        return merged, 1, f"a conflict crosses a GENERATED marker ({error})"


def backlog_driver(base: pathlib.Path, ours: pathlib.Path, theirs: pathlib.Path) -> int:
    """`git merge` driver for the backlog: write the merge into OURS, exit 1 on conflict."""
    texts = [path.read_text(encoding="utf-8") for path in (base, ours, theirs)]
    cwd = pathlib.Path.cwd()
    try:
        root = repo_root(cwd)
        queues = merge_sides(root, *(text.replace("\r\n", "\n") for text in texts))
        merged, status, why = merge_backlog(*texts, queues, cwd=root)
    except (BacklogError, OSError) as error:
        # Markers missing on one side (a branch from before the renderer) or
        # git unusable: fall back to the line merge, never to ours alone.
        print(f"bosun-backlog merge driver: {error}; using a line merge", file=sys.stderr)
        return _line_merge(base, ours, theirs)
    write(ours, merged)
    if why:
        print(
            f"bosun-backlog merge driver: {why}; "
            f"finish with python3 scripts/render_backlog.py --resolve",
            file=sys.stderr,
        )
    return status


# --------------------------------------------------------------------------
# CLI


def default_queue_dir() -> pathlib.Path:
    """$BRAUSER_QUEUE_DIR, else this clone's queue worktree (created on demand)."""
    override = os.environ.get("BRAUSER_QUEUE_DIR")  # work_queue.DIR_ENV
    if override:
        return pathlib.Path(override).expanduser()
    import work_queue  # local: it imports this module

    try:
        return work_queue.ensure(work_queue.repo_root())
    except work_queue.QueueError as error:
        raise BacklogError(f"cannot find the queue worktree: {error}") from error


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--issues", type=pathlib.Path,
                        help="the queue (default: the queue worktree's .beads/issues.jsonl)")
    parser.add_argument("--backlog", type=pathlib.Path,
                        help="the backlog (default: the queue worktree's BOSUN_FEATURE_BACKLOG.md)")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="exit 1 if the backlog is stale")
    mode.add_argument("--resolve", action="store_true",
                      help="finish a conflicted merge/rebase of the queue and backlog")
    mode.add_argument("--install-merge-driver", action="store_true",
                      help="register the queue and backlog merge drivers in this clone")
    mode.add_argument("--merge-jsonl", nargs=3, type=pathlib.Path,
                      metavar=("BASE", "OURS", "THEIRS"), help=argparse.SUPPRESS)
    mode.add_argument("--merge-backlog", nargs=3, type=pathlib.Path,
                      metavar=("BASE", "OURS", "THEIRS"), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)

    try:
        if args.merge_jsonl:
            return merge_driver(*args.merge_jsonl)
        if args.merge_backlog:
            return backlog_driver(*args.merge_backlog)
        if args.install_merge_driver:
            install_merge_driver(repo_root(pathlib.Path.cwd()))
            return 0
        if args.resolve:
            return resolve(repo_root(pathlib.Path.cwd()))
        if args.issues is None or args.backlog is None:
            where = default_queue_dir()
            args.issues = args.issues or where / QUEUE_PATH
            args.backlog = args.backlog or where / BACKLOG_PATH
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
