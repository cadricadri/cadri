"""Best-effort durations for CLI, hooks, and the MCP search tool.

The engine keeps an allowlist of command names. A missing server, a missing
login, or a slow post never changes the command's exit code.
"""

from __future__ import annotations

import json
import math
import os
import urllib.request

from . import auth

TIMEOUT_SECONDS = 1.0


def report(command: str, seconds: float) -> None:
    if not math.isfinite(seconds) or seconds < 0 or not _configured():
        return
    url = f"{auth.server_url().rstrip('/')}/api/telemetry/plugin"
    body = json.dumps({"command": command, "duration_ms": seconds * 1000}).encode()
    request = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS):
            return
    except Exception:
        # A beacon must not change the command's exit code.
        return


def _configured() -> bool:
    if os.environ.get("CADRI_URL") or os.environ.get("CADRI_API_TOKEN"):
        return True
    try:
        return bool(auth.load_credentials())
    except OSError:
        return False
