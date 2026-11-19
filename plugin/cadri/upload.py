"""POST sessions to a Cadri server.

Secret values are removed from the transcript before it is sent. The local
transcript file is left as the harness wrote it.
"""

from __future__ import annotations

import json
import sys
import urllib.error
import urllib.request
from typing import Any

from . import auth as cadri_auth
from . import secrets

REFRESH_TIMEOUT_SECONDS = 5
UPLOAD_TIMEOUT_SECONDS = 30


def without_secrets(payload: dict[str, Any]) -> dict[str, Any]:
    """The payload Cadri will store. Credential values in `content` are removed."""
    content = payload.get("content")
    if not isinstance(content, str):
        return payload
    cleaned = secrets.redact(content)
    if cleaned == content:
        return payload
    print("cadri: removed secrets from the session before upload", file=sys.stderr)
    return {**payload, "content": cleaned}


def post_session(base: str, payload: dict[str, Any], timeout: float = UPLOAD_TIMEOUT_SECONDS) -> tuple[int, dict]:
    payload = without_secrets(payload)
    url = f"{base.rstrip('/')}/api/sessions"
    headers = {"Content-Type": "application/json", "Accept": "application/json"}
    headers.update(cadri_auth.auth_headers(base, REFRESH_TIMEOUT_SECONDS))
    request = urllib.request.Request(
        url,
        data=json.dumps(payload).encode(),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read()
            try:
                parsed = json.loads(body or b"{}")
            except json.JSONDecodeError:
                parsed = {}
            return response.status, parsed if isinstance(parsed, dict) else {}
    except urllib.error.HTTPError as exc:
        try:
            parsed = json.loads(exc.read() or b"{}")
        except (OSError, json.JSONDecodeError):
            parsed = {}
        return exc.code, parsed if isinstance(parsed, dict) else {}
