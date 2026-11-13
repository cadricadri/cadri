"""Discover local coding-agent session stores."""

from __future__ import annotations

from pathlib import Path

from . import claude, codex, cursor, opencode
from .types import SessionRef

__all__ = ["SessionRef", "discover_all", "detected_harnesses"]

DISCOVERERS = {
    "claude_code": claude.list_sessions,
    "cursor": cursor.list_sessions,
    "codex": codex.list_sessions,
    "opencode": opencode.list_sessions,
}


def discover_all(home: Path | None = None) -> list[SessionRef]:
    root = home or Path.home()
    found: list[SessionRef] = []
    for list_sessions in DISCOVERERS.values():
        try:
            found.extend(list_sessions(root))
        except OSError:
            continue
    found.sort(key=lambda item: item.updated_at, reverse=True)
    return found


def detected_harnesses(home: Path | None = None) -> list[str]:
    root = home or Path.home()
    names: list[str] = []
    for name, list_sessions in DISCOVERERS.items():
        try:
            if list_sessions(root, limit=1):
                names.append(name)
        except OSError:
            continue
    return names
