"""Local state under ~/.cadri (or $CADRI_HOME): atomic files, locks, per-session records.

Hooks for one session can run at the same time (parallel tool calls, subagents), and
several sessions share one machine, so every write here is either an atomic replace
or an append under an exclusive lock.
"""

from __future__ import annotations

import contextlib
import json
import os
import re
import tempfile
import time
from pathlib import Path
from typing import Iterator

try:
    import fcntl
except ImportError:  # pragma: no cover - non-POSIX
    fcntl = None


def cadri_home() -> Path:
    home = os.environ.get("CADRI_HOME")
    return Path(home) if home else Path.home() / ".cadri"


def state_dir() -> Path:
    return cadri_home() / "state"


def log_path() -> Path:
    return cadri_home() / "hooks.log"


def safe_part(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", value)[:128] or "_"


def read_json(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def write_text(path: Path, text: str, mode: int = 0o600) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        with contextlib.suppress(OSError):
            os.unlink(tmp)
        raise


def write_json(path: Path, data: dict) -> None:
    write_text(path, json.dumps(data, indent=2, sort_keys=True) + "\n")


class LockTimeout(Exception):
    pass


@contextlib.contextmanager
def locked(path: Path, timeout: float | None = None) -> Iterator[None]:
    """Hold an exclusive flock on `path`. timeout=None waits forever, 0 tries once."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a+") as handle:
        if fcntl is not None:
            deadline = None if timeout is None else time.monotonic() + timeout
            while True:
                try:
                    fcntl.flock(handle, fcntl.LOCK_EX | (0 if deadline is None else fcntl.LOCK_NB))
                    break
                except BlockingIOError:
                    if time.monotonic() >= deadline:
                        raise LockTimeout(str(path)) from None
                    time.sleep(0.05)
        yield


def append_jsonl(path: Path, record: dict) -> None:
    line = json.dumps(record, sort_keys=True) + "\n"
    with locked(path.with_name(path.name + ".lock")):
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(line)


def read_jsonl(path: Path) -> list[dict]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError:
        return []
    records = []
    for line in lines:
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict):
            records.append(record)
    return records


class Session:
    """What cadri remembers about one agent session between its hook calls."""

    def __init__(self, harness: str, session_id: str):
        self.harness = harness
        self.session_id = session_id
        self.path = state_dir() / "sessions" / f"{safe_part(harness)}-{safe_part(session_id)}.json"
        self.lock = self.path.with_name(self.path.name + ".lock")

    def load(self) -> dict:
        return read_json(self.path)

    def update(self, **changes: object) -> dict:
        with locked(self.lock, timeout=2):
            data = self.load()
            data.update(changes)
            data["updated_at"] = time.time()
            write_json(self.path, data)
            return data

    def bump(self, key: str, by: int = 1) -> int:
        with locked(self.lock, timeout=2):
            data = self.load()
            data[key] = int(data.get(key) or 0) + by
            data["updated_at"] = time.time()
            write_json(self.path, data)
            return data[key]


def prune(directory: Path, older_than: float) -> None:
    """Delete files in `directory` not modified for `older_than` seconds."""
    cutoff = time.time() - older_than
    try:
        entries = list(directory.iterdir())
    except OSError:
        return
    for entry in entries:
        with contextlib.suppress(OSError):
            if entry.is_file() and entry.stat().st_mtime < cutoff:
                entry.unlink()
