"""POST sessions to a Cadri server."""

from __future__ import annotations

import json
import urllib.error
import urllib.request
from typing import Any

from . import auth as cadri_auth

REFRESH_TIMEOUT_SECONDS = 5
UPLOAD_TIMEOUT_SECONDS = 30


def post_session(base: str, payload: dict[str, Any], timeout: float = UPLOAD_TIMEOUT_SECONDS) -> tuple[int, dict]:
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
