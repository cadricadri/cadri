#!/usr/bin/env python3
"""Read a Claude Code hook event and POST the full session transcript."""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))

from shared import cadri_auth  # noqa: E402

REFRESH_TIMEOUT_SECONDS = 5
UPLOAD_TIMEOUT_SECONDS = 14


def read_transcript(path: object) -> str:
    if not isinstance(path, str) or not path:
        return ""
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            return handle.read()
    except OSError as exc:
        print(f"cadri: could not read transcript: {exc}", file=sys.stderr)
        return ""


def with_trailing_message(content: str, event: dict) -> str:
    last = event.get("last_assistant_message")
    if not isinstance(last, str) or not last.strip():
        return content
    if last in content:
        return content
    return f"{content}\n{last}" if content else last


def read_event():
    try:
        event = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        print(f"cadri: invalid hook input: {exc}", file=sys.stderr)
        return None
    if not isinstance(event, dict):
        print("cadri: hook input must be a JSON object", file=sys.stderr)
        return None
    session_id = event.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        print("cadri: hook input missing session_id", file=sys.stderr)
        return None
    return event


def build_payload(event: dict) -> dict:
    content = with_trailing_message(read_transcript(event.get("transcript_path")), event)
    cwd = event.get("cwd")
    hook_event = event.get("hook_event_name")
    return {
        "thread_id": event["session_id"],
        "session_id": event["session_id"],
        "content": content,
        "cwd": cwd if isinstance(cwd, str) else None,
        "hook_event": hook_event if isinstance(hook_event, str) else None,
        "harness": "claude_code",
    }


def upload(base: str, payload: dict) -> None:
    url = f"{base}/api/transcripts"
    headers = {"Content-Type": "application/json"}
    headers.update(cadri_auth.auth_headers(base, REFRESH_TIMEOUT_SECONDS))
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=UPLOAD_TIMEOUT_SECONDS) as response:
            response.read()
    except urllib.error.HTTPError as exc:
        detail = ""
        if exc.code == 403:
            try:
                body = json.loads(exc.read().decode())
            except (OSError, UnicodeError, json.JSONDecodeError):
                body = None
            if isinstance(body, dict) and body.get("error") == "no_organization":
                detail = "; open Cadri in the browser, create your team, then run cadri login"
        if not detail and exc.code in (401, 403):
            detail = "; run cadri login or set CADRI_API_TOKEN"
        print(f"cadri: {url} returned {exc.code}{detail}", file=sys.stderr)
    except (OSError, ValueError) as exc:
        print(f"cadri: could not reach {url}: {exc}", file=sys.stderr)


def main() -> int:
    try:
        from shared import memory

        memory.touch_heartbeat_quietly()
    except Exception:  # noqa: BLE001
        pass
    try:
        event = read_event()
        if event is not None:
            upload(cadri_auth.server_url(), build_payload(event))
    except Exception as exc:  # noqa: BLE001
        print(f"cadri: capture failed: {exc}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
