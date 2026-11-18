"""A one-tool MCP server so the agent can search memory where the user can see the call.

The prompt hook decides whether anything matched. When it did, it asks the agent to
call `cadri_search`. This process is what the harness draws in the activity log.
"""

from __future__ import annotations

import json
import sys
import time

from . import auth
from . import telemetry
from .memory import books as bk
from .memory import context, recall
from .state import Session

PROTOCOL = "2024-11-05"
TOOL = "cadri_search"


def search_text(session_id: str | None, harness: str | None, prompt: str | None, url: str | None = None) -> str:
    """Matching lines for this prompt, or "" when there is nothing to show."""
    text = prompt or ""
    if not text and session_id:
        text = str(Session(harness or "cursor", session_id).load().get("last_prompt") or "")
    if not text:
        return ""
    books = [book for book in bk.local_books(url or auth.server_url()) if book.path.is_dir()]
    body = recall.render(books, text)
    if not body:
        return ""
    reminder = context.reminder()
    return body + (f"\n\n{reminder}" if reminder else "")


def handle(message: dict) -> dict | None:
    method = message.get("method")
    if method == "notifications/initialized" or message.get("id") is None and method and method.startswith("notifications/"):
        return None
    identity = message.get("id")
    if method == "initialize":
        return _result(identity, {
            "protocolVersion": PROTOCOL,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "cadri", "version": "0.2.0"},
        })
    if method == "ping":
        return _result(identity, {})
    if method == "tools/list":
        return _result(identity, {"tools": [_tool()]})
    if method == "tools/call":
        params = message.get("params") or {}
        if params.get("name") != TOOL:
            return _error(identity, f"unknown tool {params.get('name')}")
        arguments = params.get("arguments") or {}
        started = time.perf_counter()
        try:
            text = search_text(
                _str(arguments.get("session")),
                _str(arguments.get("harness")),
                _str(arguments.get("prompt")),
            )
        finally:
            telemetry.report("mcp_cadri_search", time.perf_counter() - started)
        return _result(identity, {"content": [{"type": "text", "text": text}], "isError": False})
    if identity is None:
        return None
    return _error(identity, f"unknown method {method}")


def serve() -> None:
    while True:
        message = read_message()
        if message is None:
            return
        response = handle(message)
        if response is not None:
            write_message(response)


def _tool() -> dict:
    return {
        "name": TOOL,
        "description": (
            "Search Cadri memory for the current prompt and return the matching lines. "
            "Call this before other work when the prompt says Cadri remembered something, "
            "so the user sees the search. Pass the session and harness from that prompt."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "session": {"type": "string", "description": "The agent session id from the prompt."},
                "harness": {"type": "string", "description": "The harness name from the prompt, such as cursor or claude."},
                "prompt": {"type": "string", "description": "Search this text instead of the session's latest prompt."},
            },
        },
    }


def _str(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _result(identity: object, result: dict) -> dict:
    return {"jsonrpc": "2.0", "id": identity, "result": result}


def _error(identity: object, message: str) -> dict:
    return {"jsonrpc": "2.0", "id": identity, "error": {"code": -32601, "message": message}}


def read_message() -> dict | None:
    line = sys.stdin.buffer.readline()
    if not line:
        return None
    if line.lower().startswith(b"content-length:"):
        length = int(line.split(b":", 1)[1].strip())
        while True:
            header = sys.stdin.buffer.readline()
            if header in (b"\r\n", b"\n", b""):
                break
        return json.loads(sys.stdin.buffer.read(length))
    stripped = line.strip()
    if stripped.startswith(b"{"):
        return json.loads(stripped)
    return None


def write_message(payload: dict) -> None:
    body = json.dumps(payload).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()
