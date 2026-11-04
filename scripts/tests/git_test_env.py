"""Quiet, removable temporary git repositories for the script tests.

git runs background maintenance after commits (`git maintenance run --auto
--detach`, or a detached `gc --auto`). In the throwaway repositories these
tests build, that background process can create and delete
`.git/objects/maintenance.lock` while a test is already removing the
directory, which made cleanup fail intermittently (macOS CI on PR #267,
2026-11-03). `env()` turns that maintenance off through git's
GIT_CONFIG_COUNT/KEY/VALUE environment, which reaches every child git
process, including the ones bd and Dolt spawn; `rmtree()` also treats a path
that vanished during removal as removed.
"""

from __future__ import annotations

import os
import pathlib
import shutil
import stat
import sys
from typing import Iterable, Mapping, Optional

QUIET_CONFIG = (
    ("maintenance.auto", "false"),
    ("gc.auto", "0"),
    ("gc.autoDetach", "false"),
)


def env(extra_config: Iterable[tuple[str, str]] = (),
        base: Optional[Mapping[str, str]] = None, **variables: str) -> dict[str, str]:
    """BASE (default os.environ) plus VARIABLES, with the quiet settings and
    EXTRA_CONFIG appended to any GIT_CONFIG_* entries BASE already has."""
    out = dict(os.environ if base is None else base)
    out.update(variables)
    count = int(out.get("GIT_CONFIG_COUNT", "0") or "0")
    for key, value in (*QUIET_CONFIG, *extra_config):
        out[f"GIT_CONFIG_KEY_{count}"] = key
        out[f"GIT_CONFIG_VALUE_{count}"] = value
        count += 1
    out["GIT_CONFIG_COUNT"] = str(count)
    return out


def quiet_variables() -> dict[str, str]:
    """Only the GIT_CONFIG_* variables env() adds, for os.environ patches."""
    return {k: v for k, v in env(base={}).items() if k.startswith("GIT_CONFIG_")}


def make_writable_and_retry(func, path, _exc) -> None:
    """shutil.rmtree error handler: git marks objects read-only, which Windows
    refuses to unlink; a path that is already gone counts as removed."""
    try:
        os.chmod(path, stat.S_IWRITE)
        func(path)
    except FileNotFoundError:
        pass


def rmtree(path: pathlib.Path) -> None:
    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=make_writable_and_retry)
    else:
        shutil.rmtree(path, onerror=make_writable_and_retry)
