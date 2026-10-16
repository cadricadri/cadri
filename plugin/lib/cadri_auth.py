"""Credential helpers shared by the capture hook and cadri-login.

Credentials live in ~/.config/cadri/credentials.json (mode 0600), keyed by the
normalized server URL: {"<url>": {"access_token", "refresh_token", "expires_at"}}.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

DEFAULT_URL = "https://app.cadri.io"
REFRESH_MARGIN_SECONDS = 60
DEFAULT_PORTS = {"http": 80, "https": 443}


def normalize_url(url: str) -> str:
    parts = urllib.parse.urlsplit(url.strip())
    scheme = parts.scheme.lower()
    host = (parts.hostname or "").lower()
    if ":" in host:
        host = f"[{host}]"
    if parts.port and parts.port != DEFAULT_PORTS.get(scheme):
        host = f"{host}:{parts.port}"
    return urllib.parse.urlunsplit((scheme, host, parts.path.rstrip("/"), "", ""))


def server_url(override: str | None = None) -> str:
    return normalize_url(override or os.environ.get("CADRI_URL") or DEFAULT_URL)


def credentials_path() -> Path:
    return Path.home() / ".config" / "cadri" / "credentials.json"


def load_credentials() -> dict:
    try:
        with open(credentials_path(), encoding="utf-8") as handle:
            data = json.load(handle)
    except FileNotFoundError:
        return {}
    except (OSError, json.JSONDecodeError) as exc:
        print(f"cadri: could not read {credentials_path()}: {exc}", file=sys.stderr)
        return {}
    return data if isinstance(data, dict) else {}


def save_credentials(data: dict) -> None:
    path = credentials_path()
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    # mkstemp creates the file 0600 in the same directory, so os.replace is atomic.
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=".credentials-", suffix=".json")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(data, handle, indent=2)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(tmp, 0o600)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def update_entry(base: str, entry: dict | None) -> None:
    data = load_credentials()
    if entry is None:
        data.pop(base, None)
    else:
        data[base] = entry
    save_credentials(data)


def post_json(url: str, body: dict, timeout: float) -> tuple[int, dict]:
    """POST JSON and return (status, parsed body); HTTP errors are returned, not raised."""
    request = urllib.request.Request(
        url,
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json", "Accept": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, parse_json(response.read())
    except urllib.error.HTTPError as exc:
        return exc.code, parse_json(exc.read())


def parse_json(raw: bytes) -> dict:
    try:
        data = json.loads(raw or b"{}")
    except json.JSONDecodeError:
        return {}
    return data if isinstance(data, dict) else {}


def token_entry(body: dict, previous: dict | None = None) -> dict | None:
    access = body.get("access_token")
    expires_at = body.get("expires_at")
    refresh = body.get("refresh_token") or (previous or {}).get("refresh_token")
    if not isinstance(access, str) or not access or not isinstance(expires_at, (int, float)):
        return None
    return {"access_token": access, "refresh_token": refresh, "expires_at": expires_at}


def refresh_entry(base: str, entry: dict, timeout: float) -> dict | None:
    refresh = entry.get("refresh_token")
    if not isinstance(refresh, str) or not refresh:
        print("cadri: no refresh token; run cadri-login", file=sys.stderr)
        return None
    try:
        status, body = post_json(f"{base}/auth/refresh", {"refresh_token": refresh}, timeout)
    except (OSError, ValueError) as exc:
        print(f"cadri: could not refresh credentials: {exc}", file=sys.stderr)
        return None
    refreshed = token_entry(body, entry) if status == 200 else None
    if refreshed is None:
        print(f"cadri: credential refresh failed ({status}); run cadri-login", file=sys.stderr)
        return None
    try:
        update_entry(base, refreshed)
    except OSError as exc:
        print(f"cadri: could not save refreshed credentials: {exc}", file=sys.stderr)
    return refreshed


def stored_access_token(base: str, timeout: float) -> str | None:
    entry = load_credentials().get(base)
    if not isinstance(entry, dict):
        return None
    token = entry.get("access_token")
    if not isinstance(token, str) or not token:
        return None
    expires_at = entry.get("expires_at")
    if isinstance(expires_at, (int, float)) and expires_at - time.time() <= REFRESH_MARGIN_SECONDS:
        refreshed = refresh_entry(base, entry, timeout)
        if refreshed is not None:
            return refreshed["access_token"]
    return token


def auth_headers(base: str, refresh_timeout: float = 5) -> dict[str, str]:
    token = os.environ.get("CADRI_API_TOKEN") or stored_access_token(base, refresh_timeout)
    return {"Authorization": f"Bearer {token}"} if token else {}
