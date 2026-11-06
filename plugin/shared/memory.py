"""Shared clone of the team and personal memory books, kept in sync with the server.

Layout under ~/.cadri/memory (or $CADRI_HOME/memory):
  <host>/<org_id>/team/             clone of <base>/git/team.git
  <host>/<org_id>/user-<user_id>/   clone of <base>/git/user.git
  <host>/<org_id>/<book>.lock       flock held while syncing
  <host>/<org_id>/<book>.watch.lock flock held by the background watcher
  heartbeat                         touched by every hook run

Agents write through the edit API (`write`), never by committing in the clone.
The access token is passed to git with `-c http.extraHeader=...` per command and
is never written to disk.
"""

from __future__ import annotations

import json
import os
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from shared import cadri_auth

try:
    import fcntl
except ImportError:  # pragma: no cover - non-POSIX
    fcntl = None

REQUEST_TIMEOUT = 15
GIT_TIMEOUT = 120
WATCH_INTERVAL_SECONDS = 30
WATCH_IDLE_SECONDS = 15 * 60
EXIT_CONFLICT = 3


class MemoryFailure(Exception):
    """A failure to report as `cadri: <message>` with exit status `code`."""

    def __init__(self, message: str, code: int = 1):
        super().__init__(message)
        self.code = code


def memory_root() -> Path:
    home = os.environ.get("CADRI_HOME")
    return (Path(home) if home else Path.home() / ".cadri") / "memory"


def heartbeat_path() -> Path:
    return memory_root() / "heartbeat"


def watch_log_path() -> Path:
    return memory_root() / "watch.log"


def touch_heartbeat() -> None:
    path = heartbeat_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.touch()


def safe_part(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", value) or "_"


def host_dir_name(base: str) -> str:
    parts = urllib.parse.urlsplit(base)
    host = parts.hostname or "unknown"
    if ":" in host:
        host = host.replace(":", "-")
    if parts.port and parts.port != cadri_auth.DEFAULT_PORTS.get(parts.scheme):
        host = f"{host}-{parts.port}"
    return safe_part(host)


class Book:
    """One book of one signed-in identity: where it lives and how to reach it."""

    def __init__(self, base: str, org_id: str, user_id: str, scope: str):
        if scope not in ("team", "user"):
            raise MemoryFailure(f"unknown scope {scope!r}; use team or user")
        self.base = base
        self.scope = scope
        self.name = "team" if scope == "team" else f"user-{safe_part(user_id)}"
        self.org_dir = memory_root() / host_dir_name(base) / safe_part(org_id)
        self.path = self.org_dir / self.name

    @property
    def remote(self) -> str:
        return f"{self.base}/git/{self.scope}.git"

    @property
    def query(self) -> str:
        return "?scope=user" if self.scope == "user" else ""

    @property
    def lock_path(self) -> Path:
        return self.org_dir / f"{self.name}.lock"

    @property
    def watch_lock_path(self) -> Path:
        return self.org_dir / f"{self.name}.watch.lock"

    @property
    def synced_path(self) -> Path:
        return self.org_dir / f"{self.name}.synced"

    def cloned(self) -> bool:
        return (self.path / ".git").exists()


def whoami(base: str) -> tuple[str, str]:
    status, _, body = http_json("GET", f"{base}/api/me", base)
    org_id, user_id = body.get("org_id"), body.get("user_id")
    if status in (401, 403):
        raise MemoryFailure("not signed in; run cadri login or set CADRI_API_TOKEN")
    if status != 200 or not isinstance(org_id, str) or not isinstance(user_id, str):
        raise MemoryFailure(f"could not read identity from {base} ({status})")
    return org_id, user_id


def open_book(scope: str, url: str | None = None) -> Book:
    base = cadri_auth.server_url(url)
    org_id, user_id = whoami(base)
    return Book(base, org_id, user_id, scope)


def http_json(
    method: str,
    url: str,
    base: str,
    body: dict | None = None,
    headers: dict[str, str] | None = None,
) -> tuple[int, dict[str, str], dict]:
    """Return (status, response headers, parsed JSON). HTTP errors (incl. 304) are returned."""
    merged = {"Accept": "application/json"}
    merged.update(cadri_auth.auth_headers(base))
    merged.update(headers or {})
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        merged["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=merged, method=method)
    try:
        with urllib.request.urlopen(request, timeout=REQUEST_TIMEOUT) as response:
            return response.status, dict(response.headers), cadri_auth.parse_json(response.read())
    except urllib.error.HTTPError as exc:
        return exc.code, dict(exc.headers or {}), cadri_auth.parse_json(exc.read())
    except (OSError, ValueError) as exc:
        raise MemoryFailure(f"could not reach {url}: {exc}") from exc


def git(book: Book, *args: str, cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess:
    auth = cadri_auth.auth_headers(book.base).get("Authorization")
    # A server in auth mode none needs no header; one that wants it answers 401 and
    # whoami/sync report "not signed in".
    command = ["git", *(["-c", f"http.extraHeader=Authorization: {auth}"] if auth else []), *args]
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0")
    try:
        result = subprocess.run(
            command,
            cwd=str(cwd) if cwd else None,
            env=env,
            capture_output=True,
            text=True,
            timeout=GIT_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise MemoryFailure(f"git {args[0]} failed: {exc}") from exc
    if check and result.returncode != 0:
        raise MemoryFailure(f"git {args[0]} failed: {scrub(result.stderr or result.stdout)}")
    return result


def scrub(text: str) -> str:
    return re.sub(r"Bearer\s+\S+", "Bearer ***", text).strip()


def local_head(book: Book) -> str | None:
    result = git(book, "rev-parse", "--verify", "--quiet", "HEAD", cwd=book.path, check=False)
    sha = result.stdout.strip()
    return sha if result.returncode == 0 and re.fullmatch(r"[0-9a-f]{40}", sha) else None


def ensure_clone(book: Book) -> None:
    if book.cloned():
        return
    book.org_dir.mkdir(parents=True, exist_ok=True)
    git(book, "-c", "init.defaultBranch=main", "clone", book.remote, str(book.path))


class SyncLock:
    """Non-blocking exclusive flock; `acquired` is False when someone else holds it."""

    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.handle = open(path, "a+")
        self.acquired = True
        if fcntl is not None:
            try:
                fcntl.flock(self.handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except OSError:
                self.acquired = False

    def release(self) -> None:
        self.handle.close()

    def __enter__(self) -> "SyncLock":
        return self

    def __exit__(self, *_: object) -> None:
        self.release()


def record_sync(book: Book) -> None:
    book.synced_path.write_text(str(int(time.time())), encoding="utf-8")


def sync(book: Book, quiet: bool = False) -> int:
    """Fast-forward the clone to the server tip. Returns a process exit status."""
    with SyncLock(book.lock_path) as lock:
        if not lock.acquired:
            return 0
        if not book.cloned():
            ensure_clone(book)
            record_sync(book)
            return 0
        head = local_head(book)
        headers = {"If-None-Match": f'"{head or "empty"}"'}
        status, _, tip = http_json("GET", f"{book.base}/api/memories/tip{book.query}", book.base, headers=headers)
        if status == 304:
            record_sync(book)
            return 0
        if status in (401, 403):
            raise MemoryFailure("not signed in; run cadri login or set CADRI_API_TOKEN")
        if status != 200:
            raise MemoryFailure(f"tip request failed ({status})")
        if tip.get("tip") is None:
            record_sync(book)
            return 0
        git(book, "fetch", "origin", "main", cwd=book.path)
        if git(book, "merge", "--ff-only", "origin/main", cwd=book.path, check=False).returncode != 0:
            if git(book, "rebase", "origin/main", cwd=book.path, check=False).returncode != 0:
                git(book, "rebase", "--abort", cwd=book.path, check=False)
                print(
                    f"cadri: {book.path} has local commits that conflict with the server; "
                    "resolve them by hand (git rebase origin/main) or discard them",
                    file=sys.stderr,
                )
                return 1
        record_sync(book)
        if not quiet:
            print(f"cadri: synced {book.path}")
        return 0


def default_agent_id() -> str:
    explicit = os.environ.get("CADRI_AGENT_ID")
    if explicit:
        return explicit
    harness = os.environ.get("CADRI_HARNESS") or "agent"
    return f"{safe_part(harness)}-{secrets.token_hex(2)}"


def write(book: Book, path: str, content: str, message: str, agent: str | None) -> int:
    ensure_clone(book)
    head = local_head(book)
    body: dict = {
        "message": message,
        "changes": [{"op": "write", "path": path, "content": content}],
        "agent": agent or default_agent_id(),
    }
    if head:
        body["base"] = head
    status, _, reply = http_json(
        "POST", f"{book.base}/api/memories/commits?view=tree{book.query.replace('?', '&')}", book.base, body
    )
    if status == 409:
        paths = reply.get("paths")
        if isinstance(paths, list) and paths:
            print("cadri: conflict; these paths changed since your clone:", file=sys.stderr)
            for item in paths:
                print(f"  {item}", file=sys.stderr)
            print("run cadri memory sync, re-read them, and write again", file=sys.stderr)
            return EXIT_CONFLICT
        raise MemoryFailure(f"write rejected: {reply.get('error', 'conflict')}")
    if status in (401, 403):
        raise MemoryFailure("not signed in; run cadri login or set CADRI_API_TOKEN")
    if status != 200:
        raise MemoryFailure(f"write failed ({status}): {reply.get('error', 'no error message')}")
    sync_status = sync(book, quiet=True)
    commit = reply.get("commit") or reply.get("tip") or local_head(book)
    if commit:
        print(commit)
    return sync_status


def touch_heartbeat_quietly() -> None:
    try:
        touch_heartbeat()
    except OSError:
        pass


def detach_watcher(cli: Path) -> None:
    log = watch_log_path()
    log.parent.mkdir(parents=True, exist_ok=True)
    with open(log, "a") as out:
        subprocess.Popen(
            [sys.executable, str(cli), "memory", "watch"],
            stdin=subprocess.DEVNULL,
            stdout=out,
            stderr=out,
            start_new_session=True,
        )


def heartbeat_age() -> float:
    try:
        return time.time() - heartbeat_path().stat().st_mtime
    except OSError:
        return float("inf")


def watch(url: str | None = None, interval: float = WATCH_INTERVAL_SECONDS, sleep=time.sleep) -> int:
    base = cadri_auth.server_url(url)
    org_id, user_id = whoami(base)
    if not heartbeat_path().exists():
        touch_heartbeat_quietly()
    team = Book(base, org_id, user_id, "team")
    personal = Book(base, org_id, user_id, "user")
    with SyncLock(team.watch_lock_path) as lock:
        if not lock.acquired:
            return 0
        while heartbeat_age() < WATCH_IDLE_SECONDS:
            for book in (team, personal):
                if book is personal and not book.cloned():
                    continue
                try:
                    sync(book, quiet=True)
                except MemoryFailure as exc:
                    print(f"cadri: sync {book.name}: {exc}", file=sys.stderr, flush=True)
            sleep(interval)
    return 0
