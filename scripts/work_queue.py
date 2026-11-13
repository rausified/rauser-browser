#!/usr/bin/env python3
"""Brauser's work queue: beads in bd's Dolt database, synced through this repo.

The queue is not a file in git. bd keeps it in its Dolt database and syncs it
through this repository's `refs/dolt/data` (`bd sync`, which merges
concurrent edits field by field), so pull requests never carry it
(AGENTS.md "Work queue"; worker-contract.json is the contract every worker
follows). A fresh clone runs `scripts/dev/queue bootstrap`. The backlog
view, BOSUN_FEATURE_BACKLOG.md, is rendered on demand from
docs/backlog/frame.md, docs/backlog/notes/ and the beads, and never committed.

Commands (scripts/dev/queue is the entry point):

  bootstrap         `bd bootstrap` (once, in a fresh clone), keeping the
                    tracked .beads/config.yaml byte for byte: bd also writes
                    sync.remote there, which left the checkout dirty, but the
                    remote it needs is in the database (`bd dolt remote list`).
  sync              check this database's queue, pull, check the merged queue,
                    then `bd sync` (pull, conflict check, push). Any failed
                    check, or a remote that was never seeded, pushes nothing.
  export [PATH]     print the whole queue as JSONL (`bd export`) on stdout;
                    with PATH, also write it there. An empty queue is a
                    failure: a real queue never exports nothing.
  backlog [--print | --out PATH]
                    render BOSUN_FEATURE_BACKLOG.md, default
                    target/queue/BOSUN_FEATURE_BACKLOG.md (git-ignored)
  check             export, render and run the queue consistency check
  adopt --from REF [--base REV] [--dry-run]
                    apply the bead edits a branch committed in the old
                    .beads/issues.jsonl (REF's file against BASE's, default the
                    merge base with origin/main) to the database as field-level
                    updates through bd, then sync. A field this database also
                    changed differently, or a deleted bead, refuses the whole
                    adopt with nothing applied. PR doctor runs this.
  seed --remote URL --from JSONL [--extra JSONL]... [--yes]
                    one-shot: build a fresh database from JSONL plus extra rows
                    (each validated, ids unique), check it, and push it to a
                    remote that has no refs/dolt/data yet. Refuses otherwise.

Every write goes through bd (Dolt transactions); the files this writes are
per-checkout renderings under target/queue/, replaced atomically. Nothing
reads, edits and rewrites a shared file.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
from typing import Optional

SCRIPTS = pathlib.Path(__file__).resolve().parent
QUEUE_PATH = ".beads/issues.jsonl"  # the old committed export, read from branches only
EXPORT_PATH = "target/queue/issues.jsonl"
WORKSPACE_ENV = "BRAUSER_QUEUE_WORKSPACE"  # run bd here instead of the checkout
# The shared driver runs the contract's commands from a pristine checkout of
# main (cwd), naming the workspace whose database they act on in these.
BOSUN_WORKSPACE_ENV = "BOSUN_WORKSPACE"
BEADS_DIR_ENV = "BEADS_DIR"
DOLT_REF = "refs/dolt/data"
PREFIX = "brwr"
# Never prompt: the shared driver runs these with no TTY, a credential helper
# in GIT_CONFIG_* and GH_TOKEN in the environment, which pass through as is
# (so do BOSUN_WORKSPACE and BEADS_DIR, which bd reads itself).
BD_ENV = {"BD_NON_INTERACTIVE": "1", "GIT_TERMINAL_PROMPT": "0"}
# bd sync's documented exit codes.
SYNC_EXITS = {
    1: "error (transport, auth or storage)",
    2: "merge conflict: halted, nothing pushed; resolve it with `bd conflicts`, then sync again",
    3: "retries exhausted (a push race or another writer's pending edit); nothing pushed; sync again",
    4: "the working set is stuck; nothing pushed; an operator must clear it (`bd sync --help`)",
}


class QueueError(Exception):
    """A queue operation cannot go ahead; the message says why and what to do.

    `code` is the exit status: 1, or bd sync's own 2, 3 or 4 (SYNC_EXITS),
    which the shared driver branches on (worker-contract.json
    queue.sync_exit_codes).
    """

    def __init__(self, message: str, code: int = 1) -> None:
        super().__init__(message)
        self.code = code


def _modules():
    sys.path.insert(0, str(SCRIPTS))
    import check_queue_consistency  # noqa: PLC0415 (both import this module lazily)
    import render_backlog  # noqa: PLC0415

    return render_backlog, check_queue_consistency


def git(*args: str, cwd: pathlib.Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8",
                            stdin=subprocess.DEVNULL, env={**os.environ, "GIT_TERMINAL_PROMPT": "0"})
    if check and result.returncode != 0:
        raise QueueError(f"git {' '.join(args)} failed: {(result.stderr or result.stdout).strip()}")
    return result


def bd(*args: str, cwd: pathlib.Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    try:
        result = subprocess.run(["bd", *args], cwd=cwd, capture_output=True, text=True, encoding="utf-8",
                                stdin=subprocess.DEVNULL, env={**os.environ, **BD_ENV})
    except FileNotFoundError as error:
        raise QueueError("bd is not installed (https://github.com/gastownhall/beads)") from error
    if check and result.returncode != 0:
        raise QueueError(f"bd {' '.join(args)} failed ({result.returncode}): {(result.stderr or result.stdout).strip()}")
    return result


def repo_root(start: Optional[pathlib.Path] = None) -> pathlib.Path:
    """The checkout whose tools and backlog frame to use: START, else the one
    containing cwd, else this script's.

    The shared driver runs the contract's commands with cwd = a pristine
    checkout of main and BOSUN_WORKSPACE naming the workspace, so this is
    where the frame comes from; `workspace()` is where the database is.
    """
    for candidate in ([start] if start else []) + [pathlib.Path.cwd(), SCRIPTS]:
        result = git("rev-parse", "--show-toplevel", cwd=candidate, check=False)
        if result.returncode == 0:
            return pathlib.Path(result.stdout.strip()).resolve()
        if start:
            break
    raise QueueError(f"not inside a git repository: {start or pathlib.Path.cwd()}")


def workspace(repo: pathlib.Path) -> pathlib.Path:
    """Where bd runs, i.e. whose database the queue commands act on.

    $BRAUSER_QUEUE_WORKSPACE, else $BOSUN_WORKSPACE (the shared driver's
    workspace), else the directory holding $BEADS_DIR, else the checkout. bd
    itself also honours BEADS_DIR, which is passed through unchanged.
    """
    for name in (WORKSPACE_ENV, BOSUN_WORKSPACE_ENV):
        if os.environ.get(name):
            return pathlib.Path(os.environ[name]).expanduser().resolve()
    beads_dir = os.environ.get(BEADS_DIR_ENV)
    if beads_dir:
        path = pathlib.Path(beads_dir).expanduser().resolve()
        if path.name == ".beads":
            return path.parent
    return repo


# --------------------------------------------------------------------------
# Export, render, check


def export(repo: pathlib.Path, path: Optional[pathlib.Path] = None) -> pathlib.Path:
    """`bd export` to PATH (default target/queue/issues.jsonl), replaced atomically."""
    target = (path or repo / EXPORT_PATH).resolve()
    target.parent.mkdir(parents=True, exist_ok=True)
    handle, scratch = tempfile.mkstemp(prefix=".issues-", suffix=".jsonl", dir=target.parent)
    os.close(handle)
    try:
        bd("export", "-o", scratch, cwd=workspace(repo))
        if not any(line.strip() for line in pathlib.Path(scratch).read_text(encoding="utf-8").splitlines()):
            raise QueueError(
                f"bd export in {workspace(repo)} is empty: no queue here (run `scripts/dev/queue bootstrap`)"
            )
        os.replace(scratch, target)
    finally:
        if os.path.exists(scratch):
            os.unlink(scratch)
    return target


def render_text(repo: pathlib.Path, queue_text: str) -> str:
    rb, _ = _modules()
    frame = (repo / rb.FRAME_PATH).read_text(encoding="utf-8")
    return rb.render(rb.compose(frame, repo / rb.NOTES_DIR), rb.parse_issues(queue_text, "queue"))


def problems(repo: pathlib.Path, queue_text: str) -> list[str]:
    """The consistency check's findings for this queue and the checkout's frame."""
    rb, cqc = _modules()
    try:
        issues = rb.parse_issues(queue_text, "queue")
        backlog = render_text(repo, queue_text)
    except rb.BacklogError as error:
        return [f"[queue] {error}"]
    return [str(violation) for violation in cqc.check_generated(issues, backlog)]


def require_clean(repo: pathlib.Path, queue_text: str, what: str) -> None:
    found = problems(repo, queue_text)
    if found:
        raise QueueError(
            f"{what} fails the queue consistency check; nothing was pushed:\n  " + "\n  ".join(found)
            + "\nFix the beads with bd (or the frame under docs/backlog/), then sync again."
        )


def backlog(repo: pathlib.Path, out: Optional[pathlib.Path]) -> str:
    rb, _ = _modules()
    text = render_text(repo, export(repo).read_text(encoding="utf-8"))
    if out is not None:
        rb.write(out, text)
    return text


# --------------------------------------------------------------------------
# Sync


def dolt_remote(ws: pathlib.Path) -> Optional[str]:
    """The git URL behind bd's `origin` Dolt remote, or None."""
    out = bd("dolt", "remote", "list", cwd=ws, check=False).stdout
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] == "origin":
            return parts[1][len("git+"):] if parts[1].startswith("git+") else parts[1]
    return None


def seeded(git_url: str, cwd: pathlib.Path) -> bool:
    return bool(git("ls-remote", git_url, DOLT_REF, cwd=cwd).stdout.strip())


CONFIG_PATH = ".beads/config.yaml"  # tracked; bd bootstrap rewrites it


def bootstrap(repo: pathlib.Path) -> list[str]:
    """`bd bootstrap` in the workspace, then put back a .beads/config.yaml it
    changed, byte for byte; return what it says.

    The bytes on disk before the run are restored, so a local edit survives.
    bd bootstrap wires the Dolt remote into the database and also adds
    sync.remote to config.yaml (dropping a blank line on the way), which made
    the tracked file dirty and blocked deploy-live's clean-checkout check
    (2026-11-09). bd sync reads the remote from the database, so the
    committed file is enough.
    """
    ws = workspace(repo)
    config = ws / CONFIG_PATH
    try:
        before: Optional[bytes] = config.read_bytes()
    except FileNotFoundError:
        before = None
    said = []
    try:
        bd("bootstrap", cwd=ws)
    finally:
        # Even when bd fails after rewriting it: a dirty file blocks deploy-live.
        after = config.read_bytes() if config.exists() else None
        if after != before:
            if before is None:
                config.unlink()
            else:
                scratch = config.with_name(f".{config.name}.restore")
                scratch.write_bytes(before)
                os.replace(scratch, config)
            said.append(f"kept {CONFIG_PATH} as it was before bootstrap (bd bootstrap had rewritten it)")
    remote = dolt_remote(ws)
    if remote is None:
        raise QueueError(f"bd bootstrap left no `origin` Dolt remote in {ws}; "
                         "`bd dolt remote list` shows what it has")
    said.append(f"Dolt remote origin: {remote}")
    return said


def sync(repo: pathlib.Path) -> None:
    ws = workspace(repo)
    url = dolt_remote(ws) or git("remote", "get-url", "origin", cwd=repo).stdout.strip()
    if not seeded(url, repo):
        raise QueueError(
            f"{url} has no {DOLT_REF}: the queue was never seeded there. Seeding is a one-time "
            f"coordinated step (scripts/dev/queue seed); nothing was pushed."
        )
    require_clean(repo, export(repo).read_text(encoding="utf-8"), "this database's queue")
    bd("dolt", "pull", cwd=ws)
    require_clean(repo, export(repo).read_text(encoding="utf-8"), "the queue merged with the remote")
    result = bd("sync", cwd=ws, check=False)
    if result.returncode:
        code = result.returncode if result.returncode in SYNC_EXITS else 1
        raise QueueError(f"bd sync exited {result.returncode}: {SYNC_EXITS.get(result.returncode, 'failed')}\n"
                         f"{(result.stderr or result.stdout).strip()}", code)
    # bd sync may have pulled again; report (it already pushed) if that broke anything.
    late = problems(repo, export(repo).read_text(encoding="utf-8"))
    if late:
        raise QueueError("synced, but edits pulled during the push fail the check:\n  " + "\n  ".join(late))


# --------------------------------------------------------------------------
# Adopt a branch's committed queue edits


def _show(repo: pathlib.Path, rev: str, file: str) -> Optional[str]:
    result = git("show", f"{rev}:{file}", cwd=repo, check=False)
    return result.stdout.replace("\r\n", "\n") if result.returncode == 0 else None


# Fields bd derives on export; they say nothing about an edit.
DERIVED_FIELDS = ("dependency_count", "dependent_count", "comment_count")
# bd sets these itself; comparing them would make every adopt a conflict.
UNCOMPARED_FIELDS = ("updated_at",)


def _edge(dep: dict, issue_id: object) -> dict:
    return {"issue_id": dep.get("issue_id") or issue_id, "depends_on_id": dep.get("depends_on_id"), "type": dep.get("type")}


def normalize_record(record: dict) -> dict:
    """A row comparable across bd exports: labels a sorted set, dependencies
    sorted (endpoint, type) edges, derived counts dropped."""
    record = {k: v for k, v in record.items() if k not in DERIVED_FIELDS}
    if isinstance(record.get("labels"), list):
        record["labels"] = sorted(set(record["labels"]))
    if isinstance(record.get("dependencies"), list):
        edges = [_edge(dep, record.get("id")) for dep in record["dependencies"] if isinstance(dep, dict)]
        record["dependencies"] = sorted(edges, key=lambda e: (str(e["depends_on_id"]), str(e["type"])))
        if not record["dependencies"]:
            del record["dependencies"]
    if not record.get("labels") and "labels" in record:
        del record["labels"]
    return record


def normalized(text: str) -> str:
    rb, _ = _modules()
    return "".join(rb.dump_record(normalize_record(record)) + "\n"
                   for _key, (_line, record) in rb._records(text, "queue").items())


_MISSING = object()


def _fields(record: Optional[dict]) -> dict[str, object]:
    """Comparable fields of a row, metadata flattened to metadata.<key>."""
    if record is None:
        return {}
    out: dict[str, object] = {}
    for key, value in record.items():
        if key in UNCOMPARED_FIELDS:
            continue
        if key == "metadata" and isinstance(value, dict):
            out.update({f"metadata.{k}": v for k, v in value.items()})
        else:
            out[key] = value
    return out


def field_conflicts(base: dict, current: dict, theirs: dict) -> list[str]:
    """Every field the database and the branch both changed from BASE, to different values."""
    found = []
    for issue_id in sorted(set(current) & set(theirs)):
        b, c, t = (_fields(rows.get(issue_id)) for rows in (base, current, theirs))
        for field in sorted(set(b) | set(c) | set(t)):
            bv, cv, tv = (side.get(field, _MISSING) for side in (b, c, t))
            if cv != bv and tv != bv and cv != tv:
                show = lambda v: "(absent)" if v is _MISSING else json.dumps(v)[:60].replace('"', "'")
                found.append(f"{issue_id} {field}: database {show(cv)}, branch {show(tv)}")
    return found


def _after(stamp: Optional[str], floor: datetime.datetime) -> str:
    """An updated_at strictly after STAMP (bd import keeps a tie), and not before FLOOR."""
    rb, _ = _modules()
    parsed = rb._timestamp({"updated_at": stamp}) if stamp else None
    later = floor
    if parsed:
        existing = datetime.datetime.strptime(parsed, "%Y-%m-%dT%H:%M:%S.%f").replace(tzinfo=datetime.timezone.utc)
        later = max(floor, existing.replace(microsecond=0) + datetime.timedelta(seconds=1))
    return later.strftime("%Y-%m-%dT%H:%M:%SZ")


def plan_adoption(base: str, current: str, theirs: str) -> tuple[list[dict], dict[str, list[str]], dict[str, list[tuple[str, str]]], str]:
    """What applying THEIRS' edits (against BASE) to CURRENT takes.

    Returns (rows to import, labels to remove per id, dependencies to remove
    per id, the predicted queue). Rows merge field by field
    (render_backlog.merge_jsonl); raises QueueError on a field both sides
    changed that updated_at cannot order, or on a bead THEIRS deleted, so
    nothing is applied in either case.
    """
    rb, _ = _modules()
    base, current, theirs = (normalized(text) for text in (base, current, theirs))
    by_id = lambda text, label: {key: record for key, (_, record) in rb._records(text, label).items()}
    # merge_jsonl lets the newer updated_at win a field both sides changed;
    # adopt must not, so any such field refuses the whole adopt here.
    clashes = field_conflicts(by_id(base, "base"), by_id(current, "current"), by_id(theirs, "theirs"))
    if clashes:
        raise QueueError("the database and the branch changed the same field differently; nothing applied. "
                         + "; ".join(clashes))
    merged_text, conflicts, _notes = rb.merge_jsonl(base, current, theirs)
    if conflicts:
        raise QueueError(f"bead(s) {', '.join(conflicts)} changed differently here and on the branch; carry them by hand")
    current_rows = {key: record for key, (_, record) in rb._records(current, "current").items()}
    merged_rows = {key: record for key, (_, record) in rb._records(merged_text, "merged").items()}
    base_ids = set(rb._records(base, "base"))
    theirs_ids = set(rb._records(theirs, "theirs"))
    deleted = sorted((base_ids - theirs_ids) & set(current_rows))
    if deleted:
        raise QueueError(f"the branch deletes bead(s) {', '.join(deleted)}; delete them with bd by hand if that is right")
    lost = sorted(set(current_rows) - set(merged_rows))
    if lost:
        raise QueueError(f"adopting would drop bead(s) {', '.join(lost)}; refusing")
    now_utc = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    rows, labels, deps = [], {}, {}
    for key, record in merged_rows.items():
        now = current_rows.get(key)
        if record == now:
            continue
        row = dict(record)
        # Strictly newer than the database's row, so bd import's updated_at
        # guard applies it (a tie keeps the database's fields).
        row["updated_at"] = _after(now.get("updated_at") if now else None, now_utc)
        rows.append(row)
        if now:
            gone = sorted(set(now.get("labels") or []) - set(record.get("labels") or []))
            if gone:
                labels[key] = gone
            edges = lambda r: {(d.get("depends_on_id"), d.get("type")) for d in (r.get("dependencies") or []) if isinstance(d, dict)}
            removed = sorted(edges(now) - edges(record), key=str)
            if removed:
                deps[key] = removed
    return rows, labels, deps, merged_text


def resolve_insertions(text: str) -> Optional[str]:
    """TEXT (a diff3 merge) with every conflict resolved, or None.

    Only a hunk whose base is empty (both sides inserted text at the same
    point, as two cycles appending observations do) is resolved: ours, then
    theirs. Any other hunk, or a marker-looking line inside a side, returns
    None. scripts/pr-doctor.mjs resolveAppendOnly is the same rule for PRs.
    """
    out: list[str] = []
    lines = text.splitlines(keepends=True)
    i = 0
    while i < len(lines):
        if not lines[i].startswith("<<<<<<< "):
            out.append(lines[i])
            i += 1
            continue
        parts: dict[str, list[str]] = {"ours": [], "base": [], "theirs": []}
        part, i = "ours", i + 1
        while i < len(lines) and not lines[i].startswith(">>>>>>> "):
            line = lines[i]
            if line.startswith("<<<<<<< "):
                return None
            if line.startswith("||||||| ") or line.rstrip("\n") == "|||||||":
                if part != "ours":
                    return None
                part = "base"
            elif line.rstrip("\n") == "=======":
                if part != "base":
                    return None
                part = "theirs"
            else:
                parts[part].append(line)
            i += 1
        if i >= len(lines) or part != "theirs" or parts["base"]:
            return None
        ours, theirs = parts["ours"], parts["theirs"]
        if ours and not ours[-1].endswith("\n"):
            ours[-1] += "\n"
        out += ours + theirs
        i += 1
    return "".join(out)


def merge_prose(base: str, ours: str, theirs: str) -> Optional[str]:
    """Three-way merge of hand-written text; both sides' insertions at one point keep both."""
    with tempfile.TemporaryDirectory() as tmp:
        paths = []
        for name, text in (("ours", ours), ("base", base), ("theirs", theirs)):
            path = pathlib.Path(tmp) / name
            path.write_text(text, encoding="utf-8", newline="\n")
            paths.append(str(path))
        result = subprocess.run(
            ["git", "merge-file", "-p", "--diff3", "-L", "ours", "-L", "base", "-L", "theirs", *paths],
            capture_output=True, text=True, encoding="utf-8",
        )
    if result.returncode < 0 or result.returncode > 127:
        raise QueueError(f"git merge-file failed: {result.stderr.strip()}")
    return result.stdout if result.returncode == 0 else resolve_insertions(result.stdout)


def not_landed(rows: list[dict], exported: str) -> list[str]:
    """Each adopted field that the database does not now hold."""
    rb, _ = _modules()
    got = {key: normalize_record(record) for key, (_, record) in rb._records(exported, "database").items()}
    missing = []
    for row in rows:
        want, have = _fields(normalize_record(row)), _fields(got.get(row["id"]))
        if row["id"] not in got:
            missing.append(f"{row['id']} (absent)")
            continue
        for field, value in want.items():
            if have.get(field, _MISSING) != value:
                missing.append(f"{row['id']} {field}")
    return missing


def adopted_frame(repo: pathlib.Path, source: str, base: str, frame_path: pathlib.Path) -> Optional[str]:
    """The checkout's frame with the hand-written backlog edits SOURCE committed, or None.

    The old BOSUN_FEATURE_BACKLOG.md's text outside the generated blocks is
    what docs/backlog/frame.md now holds, so SOURCE's edits to it (against
    BASE) merge three ways into the frame. Raises QueueError on a conflict.
    """
    rb, _ = _modules()
    theirs, base_text = _show(repo, source, rb.BACKLOG_PATH), _show(repo, base, rb.BACKLOG_PATH)
    if theirs is None or base_text is None:
        return None
    try:
        their_skeleton, base_skeleton = rb.strip_generated(theirs), rb.strip_generated(base_text)
    except rb.BacklogError as error:
        raise QueueError(f"{source}:{rb.BACKLOG_PATH}: {error}") from error
    if their_skeleton == base_skeleton:
        return None
    frame = frame_path.read_text(encoding="utf-8")
    merged = merge_prose(base_skeleton, frame, their_skeleton)
    if merged is None:
        raise QueueError(f"{source} edits backlog prose that {rb.FRAME_PATH} also changed; carry it by hand")
    if merged == frame:
        return None
    try:
        rb.find_blocks(rb.split_lines(merged))
    except rb.BacklogError as error:
        raise QueueError(f"carrying {source}'s backlog prose breaks the frame's markers ({error})") from error
    return merged


def adopt(repo: pathlib.Path, source: str, base: Optional[str], *, dry_run: bool,
          frame_path: Optional[pathlib.Path] = None) -> int:
    """Apply SOURCE's committed queue edits through bd, then sync. Returns beads changed."""
    if base is None:
        found = git("merge-base", source, "origin/main", cwd=repo, check=False)
        if found.returncode != 0:
            raise QueueError(f"{source} has no merge base with origin/main; pass --base")
        base = found.stdout.strip()
    rb, _ = _modules()
    frame_path = frame_path or repo / rb.FRAME_PATH
    frame = adopted_frame(repo, source, base, frame_path)
    theirs, base_text = _show(repo, source, QUEUE_PATH), _show(repo, base, QUEUE_PATH)
    if (theirs is None or theirs == base_text) and frame is None:
        print(f"queue: {source} has no queue or backlog edits against {base[:12]}; nothing to adopt")
        return 0
    ws = workspace(repo)
    if not dry_run:
        bd("dolt", "pull", cwd=ws)
    current = export(repo).read_text(encoding="utf-8")
    if theirs is None or theirs == base_text:
        rows, labels, deps, predicted = [], {}, {}, current
    else:
        rows, labels, deps, predicted = plan_adoption(base_text or "", current, theirs)
    # Check the result with the new frame before changing anything.
    with tempfile.TemporaryDirectory() as tmp:
        view = pathlib.Path(tmp)
        rb.write(view / rb.FRAME_PATH, frame if frame is not None else frame_path.read_text(encoding="utf-8"))
        notes = repo / rb.NOTES_DIR
        if notes.is_dir():
            shutil.copytree(notes, view / rb.NOTES_DIR)
        require_clean(view, predicted, f"the queue with {source}'s edits")
    prose = f"; and {frame_path.name} gains its backlog prose" if frame is not None else ""
    if dry_run:
        print(f"queue: {source} would change {len(rows)} bead(s): {', '.join(r['id'] for r in rows) or 'none'}{prose} (dry run)")
        return len(rows)
    if rows:
        rb, _ = _modules()
        # Removals first: `bd dep remove` deletes by endpoints, so after the
        # import it would also delete an edge whose type the branch changed.
        for key, edges in deps.items():
            for target, _kind in edges:
                bd("dep", "remove", key, str(target), cwd=ws)
        for key, names in labels.items():
            for name in names:
                bd("label", "remove", key, name, cwd=ws)
        with tempfile.TemporaryDirectory() as tmp:
            path = pathlib.Path(tmp) / "adopt.jsonl"
            path.write_text("".join(rb.dump_record(row) + "\n" for row in rows), encoding="utf-8")
            bd("import", str(path), cwd=ws)
        missing = not_landed(rows, export(repo).read_text(encoding="utf-8"))
        if missing:
            raise QueueError("bd import reported success but these edits did not land (nothing synced): "
                             + "; ".join(missing))
    if frame is not None:
        rb.write(frame_path, frame)
    sync(repo)
    print(f"queue: adopted {len(rows)} bead edit(s) from {source} and synced{prose} (commit it in the branch)")
    return len(rows)


# --------------------------------------------------------------------------
# Seed a remote once


def validated_rows(path: pathlib.Path) -> dict[str, str]:
    rb, _ = _modules()
    text = path.read_text(encoding="utf-8")
    try:
        rb.parse_issues(text, str(path))
    except rb.BacklogError as error:
        raise QueueError(str(error)) from error
    rows = {}
    for key, (line, record) in rb._records(text, str(path)).items():
        if record.get("_type", "issue") != "issue":
            raise QueueError(f"{path}: {key} is a {record.get('_type')!r} record, not an issue")
        if not key.startswith(f"{PREFIX}-"):
            raise QueueError(f"{path}: {key} does not have the {PREFIX}- prefix")
        rows[key] = line
    return rows


def combine(main: pathlib.Path, extras: list[pathlib.Path]) -> str:
    """MAIN's rows plus each extra file's, by id; a duplicate id must be the same record."""
    rb, _ = _modules()
    rows = validated_rows(main)
    for extra in extras:
        for key, line in validated_rows(extra).items():
            if key in rows and json.loads(rows[key]) != json.loads(line):
                raise QueueError(f"{extra}: {key} is already in {main} with different fields")
            rows.setdefault(key, line)
    ordered = sorted(rows.values(), key=lambda line: rb.export_order(json.loads(line)))
    return "".join(line + "\n" for line in ordered)


def seed(repo: pathlib.Path, remote: str, main: pathlib.Path, extras: list[pathlib.Path], *, yes: bool) -> str:
    """Build a fresh database from the files and push it to REMOTE. Returns the pushed refs/dolt/data."""
    queue = combine(main, extras)
    require_clean(repo, queue, "the seed queue")
    if seeded(remote, repo):
        raise QueueError(f"{remote} already has {DOLT_REF}; seeding is one-shot. Sync instead.")
    count = len([line for line in queue.splitlines() if line.strip()])
    if not yes:
        raise QueueError(f"would seed {remote} with {count} bead(s); rerun with --yes to push")
    work = pathlib.Path(tempfile.mkdtemp(prefix="brauser-queue-seed-"))
    try:
        git("init", "-q", "-b", "main", str(work), cwd=repo)
        git("remote", "add", "origin", remote, cwd=work)
        # bd init keeps a metadata.json that is already there, and gives the
        # database that project_id; without it every clone that carries the
        # committed file fails bd's workspace identity check (brwr-765q).
        committed = repo / ".beads" / "metadata.json"
        (work / ".beads").mkdir()
        if committed.is_file():
            shutil.copyfile(committed, work / ".beads" / "metadata.json")
        bd("init", "--prefix", PREFIX, "--non-interactive", "--skip-hooks", "--skip-agents", cwd=work)
        project_id = json.loads((work / ".beads" / "metadata.json").read_text(encoding="utf-8")).get("project_id")
        if committed.is_file() and project_id != json.loads(committed.read_text(encoding="utf-8")).get("project_id"):
            raise QueueError(f"bd init did not keep the committed project_id from {committed}; nothing pushed")
        if dolt_remote(work) is None:
            bd("dolt", "remote", "add", "origin", f"git+{remote}", cwd=work)
        source = work / "seed.jsonl"
        source.write_text(queue, encoding="utf-8")
        bd("import", str(source), cwd=work)
        got = export(work, work / "check.jsonl").read_text(encoding="utf-8")
        rb, _ = _modules()
        want_ids, got_ids = set(rb._records(queue, "seed")), set(rb._records(got, "database"))
        if want_ids != got_ids:
            missing, extra = sorted(want_ids - got_ids), sorted(got_ids - want_ids)
            raise QueueError(f"the database does not match the seed (missing {missing}, extra {extra}); nothing pushed")
        require_clean(repo, got, "the imported seed")
        bd("dolt", "push", cwd=work)
        pushed = git("ls-remote", remote, DOLT_REF, cwd=repo).stdout.split()
        if not pushed:
            raise QueueError(f"bd dolt push reported success but {remote} has no {DOLT_REF}")
        print(f"queue: seeded {remote} with {count} bead(s) at {DOLT_REF} {pushed[0][:12]}")
        if committed.is_file():
            print(f"queue: the database keeps the committed project_id {project_id}")
        else:
            print(f"queue: {committed} is missing: commit .beads/metadata.json with project_id {project_id} "
                  f"(the seeded database's), or clones fail bd's identity check")
        return pushed[0]
    finally:
        shutil.rmtree(work, ignore_errors=True)


# --------------------------------------------------------------------------
# CLI


def main(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="scripts/dev/queue", description=__doc__.splitlines()[0])
    parser.add_argument("--repo", type=pathlib.Path, help="act for this checkout (default: this one)")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("bootstrap", help="bd bootstrap, keeping .beads/config.yaml as committed")
    sub.add_parser("sync", help="check, pull, check, bd sync")
    export_parser = sub.add_parser("export", help="print the queue JSONL on stdout (and write PATH)")
    export_parser.add_argument("path", nargs="?", type=pathlib.Path)
    backlog_parser = sub.add_parser("backlog", help="render BOSUN_FEATURE_BACKLOG.md")
    where = backlog_parser.add_mutually_exclusive_group()
    where.add_argument("--print", action="store_true")
    where.add_argument("--out", type=pathlib.Path)
    sub.add_parser("check", help="run the queue consistency check on this database")
    adopt_parser = sub.add_parser("adopt", help="apply a branch's committed queue edits through bd")
    adopt_parser.add_argument("--from", dest="source", required=True)
    adopt_parser.add_argument("--base")
    adopt_parser.add_argument("--dry-run", action="store_true")
    adopt_parser.add_argument("--frame", type=pathlib.Path, help="the frame to carry backlog prose into (default: the checkout's)")
    seed_parser = sub.add_parser("seed", help="one-shot: seed an empty Dolt remote")
    seed_parser.add_argument("--remote", required=True, help="git URL, e.g. https://github.com/rausified/rauser-browser.git")
    seed_parser.add_argument("--from", dest="source", type=pathlib.Path, required=True)
    seed_parser.add_argument("--extra", type=pathlib.Path, action="append", default=[])
    seed_parser.add_argument("--yes", action="store_true", help="really push")
    args = parser.parse_args(argv)

    try:
        repo = repo_root(args.repo.resolve() if args.repo else None)
        if args.command == "bootstrap":
            for line in bootstrap(repo):
                print(f"queue: {line}")
            print("queue: bootstrapped")
        elif args.command == "sync":
            sync(repo)
            print("queue: synced")
        elif args.command == "export":
            path = export(repo, args.path.resolve() if args.path else None)
            sys.stdout.write(path.read_text(encoding="utf-8"))
        elif args.command == "backlog":
            rb, _ = _modules()
            out = None if args.print else (args.out or repo / rb.RENDER_PATH)
            text = backlog(repo, out)
            print(text, end="") if args.print else print(out)
        elif args.command == "check":
            found = problems(repo, export(repo).read_text(encoding="utf-8"))
            for line in found:
                print(f"  {line}")
            print("queue consistency check " + ("FAILED" if found else "passed"))
            return 1 if found else 0
        elif args.command == "adopt":
            adopt(repo, args.source, args.base, dry_run=args.dry_run,
                  frame_path=args.frame.resolve() if args.frame else None)
        elif args.command == "seed":
            seed(repo, args.remote, args.source.resolve(), [p.resolve() for p in args.extra], yes=args.yes)
        return 0
    except (QueueError, OSError, subprocess.SubprocessError) as error:
        # One line: the driver reads the first stderr line as the reason.
        reason = "; ".join(line.strip() for line in str(error).splitlines() if line.strip())
        print(f"queue {args.command}: {reason or type(error).__name__}", file=sys.stderr)
        return getattr(error, "code", 1)


if __name__ == "__main__":
    sys.exit(main())
