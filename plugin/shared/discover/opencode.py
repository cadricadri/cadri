"""OpenCode sessions from the local SQLite store (macOS Application Support)."""

from __future__ import annotations

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


def export_session_json(db_path: Path, session_id: str) -> str:
    """Export one OpenCode session as JSON for upload (info + messages)."""
    import json

    connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    try:
        tables = {
            row[0]
            for row in connection.execute(
                "SELECT name FROM sqlite_master WHERE type='table'"
            )
        }
        info: dict = {"id": session_id}
        messages: list = []
        if "session_v2" in tables:
            row = connection.execute(
                "SELECT * FROM session_v2 WHERE id = ?", (session_id,)
            ).fetchone()
            if row:
                cols = [d[0] for d in connection.execute("SELECT * FROM session_v2 LIMIT 0").description]
                info = dict(zip(cols, row))
        if "session_message" in tables:
            for row in connection.execute(
                "SELECT data FROM session_message WHERE session_id = ? ORDER BY id",
                (session_id,),
            ):
                raw = row[0]
                if isinstance(raw, str):
                    try:
                        messages.append(json.loads(raw))
                    except json.JSONDecodeError:
                        messages.append({"raw": raw})
        elif "message" in tables:
            for row in connection.execute(
                "SELECT data FROM message WHERE sessionID = ? ORDER BY id",
                (session_id,),
            ):
                raw = row[0]
                if isinstance(raw, str):
                    try:
                        messages.append(json.loads(raw))
                    except json.JSONDecodeError:
                        messages.append({"raw": raw})
        return json.dumps({"info": info, "messages": messages})
    finally:
        connection.close()
