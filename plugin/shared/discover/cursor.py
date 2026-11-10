"""Cursor agent sessions under ~/.cursor/projects/*/agent-transcripts/."""

from __future__ import annotations

from pathlib import Path

from .types import SessionRef


def list_sessions(home: Path, limit: int | None = None) -> list[SessionRef]:
    root = home / ".cursor" / "projects"
    if not root.is_dir():
        return []
    refs: list[SessionRef] = []
    for project in root.iterdir():
        if not project.is_dir():
            continue
        transcripts = project / "agent-transcripts"
        if not transcripts.is_dir():
            continue
        for path in transcripts.rglob("*.jsonl"):
            if not path.is_file():
                continue
            try:
                updated = path.stat().st_mtime
            except OSError:
                continue
            refs.append(
                SessionRef(
                    harness="cursor",
                    thread_id=path.stem,
                    path=path,
                    updated_at=updated,
                )
            )
    refs.sort(key=lambda item: item.updated_at, reverse=True)
    if limit is not None:
        return refs[:limit]
    return refs
