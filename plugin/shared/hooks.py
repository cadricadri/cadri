"""`cadri hook <harness>`: turn one harness hook event into a session upload.

Every harness calls the same entry point with its own JSON on stdin. The hook
answers the harness at once and uploads in a detached child, so a slow network
never stalls the agent. CADRI_HOOK_FOREGROUND=1 keeps everything in-process.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import sys
import time
from pathlib import Path

from . import cadri_auth, memory
from .discover import opencode
from .upload import post_session

HARNESSES = ("claude_code", "cursor", "codex", "opencode")
ALIASES = {"claude": "claude_code"}
START_EVENTS = {"SessionStart", "sessionStart", "session.created"}
UPLOAD_TIMEOUT_SECONDS = 30
SESSION_ID = re.compile(r"^[A-Za-z0-9._:-]{1,128}$")


def log_path() -> Path:
    home = os.environ.get("CADRI_HOME")
    return (Path(home) if home else Path.home() / ".cadri") / "hooks.log"


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
    hook_event = text(event, "hook_event_name")
    cwd = text(event, "cwd")
    if harness == "cursor":
        thread_id = text(event, "conversation_id", "session_id")
        if thread_id is None:
            return None
        path = text(event, "transcript_path") or cursor_transcript(thread_id)
        content = read_file(path)
        roots = event.get("workspace_roots")
        if cwd is None and isinstance(roots, list) and roots and isinstance(roots[0], str):
            cwd = roots[0]
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
        "cwd": cwd,
        "hook_event": hook_event,
        "harness": harness,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }
    model = text(event, "model")
    if model:
        payload["model"] = model
    if harness == "claude_code":
        payload["session_id"] = thread_id
    return payload


def upload(payload: dict) -> None:
    base = cadri_auth.server_url()
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


def export_session(event: dict) -> None:
    """Claude Code sources `CLAUDE_ENV_FILE` before each Bash command, so `cadri memory
    write` in this session picks up `CADRI_SESSION_ID` and links its commits here."""
    env_file = os.environ.get("CLAUDE_ENV_FILE")
    session_id = text(event, "session_id")
    if not env_file or session_id is None or not SESSION_ID.match(session_id):
        return
    try:
        with open(env_file, "a", encoding="utf-8") as handle:
            handle.write(f"export CADRI_SESSION_ID={shlex.quote(session_id)}\n")
            handle.write("export CADRI_HARNESS=claude_code\n")
    except OSError as exc:
        print(f"cadri: session id not exported: {exc}", file=sys.stderr)


def start_memory_watcher() -> None:
    if not memory.git_ready():
        return
    try:
        memory.touch_heartbeat()
        memory.detach_watcher(Path(__file__).resolve().parent.parent / "bin" / "cadri_cli.py")
    except Exception as exc:  # noqa: BLE001
        print(f"cadri: memory watcher not started: {exc}", file=sys.stderr)


def handle(harness: str, event: dict) -> None:
    memory.touch_heartbeat_quietly()
    if text(event, "hook_event_name") in START_EVENTS:
        start_memory_watcher()
    payload = build_payload(harness, event)
    if payload is not None:
        upload(payload)


def detach() -> bool:
    """Fork; return True in the child, which carries on with stdio sent to the hook log."""
    if os.environ.get("CADRI_HOOK_FOREGROUND") == "1":
        return True
    try:
        pid = os.fork()
    except OSError:
        return True
    if pid:
        return False
    os.setsid()
    log = log_path()
    try:
        log.parent.mkdir(parents=True, exist_ok=True)
        out = os.open(log, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    except OSError:
        out = os.open(os.devnull, os.O_WRONLY)
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    os.dup2(out, 1)
    os.dup2(out, 2)
    return True


def run(name: str, stdin: str) -> int:
    harness = ALIASES.get(name, name)
    if harness not in HARNESSES:
        print(f"cadri: unknown harness {name!r}", file=sys.stderr)
        return 0
    if harness == "cursor":
        # Cursor reads the hook's stdout as JSON; an empty object means "carry on".
        print("{}", flush=True)
    try:
        event = json.loads(stdin)
    except json.JSONDecodeError as exc:
        print(f"cadri: invalid hook input: {exc}", file=sys.stderr)
        return 0
    if not isinstance(event, dict):
        print("cadri: hook input must be a JSON object", file=sys.stderr)
        return 0
    if harness == "claude_code" and text(event, "hook_event_name") in START_EVENTS:
        export_session(event)
    sys.stdout.flush()
    sys.stderr.flush()
    if not detach():
        return 0
    try:
        handle(harness, event)
    except Exception as exc:  # noqa: BLE001
        print(f"cadri: {harness} hook failed: {exc}", file=sys.stderr)
    if os.environ.get("CADRI_HOOK_FOREGROUND") != "1":
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(0)
    return 0
