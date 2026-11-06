"""OpenCode sessions from the local SQLite store (macOS Application Support)."""

from __future__ import annotations

import json
import sqlite3
from pathlib import Path

from .types import SessionRef


def _db_candidates(home: Path) -> list[Path]:
    return [
        home / "Library" / "Application Support" / "opencode" / "opencode.db",
        home / ".local" / "share" / "opencode" / "opencode.db",
        home / ".local" / "share" / "opencode" / "opencode-local.db",
    ]


def database(home: Path) -> Path | None:
    return next((path for path in _db_candidates(home) if path.is_file()), None)


def list_sessions(home: Path, limit: int | None = None) -> list[SessionRef]:
    refs: list[SessionRef] = []
    for db_path in _db_candidates(home):
        if not db_path.is_file():
            continue
        try:
            connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        except sqlite3.Error:
            continue
        try:
            # Prefer v2 tables when present; fall back to legacy session listing.
            rows = connection.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            ).fetchall()
            tables = {row[0] for row in rows}
            if "session_v2" in tables:
                query = "SELECT id, time_updated FROM session_v2 ORDER BY time_updated DESC"
            elif "session" in tables:
                query = "SELECT id, time_updated FROM session ORDER BY time_updated DESC"
            else:
                continue
            if limit is not None:
                query += f" LIMIT {int(limit)}"
            for session_id, updated in connection.execute(query):
                if not isinstance(session_id, str) or not session_id:
                    continue
                updated_at = float(updated or 0) / (
                    1000.0 if isinstance(updated, (int, float)) and updated > 10_000_000_000 else 1.0
                )
                refs.append(
                    SessionRef(
                        harness="opencode",
                        thread_id=session_id,
                        path=db_path,
                        updated_at=updated_at,
                    )
                )
        except sqlite3.Error:
            continue
        finally:
            connection.close()
        if refs:
            break
    refs.sort(key=lambda item: item.updated_at, reverse=True)
    if limit is not None:
        return refs[:limit]
    return refs


def _decode(raw: object) -> dict:
    if not isinstance(raw, str):
        return {}
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        return {"raw": raw}
    return value if isinstance(value, dict) else {"raw": value}


def export_session_json(db_path: Path, session_id: str) -> str:
    """Export one OpenCode session as JSON for upload (info + messages).

    OpenCode 2 keeps turns in session_message; OpenCode 1 keeps them in message and
    part, and may also carry an empty session_message table.
    """
    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        tables = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            )
        }
        info: dict = {"id": session_id}
        for table in ("session_v2", "session"):
            if table in tables:
                row = connection.execute(f"SELECT * FROM {table} WHERE id = ?", (session_id,)).fetchone()
                if row:
                    info = dict(row)
                    break
        messages: list = []
        if "session_message" in tables:
            messages = [
                _decode(row["data"])
                for row in connection.execute(
                    "SELECT data FROM session_message WHERE session_id = ? ORDER BY seq, id",
                    (session_id,),
                )
            ]
        if not messages and {"message", "part"} <= tables:
            parts: dict[str, list] = {}
            for row in connection.execute(
                "SELECT message_id, data FROM part WHERE session_id = ? ORDER BY time_created, id",
                (session_id,),
            ):
                parts.setdefault(row["message_id"], []).append(_decode(row["data"]))
            for row in connection.execute(
                "SELECT id, data FROM message WHERE session_id = ? ORDER BY time_created, id",
                (session_id,),
            ):
                messages.append({"id": row["id"], **_decode(row["data"]), "parts": parts.get(row["id"], [])})
        return json.dumps({"info": info, "messages": messages})
    finally:
        connection.close()
