"""Keep the shared clone current and in a state agents can work in.

`quick` runs inside the session-start hook under a deadline: it classifies the clone,
fast-forwards when that is cheap, and says what is left. `full` finishes the job in a
detached process (or from `cadri memory sync`): clone, fetch, fast-forward, publish
unpushed commits, re-clone a broken checkout.

The clone is never stashed, reset, or rebased here while agents may be editing it.
Fast-forward refuses to overwrite a dirty file, and that refusal just means "later".
"""

from __future__ import annotations

import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

from ..state import LockTimeout, locked
from .books import Book, MemoryFailure, ensure_clone, git, http_json, record_sync
from . import repair

CLEAN = "clean"
MISSING = "missing"
AHEAD = "ahead"
DIVERGED = "diverged"
DIRTY_BLOCKED = "dirty-blocked"
REPAIR = "repair"
CORRUPT = "corrupt"
BUSY = "busy"
OFFLINE = "offline"


@dataclass
class Status:
    book: Book
    state: str
    detail: str = ""
    conflicts: list[str] = field(default_factory=list)
    dirty: bool = False
    needs_background: bool = False


def remote_tip(book: Book) -> str | None:
    result = git(book, "rev-parse", "--verify", "--quiet", "refs/remotes/origin/main", check=False)
    sha = result.stdout.strip()
    return sha if result.returncode == 0 and sha else None


def ahead_count(book: Book) -> int:
    if git(book, "rev-parse", "--verify", "--quiet", "HEAD", check=False).returncode != 0:
        return 0
    spec = "origin/main..HEAD" if remote_tip(book) else "HEAD"
    result = git(book, "rev-list", "--count", spec, check=False)
    try:
        return int(result.stdout.strip())
    except ValueError:
        return 0


def mid_rebase(path: Path) -> bool:
    git_dir = path / ".git"
    return (git_dir / "rebase-merge").exists() or (git_dir / "rebase-apply").exists()


def server_has_new(book: Book, timeout: float) -> bool | None:
    """Whether the server tip differs from origin/main; None when the server can't be asked."""
    known = remote_tip(book) or "empty"
    try:
        status, _, body = http_json(
            "GET",
            f"{book.base}/api/memories/tip{book.query}",
            book.base,
            headers={"If-None-Match": f'"{known}"'},
            timeout=timeout,
        )
    except MemoryFailure:
        return None
    if status == 304:
        return False
    if status != 200:
        return None
    tip = body.get("tip")
    return bool(tip) and tip != remote_tip(book)


def fast_forward(book: Book) -> bool:
    if not remote_tip(book):
        return True
    return git(book, "merge", "--ff-only", "--quiet", "origin/main", check=False).returncode == 0


def quick(book: Book, deadline: float) -> Status:
    """Classify the clone and fast-forward if the server answers before `deadline` (monotonic)."""
    if repair.active(book):
        marker = repair.marker(book)
        return Status(book, REPAIR, "a push conflict is waiting to be resolved", marker.get("conflicts", []))
    if not book.cloned():
        return Status(book, MISSING, "not cloned yet; cloning in the background", needs_background=True)
    try:
        with locked(book.lock_path, timeout=0.3):
            return quick_locked(book, deadline)
    except LockTimeout:
        return Status(book, BUSY, "another session is syncing this book")


def quick_locked(book: Book, deadline: float) -> Status:
    if mid_rebase(book.path):
        # Cadri never leaves the shared clone mid-rebase; this is left over from an older client.
        git(book, "rebase", "--abort", check=False)
    porcelain = git(book, "status", "--porcelain", check=False)
    if porcelain.returncode != 0:
        return Status(book, CORRUPT, "the clone is unreadable; re-cloning in the background", needs_background=True)
    dirty = bool(porcelain.stdout.strip())
    remaining = deadline - time.monotonic()
    new = server_has_new(book, timeout=max(0.2, min(1.0, remaining))) if remaining > 0.2 else None
    if new is None:
        state = AHEAD if ahead_count(book) else OFFLINE
        return Status(book, state, "server not reached; using the local copy", dirty=dirty, needs_background=True)
    if new:
        remaining = deadline - time.monotonic()
        if remaining < 0.3:
            return Status(book, CLEAN, "newer memory is downloading", dirty=dirty, needs_background=True)
        try:
            git(book, "fetch", "--quiet", "origin", "main", timeout=remaining, network=True)
        except MemoryFailure:
            return Status(book, CLEAN, "newer memory is downloading", dirty=dirty, needs_background=True)
        if not fast_forward(book):
            if ahead_count(book):
                return Status(book, DIVERGED, "local commits and server changes; merging in the background", dirty=dirty, needs_background=True)
            return Status(book, DIRTY_BLOCKED, "uncommitted edits block the update; it will retry", dirty=dirty, needs_background=True)
    record_sync(book)
    if ahead_count(book):
        return Status(book, AHEAD, "unpushed commits; pushing in the background", dirty=dirty, needs_background=True)
    return Status(book, CLEAN, dirty=dirty)


def full(book: Book, log=print) -> int:
    """Bring the clone fully up to date and publish unpushed commits. Returns an exit status."""
    from . import push

    if repair.active(book):
        log(f"cadri: {book.label} book has a repair in progress at {repair.worktree(book)}")
        return 0
    try:
        with locked(book.lock_path, timeout=60):
            if book.cloned() and git(book, "status", "--porcelain", check=False).returncode != 0:
                aside = book.path.with_name(f"{book.name}.corrupt-{int(time.time())}")
                book.path.rename(aside)
                log(f"cadri: moved the unreadable clone to {aside}")
            if not book.cloned():
                ensure_clone(book)
                record_sync(book)
                return 0
            if mid_rebase(book.path):
                git(book, "rebase", "--abort", check=False)
            git(book, "fetch", "--quiet", "origin", "main", network=True, check=False)
            if fast_forward(book) and not ahead_count(book):
                record_sync(book)
                return 0
            if not ahead_count(book):
                log(f"cadri: {book.label} book: uncommitted edits block the update; will retry")
                return 0
            return push.publish_locked(book, log=log)
    except LockTimeout:
        log(f"cadri: {book.label} book is busy; sync skipped")
        return 0


def pull(book: Book, quiet: bool = False) -> int:
    status = full(book, log=lambda message: print(message, file=sys.stderr))
    if status == 0 and not quiet:
        print(f"cadri: synced {book.path}")
    return status
