"""Resolve a push conflict in a scratch worktree, never in the shared clone.

When local commits conflict with the server, push aborts the rebase in the shared
clone (other agents keep editing there) and replays the commits here instead, on the
local branch `cadri-repair`. The worktree shares the clone's objects and refs, so a
push from it updates origin/main for the clone too. The server only ever sees a
fast-forward push to main.
"""

from __future__ import annotations

import shutil
import time
from pathlib import Path

from ..state import read_json, write_json
from .books import Book, MemoryFailure, git

BRANCH = "cadri-repair"
EXIT_CONFLICT = 3
MARKERS = ("<<<<<<< ", ">>>>>>> ")


def worktree(book: Book) -> Path:
    return book.sibling(".repair")


def marker_path(book: Book) -> Path:
    return book.sibling(".repair.json")


def active(book: Book) -> bool:
    return marker_path(book).is_file() and worktree(book).is_dir()


def marker(book: Book) -> dict:
    return read_json(marker_path(book))


def conflicted(book: Book) -> list[str]:
    result = git(book, "diff", "--name-only", "--diff-filter=U", cwd=worktree(book), check=False)
    return [line for line in result.stdout.splitlines() if line]


def in_rebase(book: Book) -> bool:
    result = git(book, "rev-parse", "--git-path", "rebase-merge", cwd=worktree(book), check=False)
    path = Path(result.stdout.strip())
    if not path.is_absolute():
        path = worktree(book) / path
    return result.returncode == 0 and path.exists()


def remove(book: Book) -> None:
    path = worktree(book)
    if path.exists():
        git(book, "worktree", "remove", "--force", str(path), check=False)
        shutil.rmtree(path, ignore_errors=True)
    git(book, "worktree", "prune", check=False)
    git(book, "branch", "-D", BRANCH, check=False)
    marker_path(book).unlink(missing_ok=True)


def instructions(book: Book, conflicts: list[str]) -> str:
    files = "\n".join(f"  {path}" for path in conflicts) or "  (none listed)"
    return (
        f"Memory push conflict in the {book.label} book. Resolve it in the scratch worktree, not the shared clone:\n"
        f"  {worktree(book)}\n"
        f"Conflicted files:\n{files}\n"
        "Follow the cadri-memory-repair skill: keep both sides' facts, remove the conflict markers, keep frontmatter valid,\n"
        f"then run `cadri memory push --continue --book {book.scope}`. "
        f"To give up for now: `cadri memory push --abort --book {book.scope}`."
    )


def begin(book: Book) -> list[str]:
    """Replay the clone's unpushed commits onto origin/main in the worktree. Call with the book lock held."""
    remove(book)
    git(book, "worktree", "add", "--quiet", "-B", BRANCH, str(worktree(book)), "HEAD")
    result = git(book, "rebase", "origin/main", cwd=worktree(book), check=False)
    conflicts = conflicted(book) if result.returncode != 0 else []
    write_json(marker_path(book), {"started_at": time.time(), "conflicts": conflicts})
    return conflicts


def leftover_markers(book: Book, paths: list[str]) -> list[str]:
    found = []
    for relative in paths:
        try:
            text = (worktree(book) / relative).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        if any(line.startswith(MARKERS) for line in text.splitlines()):
            found.append(relative)
    return found


def advance(book: Book) -> list[str]:
    """Continue the rebase after the agent resolved files. Returns files still conflicted."""
    if not in_rebase(book):
        return []
    pending = marker(book).get("conflicts") or conflicted(book)
    unresolved = leftover_markers(book, pending)
    if unresolved:
        return unresolved
    git(book, "add", "-A", cwd=worktree(book))
    while True:
        result = git(book, "rebase", "--continue", cwd=worktree(book), check=False)
        if result.returncode == 0 and not in_rebase(book):
            return []
        conflicts = conflicted(book)
        if conflicts:
            write_json(marker_path(book), {"started_at": marker(book).get("started_at"), "conflicts": conflicts})
            return conflicts
        if result.returncode != 0 and "nothing to commit" not in (result.stdout + result.stderr):
            raise MemoryFailure(f"rebase --continue failed: {(result.stderr or result.stdout).strip()}")
        git(book, "rebase", "--skip", cwd=worktree(book), check=False)
        if not in_rebase(book):
            return []


def adopt(book: Book) -> bool:
    """Move the shared clone to the pushed result and drop the worktree. Call with the lock held.

    Returns False when uncommitted edits in the clone block the move; the next sync's
    rebase then drops the already-pushed commits by patch id.
    """
    moved = git(book, "reset", "--keep", "--quiet", "origin/main", check=False).returncode == 0
    remove(book)
    return moved
