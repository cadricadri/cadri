"""Cursor agent sessions under ~/.cursor/projects/*/agent-transcripts/."""

from __future__ import annotations

import json
import os
import sqlite3
from pathlib import Path
from urllib.parse import quote

from .types import SessionRef

# Agent transcripts do not record the model. Cursor stores the chat's current
# selection in globalStorage as composerData:<id>.modelConfig.
_STATE_DBS: dict[Path, sqlite3.Connection] = {}


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


def state_db(home: Path) -> Path | None:
    """Cursor's global state database, which holds composerData for each chat."""
    candidates = [
        home / "Library/Application Support/Cursor/User/globalStorage/state.vscdb",
        home / ".config/Cursor/User/globalStorage/state.vscdb",
    ]
    xdg = os.environ.get("XDG_CONFIG_HOME")
    if xdg:
        candidates.append(Path(xdg) / "Cursor/User/globalStorage/state.vscdb")
    appdata = os.environ.get("APPDATA")
    if appdata:
        candidates.append(Path(appdata) / "Cursor/User/globalStorage/state.vscdb")
    for path in candidates:
        if path.is_file():
            return path
    return None


def model_name(home: Path, thread_id: str) -> str | None:
    """Model selected for this Cursor chat, or None when Cursor has no record of it."""
    database = _database(home)
    if database is None or not thread_id:
        return None
    try:
        row = database.execute(
            "SELECT value FROM cursorDiskKV WHERE key = ?",
            (f"composerData:{thread_id}",),
        ).fetchone()
    except sqlite3.Error:
        return None
    if row is None:
        return None
    return _model_from_composer(row[0])


def _database(home: Path) -> sqlite3.Connection | None:
    path = state_db(home)
    if path is None:
        return None
    cached = _STATE_DBS.get(path)
    if cached is not None:
        return cached
    uri = f"file:{quote(path.as_posix(), safe='/')}?mode=ro"
    try:
        connection = sqlite3.connect(uri, uri=True, timeout=1)
        connection.execute("PRAGMA query_only = ON")
    except sqlite3.Error:
        return None
    _STATE_DBS[path] = connection
    return connection


def _model_from_composer(value: object) -> str | None:
    if isinstance(value, str):
        text = value
    elif isinstance(value, (bytes, bytearray, memoryview)):
        text = bytes(value).decode("utf-8", "replace")
    else:
        return None
    try:
        data = json.loads(text)
    except json.JSONDecodeError:
        return None
    config = data.get("modelConfig") if isinstance(data, dict) else None
    if not isinstance(config, dict):
        return None
    name = _clean(config.get("modelName"))
    if name:
        return name
    selected = config.get("selectedModels")
    if isinstance(selected, list):
        for item in selected:
            if isinstance(item, dict):
                model_id = _clean(item.get("modelId"))
                if model_id:
                    return model_id
    return None


def _clean(value: object) -> str | None:
    if not isinstance(value, str):
        return None
    text = value.strip()
    return text or None

