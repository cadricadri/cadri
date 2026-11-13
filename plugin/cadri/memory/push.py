"""`cadri memory push`: commit memory edits in the shared clone and push them to main.

Several agents edit the same clone (the main agent, memory keepers, other sessions),
so push commits only the caller's files: those the edit record attributes to the
caller's session, files nobody recorded, and files another session left untouched
for QUIET_SECONDS. Each session's files become their own commit with the author
trailers the server requires.

Committing needs no network, so it always happens. Pushing may fail inside an agent
sandbox; then the commits wait for the Stop hook's detached push.
"""

from __future__ import annotations

import os
import sys
import time
from dataclasses import dataclass

from ..state import LockTimeout, locked
from . import edits, repair
from .books import Book, MemoryFailure, git, local_head, record_sync
from .layout import TEAM_AREAS, team_area

EXIT_CONFLICT = 3
QUIET_SECONDS = 120
LOCK_SECONDS = 30
PUSH_ATTEMPTS = 3
MAX_FILE_CHARS = 20_000
MAX_DEPTH = 2
NETWORK_ERRORS = (
    "could not resolve host",
    "failed to connect",
    "connection refused",
    "operation timed out",
    "timed out",
    "unable to access",
    "operation not permitted",
    "network is unreachable",
)

HARNESS_LABELS = {"claude": "Claude Code", "codex": "Codex", "cursor": "Cursor", "opencode": "OpenCode"}


@dataclass(frozen=True)
class Author:
    kind: str  # human | robot
    id: str
    label: str


KEEPER = "cadri-memory-keeper"


@dataclass(frozen=True)
class Caller:
    session: str | None
    harness: str | None
    agent: str
    human: Author | None = None
    # A keeper's own edits since it started count as its own, whatever session id its
    # harness put on them (a Codex subagent runs as its own thread).
    since: float | None = None


class Offline(Exception):
    pass


class Rejected(Exception):
    pass


def robot(harness: str | None, agent: str) -> Author:
    who = "memory keeper" if agent == KEEPER else "agent"
    label = f"{HARNESS_LABELS.get(harness or '', harness or 'Agent')} {who}"
    return Author("robot", f"{harness or 'agent'}-{agent}"[:64], label)


def trailers(author: Author, session: str | None) -> str:
    lines = [f"Cadri-Author-Kind: {author.kind}", f"Cadri-Author-Id: {author.id}", f"Cadri-Author-Label: {author.label}"]
    if session:
        lines.append(f"Cadri-Session-Id: {session}")
    return "\n".join(lines)


def changed_paths(book: Book) -> list[str]:
    result = git(book, "status", "--porcelain=v1", "-z", "--untracked-files=all")
    entries = result.stdout.split("\0")
    paths: list[str] = []
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if len(entry) < 4:
            continue
        status, path = entry[:2], entry[3:]
        paths.append(path)
        if "R" in status or "C" in status:
            if index < len(entries) and entries[index]:
                paths.append(entries[index])
            index += 1
    return sorted(set(paths))


def quiet_for(book: Book, path: str, edit: edits.Edit | None, now: float) -> float:
    try:
        touched = (book.path / path).stat().st_mtime
    except OSError:
        touched = 0.0
    return now - max(touched, edit.at if edit else 0.0)


def select(book: Book, caller: Caller, paths: list[str], now: float | None = None) -> dict[str | None, list[str]]:
    """Group the paths this caller may commit by the session that owns them."""
    now = time.time() if now is None else now
    latest = edits.latest(book)
    groups: dict[str | None, list[str]] = {}
    for path in paths:
        edit = latest.get(path)
        if caller.session is None:
            owner = edit.session if edit else None
        elif edit is None or edit.session == caller.session:
            owner = caller.session
        elif caller.agent == KEEPER and edit.agent == KEEPER and caller.since and edit.at >= caller.since:
            owner = caller.session
        elif quiet_for(book, path, edit, now) >= QUIET_SECONDS:
            owner = edit.session
        else:
            continue
        groups.setdefault(owner, []).append(path)
    return groups


def problems(book: Book, paths: list[str]) -> list[str]:
    found = []
    areas = set(TEAM_AREAS) if book.scope == "team" else None
    for path in paths:
        if path.count("/") > MAX_DEPTH:
            found.append(f"{path}: more than {MAX_DEPTH} directories deep")
            continue
        if areas is not None:
            area = team_area(path)
            if area is not None and area not in areas:
                found.append(
                    f"{path}: team books only use these top-level directories: {', '.join(TEAM_AREAS)}"
                )
                continue
        file = book.path / path
        if not file.exists():
            continue
        try:
            text = file.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            found.append(f"{path}: not UTF-8 text")
            continue
        except OSError as exc:
            found.append(f"{path}: {exc}")
            continue
        if len(text) > MAX_FILE_CHARS:
            found.append(f"{path}: {len(text)} characters (limit {MAX_FILE_CHARS}); split it")
    return found


def subject_for(paths: list[str]) -> str:
    if len(paths) == 1:
        return f"Update {paths[0]}"
    return f"Update {len(paths)} memory files"


def commit(book: Book, paths: list[str], message: str, author: Author, session: str | None) -> bool:
    git(book, "add", "-A", "--", *paths)
    staged = git(book, "diff", "--cached", "--name-only", "--", *paths).stdout.split()
    if not staged:
        return False
    full = f"{message.strip()}\n\n{trailers(author, session)}\n"
    git(book, "commit", "--quiet", "--no-verify", "-m", full, "--", *paths)
    return True


def classify_push_error(text: str) -> str:
    lowered = text.lower()
    if "non-fast-forward" in lowered or "fetch first" in lowered or "[rejected]" in lowered:
        return "behind"
    if any(marker in lowered for marker in NETWORK_ERRORS):
        return "offline"
    return "rejected"


def has_remote_main(book: Book) -> bool:
    return bool(git(book, "rev-parse", "--verify", "--quiet", "refs/remotes/origin/main", check=False).stdout.strip())


def rebased_elsewhere(book: Book) -> tuple[str | None, list[str] | None]:
    """Replay HEAD's unpushed commits onto origin/main in a throwaway worktree.

    The shared clone has other agents' uncommitted edits, so it is never rebased in
    place. Returns (new head, None) on success, or (None, conflicts) after handing the
    conflict to a repair worktree.
    """
    scratch = book.sibling(".integrate")
    git(book, "worktree", "remove", "--force", str(scratch), check=False)
    git(book, "worktree", "prune", check=False)
    git(book, "worktree", "add", "--quiet", "--detach", str(scratch), "HEAD")
    try:
        if git(book, "rebase", "--quiet", "origin/main", cwd=scratch, check=False).returncode == 0:
            return local_head(book, cwd=scratch), None
        git(book, "rebase", "--abort", cwd=scratch, check=False)
    finally:
        git(book, "worktree", "remove", "--force", str(scratch), check=False)
    return None, repair.begin(book)


def catch_up(book: Book) -> None:
    """Move the shared clone to origin/main after a push from elsewhere, keeping unrelated edits."""
    if git(book, "reset", "--keep", "--quiet", "origin/main", check=False).returncode != 0:
        print(
            f"cadri: {book.label} book pushed; uncommitted edits overlap, so the clone catches up on the next sync",
            file=sys.stderr,
        )


def publish_locked(book: Book, log=print) -> int:
    """Fetch, rebase, and push the clone's unpushed commits. Call with the book lock held."""
    for _ in range(PUSH_ATTEMPTS):
        fetched = git(book, "fetch", "--quiet", "origin", "main", network=True, check=False)
        if fetched.returncode != 0:
            text = fetched.stderr or fetched.stdout
            if "couldn't find remote ref" not in text.lower():
                raise Offline(text.strip())
        head = local_head(book)
        if head is None:
            return 0
        moved = has_remote_main(book) and (
            git(book, "merge-base", "--is-ancestor", "origin/main", "HEAD", check=False).returncode != 0
        )
        if moved:
            head, conflicts = rebased_elsewhere(book)
            if conflicts is not None:
                log(repair.instructions(book, conflicts))
                return EXIT_CONFLICT
        pushed = git(book, "push", "--quiet", "origin", f"{head}:refs/heads/main", network=True, check=False)
        if pushed.returncode == 0:
            git(book, "fetch", "--quiet", "origin", "main", network=True, check=False)
            if moved:
                catch_up(book)
            record_sync(book)
            return 0
        reason = classify_push_error(pushed.stderr + pushed.stdout)
        if reason == "offline":
            raise Offline((pushed.stderr or pushed.stdout).strip())
        if reason == "rejected":
            raise Rejected((pushed.stderr or pushed.stdout).strip())
    log(f"cadri: {book.label} book: the server kept moving; try again shortly")
    return 1


def push_book(book: Book, caller: Caller, message: str | None, log=print) -> int:
    if not book.cloned():
        return 0
    try:
        with locked(book.lock_path, timeout=LOCK_SECONDS):
            return push_locked(book, caller, message, log)
    except LockTimeout:
        log(f"cadri: the {book.label} book is busy (another push or sync); try again in a moment")
        return 1


def push_locked(book: Book, caller: Caller, message: str | None, log) -> int:
    groups = select(book, caller, changed_paths(book))
    selected = [path for paths in groups.values() for path in paths]
    issues = problems(book, selected)
    if issues:
        log(f"cadri: not pushed; fix these in the {book.label} book first:")
        for issue in issues:
            log(f"  {issue}")
        return 1
    made = 0
    latest = edits.latest(book)
    ordered = sorted(groups.items(), key=lambda item: item[0] != caller.session)
    for owner, paths in ordered:
        if caller.human is not None and owner == caller.session:
            author = caller.human
        elif owner == caller.session:
            author = robot(caller.harness, caller.agent)
        else:
            edit = latest.get(paths[0])
            author = robot(edit.harness if edit else caller.harness, normalize_agent(edit.agent if edit else None))
        text = message if (message and owner == caller.session) else subject_for(paths)
        if owner not in (None, caller.session):
            text = f"{subject_for(paths)}\n\nLeft uncommitted by session {owner}."
        if commit(book, paths, text, author, owner):
            made += 1
    if repair.active(book):
        if made:
            log(f"cadri: committed in the {book.label} book; a repair is in progress, so it is not pushed yet")
        return 0
    try:
        status = publish_locked(book, log)
    except Offline:
        if made:
            log(f"cadri: committed in the {book.label} book; the server was not reachable, so Cadri pushes it after this turn")
        return 0
    except Rejected as exc:
        if made:
            git(book, "reset", "--mixed", "--quiet", f"HEAD~{made}", check=False)
        log(f"cadri: the server rejected the {book.label} book push; your edits are uncommitted again:\n{exc}")
        return 1
    if status == 0 and made:
        log(f"cadri: pushed {made} commit{'s' if made > 1 else ''} to the {book.label} book")
    return status


def sweep(book: Book, harness: str | None, log=print) -> int:
    """Commit memory edits nobody has touched for QUIET_SECONDS, then publish everything.

    Runs detached after each turn, so commits an agent made without network access
    reach the server, and edits whose agent never pushed are not stranded.
    """
    if not book.cloned() or repair.active(book):
        return 0
    try:
        with locked(book.lock_path, timeout=LOCK_SECONDS):
            now = time.time()
            latest = edits.latest(book)
            groups: dict[str | None, list[str]] = {}
            for path in changed_paths(book):
                edit = latest.get(path)
                if quiet_for(book, path, edit, now) >= QUIET_SECONDS:
                    groups.setdefault(edit.session if edit else None, []).append(path)
            for owner, paths in groups.items():
                paths = [path for path in paths if not problems(book, [path])]
                if paths:
                    edit = latest.get(paths[0])
                    author = robot(edit.harness if edit else harness, normalize_agent(edit.agent if edit else None))
                    commit(book, paths, subject_for(paths), author, owner)
            if not ahead_count(book):
                return 0
            return publish_locked(book, log)
    except LockTimeout:
        return 0
    except (Offline, Rejected) as exc:
        log(f"cadri: {book.label} book not pushed: {exc}")
        return 1


def ahead_count(book: Book) -> int:
    if local_head(book) is None:
        return 0
    spec = "origin/main..HEAD" if has_remote_main(book) else "HEAD"
    result = git(book, "rev-list", "--count", spec, check=False)
    try:
        return int(result.stdout.strip())
    except ValueError:
        return 0


def resume(book: Book, log=print) -> int:
    if not repair.active(book):
        log(f"cadri: no repair in progress for the {book.label} book")
        return 0
    try:
        with locked(book.lock_path, timeout=LOCK_SECONDS):
            remaining = repair.advance(book)
            if remaining:
                log(repair.instructions(book, remaining))
                return EXIT_CONFLICT
            for _ in range(PUSH_ATTEMPTS):
                git(book, "fetch", "--quiet", "origin", "main", cwd=repair.worktree(book), network=True)
                if git(book, "merge-base", "--is-ancestor", "origin/main", "HEAD", cwd=repair.worktree(book), check=False).returncode != 0:
                    if git(book, "rebase", "--quiet", "origin/main", cwd=repair.worktree(book), check=False).returncode != 0:
                        conflicts = repair.conflicted(book)
                        log(repair.instructions(book, conflicts))
                        return EXIT_CONFLICT
                pushed = git(book, "push", "--quiet", "origin", "HEAD:refs/heads/main", cwd=repair.worktree(book), network=True, check=False)
                if pushed.returncode == 0:
                    git(book, "fetch", "--quiet", "origin", "main", network=True, check=False)
                    if not repair.adopt(book):
                        log("cadri: pushed; uncommitted edits in the shared clone overlap, so it catches up on the next sync")
                    record_sync(book)
                    log(f"cadri: conflict resolved and pushed to the {book.label} book")
                    return 0
                if classify_push_error(pushed.stderr + pushed.stdout) != "behind":
                    raise MemoryFailure(f"push failed: {(pushed.stderr or pushed.stdout).strip()}")
            log("cadri: the server kept moving; run the same command again")
            return 1
    except LockTimeout:
        log(f"cadri: the {book.label} book is busy; try again in a moment")
        return 1


def abort(book: Book, log=print) -> int:
    try:
        with locked(book.lock_path, timeout=LOCK_SECONDS):
            repair.remove(book)
    except LockTimeout:
        log(f"cadri: the {book.label} book is busy; try again in a moment")
        return 1
    log(f"cadri: repair abandoned; the unpushed commits stay in the {book.label} book and are retried later")
    return 0


def caller_from(
    session: str | None,
    harness: str | None,
    agent: str | None,
    human: Author | None,
    since: float | None = None,
) -> Caller:
    return Caller(
        session=session or os.environ.get("CADRI_SESSION_ID") or None,
        harness=harness or os.environ.get("CADRI_HARNESS") or None,
        agent=normalize_agent(agent or os.environ.get("CADRI_AGENT")),
        human=human,
        since=since,
    )


def normalize_agent(agent: str | None) -> str:
    """Hook payloads name plugin agents with their namespace (`cadri:cadri-memory-keeper`)."""
    if not agent:
        return "main"
    return agent.rsplit(":", 1)[-1]
