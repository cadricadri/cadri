"""Which session last edited which memory file, recorded by the edit hooks.

Push uses this to commit only the caller's files and to attribute each commit to the
session that made the change. Edits made where no hook sees them (shell commands)
have no record; push treats them as the caller's once they have been quiet a while.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path

from ..state import append_jsonl, read_jsonl
from .books import Book

KEEP_SECONDS = 7 * 24 * 60 * 60


@dataclass(frozen=True)
class Edit:
    path: str
    session: str | None
    agent: str
    harness: str | None
    at: float


def record_path(book: Book) -> Path:
    return book.sibling(".edits.jsonl")


def record(book: Book, relative: str, session: str | None, agent: str | None, harness: str | None) -> None:
    append_jsonl(
        record_path(book),
        {"path": relative, "session": session, "agent": agent or "main", "harness": harness, "at": time.time()},
    )


def latest(book: Book) -> dict[str, Edit]:
    """The most recent edit of each path."""
    found: dict[str, Edit] = {}
    cutoff = time.time() - KEEP_SECONDS
    for row in read_jsonl(record_path(book)):
        path, at = row.get("path"), row.get("at")
        if not isinstance(path, str) or not isinstance(at, (int, float)) or at < cutoff:
            continue
        found[path] = Edit(path, row.get("session"), row.get("agent") or "main", row.get("harness"), float(at))
    return found


def by_session(book: Book, session: str) -> list[Edit]:
    return [edit for edit in latest(book).values() if edit.session == session]
