"""Credentials, refresh, and device sign-in, shared by hooks and `cadri login`.

Credentials live in ~/.config/cadri/credentials.json (mode 0600), keyed by the
normalized server URL: {"<url>": {"access_token", "refresh_token", "expires_at"}}.
A sign-in a hook started but the user has not approved yet lives in
pending-login.json next to it, so every hook in every session shows the same code.
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
    """--url, then CADRI_URL, then the server `cadri init` last set up, then hosted Cadri."""
    return normalize_url(override or os.environ.get("CADRI_URL") or saved_url() or DEFAULT_URL)


def config_path() -> Path:
    return Path.home() / ".config" / "cadri" / "config.json"


def saved_url() -> str | None:
    try:
        data = json.loads(config_path().read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    url = data.get("url") if isinstance(data, dict) else None
    return url if isinstance(url, str) and url else None


def save_url(base: str) -> None:
    """Remember the server for hooks, which run without the CADRI_URL of the shell that ran init."""
    path = config_path()
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    path.write_text(json.dumps({"url": normalize_url(base)}, indent=2) + "\n", encoding="utf-8")


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
        print("cadri: no refresh token; run cadri login", file=sys.stderr)
        return None
    try:
        status, body = post_json(f"{base}/auth/refresh", {"refresh_token": refresh}, timeout)
    except (OSError, ValueError) as exc:
        print(f"cadri: could not refresh credentials: {exc}", file=sys.stderr)
        return None
    refreshed = token_entry(body, entry) if status == 200 else None
    if refreshed is None:
        print(f"cadri: credential refresh failed ({status}); run cadri login", file=sys.stderr)
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


class AuthFailure(Exception):
    pass


class SignInUnsupported(AuthFailure):
    """The server has no sign-in, as when it runs without auth; requests go through unsigned."""


OK = "ok"
MISSING = "missing"
EXPIRED = "expired"
OFFLINE = "offline"


def check(base: str, timeout: float) -> str:
    """OK, MISSING (never signed in), EXPIRED (refresh refused), or OFFLINE (could not ask)."""
    if os.environ.get("CADRI_API_TOKEN"):
        return OK
    entry = load_credentials().get(base)
    if not isinstance(entry, dict) or not entry.get("access_token"):
        return MISSING
    expires_at = entry.get("expires_at")
    if not isinstance(expires_at, (int, float)) or expires_at - time.time() > REFRESH_MARGIN_SECONDS:
        return OK
    refresh = entry.get("refresh_token")
    if not isinstance(refresh, str) or not refresh:
        return EXPIRED if expires_at <= time.time() else OK
    try:
        status, body = post_json(f"{base}/auth/refresh", {"refresh_token": refresh}, timeout)
    except (OSError, ValueError):
        return OFFLINE
    refreshed = token_entry(body, entry) if status == 200 else None
    if refreshed is None:
        return OFFLINE if status >= 500 else EXPIRED
    update_entry(base, refreshed)
    return OK


def start_device(base: str, timeout: float = 15) -> dict:
    status, body = post_json(f"{base}/auth/device/start", {}, timeout)
    if status == 404:
        raise SignInUnsupported(f"{base} does not support sign-in; set CADRI_API_TOKEN instead")
    if status != 200 or not isinstance(body.get("device_code"), str):
        raise AuthFailure(f"could not start sign-in ({status}): {body.get('error', 'bad response')}")
    return body


def seconds(value: object, default: float) -> float:
    return float(value) if isinstance(value, (int, float)) and value >= 0 else default


SLOW_DOWN_SECONDS = 5


def poll_device(base: str, device: dict, timeout: float = 15, sleep=time.sleep) -> dict:
    """Wait for the user to approve `device`; return the credentials entry."""
    interval = max(1.0, seconds(device.get("interval"), 5))
    deadline = time.monotonic() + seconds(device.get("expires_in"), 600)
    while time.monotonic() < deadline:
        sleep(interval)
        status, body = post_json(f"{base}/auth/device/token", {"device_code": device["device_code"]}, timeout)
        error = body.get("error")
        if status == 200:
            entry = token_entry(body)
            if entry is None:
                raise AuthFailure("server returned an invalid token response")
            return entry
        if status == 428:
            continue
        if status == 429:
            interval += SLOW_DOWN_SECONDS
            continue
        if error == "expired_token":
            raise AuthFailure("the code expired; run cadri login again")
        if error == "access_denied":
            raise AuthFailure("sign-in was denied")
        raise AuthFailure(f"unexpected response ({status}): {error or 'no error message'}")
    raise AuthFailure("the code expired; run cadri login again")


def pending_path() -> Path:
    return credentials_path().with_name("pending-login.json")


def pending_login(base: str) -> dict | None:
    """The unexpired sign-in a hook started for `base`, if any."""
    try:
        data = json.loads(pending_path().read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    entry = data.get(base) if isinstance(data, dict) else None
    if not isinstance(entry, dict) or not isinstance(entry.get("expires_at"), (int, float)):
        return None
    return entry if entry["expires_at"] > time.time() + 30 else None


def save_pending(base: str, entry: dict | None) -> None:
    path = pending_path()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        data = {}
    if not isinstance(data, dict):
        data = {}
    if entry is None:
        data.pop(base, None)
    else:
        data[base] = entry
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    tmp = path.with_name(f".{path.name}-{os.getpid()}")
    tmp.write_text(json.dumps(data, indent=2), encoding="utf-8")
    os.chmod(tmp, 0o600)
    os.replace(tmp, path)


def begin_background_login(base: str, timeout: float) -> dict:
    """Start a device sign-in for hooks to show; the caller polls it with `finish_background_login`."""
    existing = pending_login(base)
    if existing is not None:
        return existing
    device = start_device(base, timeout)
    url = device.get("verification_uri_complete") or device.get("verification_uri")
    entry = {
        "device_code": device["device_code"],
        "user_code": device.get("user_code"),
        "url": url,
        "interval": seconds(device.get("interval"), 5),
        "expires_at": time.time() + seconds(device.get("expires_in"), 600),
    }
    save_pending(base, entry)
    return entry


def finish_background_login(base: str, entry: dict) -> bool:
    """Poll a pending sign-in to the end. Returns whether credentials were saved.

    Every session's hook may ask for this; only the first poller runs.
    """
    from .state import LockTimeout, locked

    device = {
        "device_code": entry["device_code"],
        "interval": entry.get("interval", 5),
        "expires_in": max(0.0, entry["expires_at"] - time.time()),
    }
    try:
        with locked(pending_path().with_name("pending-login.lock"), timeout=0):
            try:
                credentials = poll_device(base, device)
            except (AuthFailure, OSError, ValueError) as exc:
                print(f"cadri: background sign-in ended: {exc}", file=sys.stderr)
                save_pending(base, None)
                return False
            update_entry(base, credentials)
            save_pending(base, None)
            return True
    except LockTimeout:
        return False
