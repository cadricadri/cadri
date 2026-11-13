"""Where the team and personal memory books live, and how to reach them.

Layout under ~/.cadri/memory (or $CADRI_HOME/memory):
  <host>/<org_id>/team/                clone of <base>/git/team.git
  <host>/<org_id>/user-<user_id>/      clone of <base>/git/user.git
  <host>/<org_id>/<book>.lock          held around every git operation on the clone
  <host>/<org_id>/<book>.edits.jsonl   which session edited which file (see edits.py)
  <host>/<org_id>/<book>.index.json    recall index (see recall.py)
  <host>/<org_id>/<book>.repair/       scratch worktree while a push conflict is resolved
  <host>/<org_id>/<book>.repair.json   marker for that repair

The access token is passed to git with `-c http.extraHeader=...` per command and
is never written to disk.
"""

from __future__ import annotations

import functools
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from .. import auth
from ..state import cadri_home, read_json, safe_part, write_json

REQUEST_TIMEOUT = 15
GIT_TIMEOUT = 120
IDENTITY_MAX_AGE = 24 * 60 * 60


class MemoryFailure(Exception):
    """A failure to report as `cadri: <message>` with exit status `code`."""

    def __init__(self, message: str, code: int = 1):
        super().__init__(message)
        self.code = code


def memory_root() -> Path:
    return cadri_home() / "memory"


def host_dir_name(base: str) -> str:
    parts = urllib.parse.urlsplit(base)
    host = parts.hostname or "unknown"
    if ":" in host:
        host = host.replace(":", "-")
    if parts.port and parts.port != auth.DEFAULT_PORTS.get(parts.scheme):
        host = f"{host}-{parts.port}"
    return safe_part(host)


class Book:
    """One book of one signed-in identity: where it lives and how to reach it."""

    def __init__(self, base: str, org_id: str, user_id: str, scope: str):
        if scope not in ("team", "user"):
            raise MemoryFailure(f"unknown scope {scope!r}; use team or user")
        self.base = base
        self.scope = scope
        self.user_id = user_id
        self.name = "team" if scope == "team" else f"user-{safe_part(user_id)}"
        self.label = "team" if scope == "team" else "personal"
        self.org_dir = memory_root() / host_dir_name(base) / safe_part(org_id)
        self.path = self.org_dir / self.name

    def __repr__(self) -> str:
        return f"Book({self.label}, {self.path})"

    @property
    def remote(self) -> str:
        return f"{self.base}/git/{self.scope}.git"

    @property
    def query(self) -> str:
        return "?scope=user" if self.scope == "user" else ""

    def sibling(self, suffix: str) -> Path:
        return self.org_dir / f"{self.name}{suffix}"

    @property
    def lock_path(self) -> Path:
        return self.sibling(".lock")

    @property
    def synced_path(self) -> Path:
        return self.sibling(".synced")

    def cloned(self) -> bool:
        return (self.path / ".git").exists()

    def contains(self, path: str | Path) -> bool:
        try:
            Path(path).expanduser().resolve().relative_to(self.path.resolve())
        except (ValueError, OSError):
            return False
        return True

    def relative(self, path: str | Path) -> str:
        return Path(path).expanduser().resolve().relative_to(self.path.resolve()).as_posix()


def identity_path(base: str) -> Path:
    return memory_root() / host_dir_name(base) / "identity.json"


def cached_identity(base: str) -> dict:
    return read_json(identity_path(base))


def whoami(base: str, timeout: float = REQUEST_TIMEOUT) -> dict:
    """{user_id, org_id, email} from /api/me, saved so hooks can find the books offline."""
    status, _, body = http_json("GET", f"{base}/api/me", base, timeout=timeout)
    org_id, user_id = body.get("org_id"), body.get("user_id")
    if status in (401, 403):
        raise MemoryFailure("not signed in; run cadri login or set CADRI_API_TOKEN")
    if status != 200 or not isinstance(org_id, str) or not isinstance(user_id, str):
        raise MemoryFailure(f"could not read identity from {base} ({status})")
    identity = {"org_id": org_id, "user_id": user_id, "email": body.get("email"), "checked_at": time.time()}
    write_json(identity_path(base), identity)
    return identity


def identity(base: str, timeout: float = REQUEST_TIMEOUT, refresh: bool = False) -> dict:
    cached = cached_identity(base)
    fresh = time.time() - float(cached.get("checked_at") or 0) < IDENTITY_MAX_AGE
    if cached.get("org_id") and cached.get("user_id") and fresh and not refresh:
        return cached
    try:
        return whoami(base, timeout)
    except MemoryFailure:
        if cached.get("org_id") and cached.get("user_id"):
            return cached
        raise


def books_for(base: str, ident: dict) -> list[Book]:
    return [Book(base, ident["org_id"], ident["user_id"], scope) for scope in ("team", "user")]


def local_books(base: str | None = None) -> list[Book]:
    """The books of the last identity seen, without touching the network. Empty before first sign-in."""
    base = auth.server_url(base)
    cached = cached_identity(base)
    if not cached.get("org_id") or not cached.get("user_id"):
        return []
    return books_for(base, cached)


def open_book(scope: str, url: str | None = None) -> Book:
    base = auth.server_url(url)
    ident = identity(base)
    return Book(base, ident["org_id"], ident["user_id"], scope)


def http_json(
    method: str,
    url: str,
    base: str,
    body: dict | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = REQUEST_TIMEOUT,
) -> tuple[int, dict[str, str], dict]:
    """Return (status, response headers, parsed JSON). HTTP errors (incl. 304) are returned."""
    merged = {"Accept": "application/json"}
    merged.update(auth.auth_headers(base, refresh_timeout=min(5.0, timeout)))
    merged.update(headers or {})
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        merged["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=merged, method=method)
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, dict(response.headers), auth.parse_json(response.read())
    except urllib.error.HTTPError as exc:
        return exc.code, dict(exc.headers or {}), auth.parse_json(exc.read())
    except (OSError, ValueError) as exc:
        raise MemoryFailure(f"could not reach {url}: {exc}") from exc


@functools.lru_cache(maxsize=1)
def git_ready() -> bool:
    """Whether git runs. On a fresh Mac /usr/bin/git is a stub that opens the Command Line Tools installer."""
    path = shutil.which("git")
    if path is None:
        return False
    if sys.platform == "darwin" and Path(path) == Path("/usr/bin/git"):
        return subprocess.run(["xcode-select", "-p"], capture_output=True, check=False).returncode == 0
    return True


GIT_IDENTITY = ("-c", "user.name=Cadri", "-c", "user.email=memory@cadri.invalid")


def git(
    book: Book,
    *args: str,
    cwd: Path | None = None,
    check: bool = True,
    timeout: float = GIT_TIMEOUT,
    network: bool = False,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess:
    if not git_ready():
        raise MemoryFailure("git is not installed; run `xcode-select --install`")
    header: list[str] = []
    if network:
        token = auth.auth_headers(book.base, refresh_timeout=min(5.0, timeout)).get("Authorization")
        # A server in auth mode none needs no header; one that wants it answers 401.
        if token:
            header = ["-c", f"http.extraHeader=Authorization: {token}"]
    command = ["git", *header, *GIT_IDENTITY, *args]
    merged = dict(os.environ, GIT_TERMINAL_PROMPT="0", GIT_EDITOR="true", **(env or {}))
    try:
        result = subprocess.run(
            command,
            cwd=str(cwd or book.path),
            env=merged,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise MemoryFailure(f"git {args[0]} timed out") from exc
    except OSError as exc:
        raise MemoryFailure(f"git {args[0]} failed: {exc}") from exc
    if check and result.returncode != 0:
        raise MemoryFailure(f"git {args[0]} failed: {scrub(result.stderr or result.stdout)}")
    return result


def scrub(text: str) -> str:
    return re.sub(r"Bearer\s+\S+", "Bearer ***", text).strip()


def local_head(book: Book, cwd: Path | None = None) -> str | None:
    result = git(book, "rev-parse", "--verify", "--quiet", "HEAD", cwd=cwd, check=False)
    sha = result.stdout.strip()
    return sha if result.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", sha) else None


def ensure_clone(book: Book, timeout: float = GIT_TIMEOUT) -> None:
    if book.cloned():
        return
    book.org_dir.mkdir(parents=True, exist_ok=True)
    git(
        book,
        "-c",
        "init.defaultBranch=main",
        "clone",
        book.remote,
        str(book.path),
        cwd=book.org_dir,
        timeout=timeout,
        network=True,
    )


def record_sync(book: Book) -> None:
    book.synced_path.write_text(str(int(time.time())), encoding="utf-8")


def last_synced(book: Book) -> float | None:
    try:
        return float(book.synced_path.read_text(encoding="utf-8").strip())
    except (OSError, ValueError):
        return None


def session_id_from_env() -> str | None:
    return os.environ.get("CADRI_SESSION_ID", "").strip() or None
