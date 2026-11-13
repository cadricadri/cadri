"""Session capture: turn a hook payload into a session row and upload it.

The hooks call this from a detached process after answering the harness, so a slow
network never stalls the agent.
"""

from __future__ import annotations

import sys
import time
from pathlib import Path

from . import auth
from .discover import cursor, opencode
from .upload import post_session

HARNESSES = ("claude_code", "cursor", "codex", "opencode")
ALIASES = {"claude": "claude_code"}
UPLOAD_TIMEOUT_SECONDS = 30


def text(event: dict, *keys: str) -> str | None:
    for key in keys:
        value = event.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def read_file(path: str | None) -> str:
    if not path:
        return ""
    try:
        return Path(path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def cursor_transcript(conversation_id: str) -> str | None:
    root = Path.home() / ".cursor" / "projects"
    if not root.is_dir():
        return None
    for project in root.iterdir():
        folder = project / "agent-transcripts"
        for candidate in (folder / conversation_id / f"{conversation_id}.jsonl", folder / f"{conversation_id}.jsonl"):
            if candidate.is_file():
                return str(candidate)
    return None


def build_payload(harness: str, event: dict) -> dict | None:
    """The session row for this event, or None when there is nothing to upload yet."""
    harness = ALIASES.get(harness, harness)
    hook_event = text(event, "hook_event_name")
    if harness == "cursor":
        thread_id = text(event, "conversation_id", "session_id")
        if thread_id is None:
            return None
        path = text(event, "transcript_path") or cursor_transcript(thread_id)
        content = read_file(path)
    elif harness == "opencode":
        thread_id = text(event, "session_id")
        if thread_id is None:
            return None
        db = opencode.database(Path.home())
        content = opencode.export_session_json(db, thread_id) if db else ""
    else:
        thread_id = text(event, "session_id")
        if thread_id is None:
            return None
        content = read_file(text(event, "transcript_path"))
        last = text(event, "last_assistant_message")
        if last and last not in content:
            content = f"{content}\n{last}" if content else last
    if not content.strip():
        return None
    payload = {
        "thread_id": thread_id,
        "content": content,
        "hook_event": hook_event,
        "harness": harness,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    model = text(event, "model")
    if model is None and harness == "cursor":
        # Stop hooks usually carry `model`. Backfill and older payloads do not;
        # the transcript file never does either.
        model = cursor.model_name(Path.home(), thread_id)
    if model:
        payload["model"] = model
    if harness == "claude_code":
        payload["session_id"] = thread_id
    return payload


def upload(payload: dict) -> None:
    base = auth.server_url()
    try:
        status, body = post_session(base, payload, UPLOAD_TIMEOUT_SECONDS)
    except (OSError, ValueError) as exc:
        print(f"cadri: could not reach {base}: {exc}", file=sys.stderr)
        return
    if status in (200, 201):
        return
    detail = ""
    if status == 403 and body.get("error") == "no_organization":
        detail = "; open Cadri in the browser, create your team, then run cadri login"
    elif status in (401, 403):
        detail = "; run cadri login or set CADRI_API_TOKEN"
    print(f"cadri: {base}/api/sessions returned {status}{detail}", file=sys.stderr)


def capture(harness: str, event: dict) -> None:
    payload = build_payload(harness, event)
    if payload is not None:
        upload(payload)
