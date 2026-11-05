"""Codex sessions under ~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<id>.jsonl."""

from __future__ import annotations

import re
from pathlib import Path

from .types import SessionRef

SESSION_ID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$", re.IGNORECASE)


def thread_id(path: Path) -> str:
    """The session id Codex hooks report, which is the UUID that ends the rollout file name."""
    match = SESSION_ID.search(path.stem)
    return match.group(0) if match else path.stem


def list_sessions(home: Path, limit: int | None = None) -> list[SessionRef]:
    root = home / ".codex" / "sessions"
    if not root.is_dir():
        return []
    refs: list[SessionRef] = []
    for path in root.rglob("*.jsonl"):
        if not path.is_file():
            continue
        try:
            updated = path.stat().st_mtime
        except OSError:
            continue
        refs.append(
            SessionRef(
                harness="codex",
                thread_id=thread_id(path),
                path=path,
                updated_at=updated,
            )
        )
    refs.sort(key=lambda item: item.updated_at, reverse=True)
    if limit is not None:
        return refs[:limit]
    return refs
