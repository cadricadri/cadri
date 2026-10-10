#!/usr/bin/env python3
"""Read a Claude Code hook event and POST the full session transcript.

Claude Code writes the conversation to the JSONL file at transcript_path and
passes that path, plus session_id, on stdin. This script sends the file as-is.
Set CADRI_URL to the engine origin (default http://127.0.0.1:8080).
"""

import json
import os
import sys
import urllib.error
import urllib.request


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


def main() -> int:
    try:
        event = json.load(sys.stdin)
    except json.JSONDecodeError as exc:
        print(f"cadri: invalid hook input: {exc}", file=sys.stderr)
        return 1

    if not isinstance(event, dict):
        print("cadri: hook input must be a JSON object", file=sys.stderr)
        return 1

    session_id = event.get("session_id")
    if not isinstance(session_id, str) or not session_id:
        print("cadri: hook input missing session_id", file=sys.stderr)
        return 1

    content = with_trailing_message(read_transcript(event.get("transcript_path")), event)
    cwd = event.get("cwd")
    hook_event = event.get("hook_event_name")
    payload = {
        "session_id": session_id,
        "content": content,
        "cwd": cwd if isinstance(cwd, str) else None,
        "hook_event": hook_event if isinstance(hook_event, str) else None,
    }

    base = os.environ.get("CADRI_URL", "http://127.0.0.1:8080").rstrip("/")
    url = f"{base}/api/transcripts"
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            response.read()
    except urllib.error.URLError as exc:
        # Leave the Claude session usable when the engine is not running.
        print(f"cadri: could not reach {url}: {exc}", file=sys.stderr)
        return 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
