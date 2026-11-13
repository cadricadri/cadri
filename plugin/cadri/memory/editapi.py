"""Writes through the server's edit API (`cadri memory write`).

Steady-state agents, including the install seed, edit files in the clone and run
`cadri memory push` instead (see push.py).
"""

from __future__ import annotations

import os
import secrets
import sys

from . import sync
from .books import Book, MemoryFailure, ensure_clone, http_json, local_head, session_id_from_env
from .layout import TEAM_LAYOUT

EXIT_CONFLICT = 3


def default_agent_id() -> str:
    explicit = os.environ.get("CADRI_AGENT_ID")
    if explicit:
        return explicit
    harness = os.environ.get("CADRI_HARNESS") or "agent"
    return f"{harness}-{secrets.token_hex(2)}"


def commit_files(
    book: Book,
    files: tuple[tuple[str, str], ...] | list[tuple[str, str]],
    message: str,
    agent: str | None,
    session: str | None,
    quiet: bool = False,
) -> int:
    ensure_clone(book)
    head = local_head(book)
    body: dict = {
        "message": message,
        "changes": [{"op": "write", "path": path, "content": content} for path, content in files],
        "agent": agent or default_agent_id(),
    }
    if head:
        body["base"] = head
    if session:
        body["session_id"] = session
    status, _, reply = http_json(
        "POST", f"{book.base}/api/memories/commits?view=tree{book.query.replace('?', '&')}", book.base, body
    )
    if status == 409:
        paths = reply.get("paths")
        if isinstance(paths, list) and paths:
            print("cadri: conflict; these paths changed since your clone:", file=sys.stderr)
            for item in paths:
                print(f"  {item}", file=sys.stderr)
            print("run cadri memory sync, re-read them, and write again", file=sys.stderr)
            return EXIT_CONFLICT
        raise MemoryFailure(f"write rejected: {reply.get('error', 'conflict')}")
    if status in (401, 403):
        raise MemoryFailure("not signed in; run cadri login or set CADRI_API_TOKEN")
    if status != 200:
        raise MemoryFailure(f"write failed ({status}): {reply.get('error', 'no error message')}")
    sync_status = sync.pull(book, quiet=True)
    commit = reply.get("commit") or reply.get("tip") or local_head(book)
    if commit and not quiet:
        print(commit)
    return sync_status


def write(book: Book, path: str, content: str, message: str, agent: str | None, session: str | None = None) -> int:
    return commit_files(book, [(path, content)], message, agent, session or session_id_from_env())


def scaffold_team(book: Book) -> bool:
    """Publish the team directory indexes when the book has no commits. Returns whether it wrote.

    Init does not call this. An index-only commit has no lessons, and the memories
    page keeps showing the seeding instructions until a topic file exists. The seed
    writes indexes in the same push as the first lessons.
    """
    if book.scope != "team" or local_head(book):
        return False
    status = commit_files(book, TEAM_LAYOUT, "Seed team memory layout", "cadri-init", session=None, quiet=True)
    if status != 0:
        raise MemoryFailure("seeded the team layout but could not sync the clone")
    return True
