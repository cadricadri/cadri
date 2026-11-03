"""Claude Code sessions under ~/.claude/projects/*/…/*.jsonl."""

from __future__ import annotations

from pathlib import Path

from .types import SessionRef


def list_sessions(home: Path, limit: int | None = None) -> list[SessionRef]:
    root = home / ".claude" / "projects"
    if not root.is_dir():
        return []
    refs: list[SessionRef] = []
    for path in root.rglob("*.jsonl"):
        if not path.is_file():
            continue
        # Skip side files that are not session transcripts when possible.
        name = path.name
        if name.startswith("."):
            continue
        thread_id = path.stem
        try:
            updated = path.stat().st_mtime
        except OSError:
            continue
        refs.append(
            SessionRef(
                harness="claude_code",
                thread_id=thread_id,
                path=path,
                updated_at=updated,
            )
        )
    refs.sort(key=lambda item: item.updated_at, reverse=True)
    if limit is not None:
        return refs[:limit]
    return refs
