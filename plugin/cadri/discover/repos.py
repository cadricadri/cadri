"""Which git repository a seed window was working in.

Cursor and Claude store each session under a project directory whose name is the
absolute path with ``/`` written as ``-``. Codex and OpenCode record a working
directory in the session itself. A hyphen inside a directory name is ambiguous, so
the all-separators path wins when it exists, and a hyphenated name is used when
that path does not.
"""

from __future__ import annotations

import json
import sqlite3
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .types import SessionRef

# A root with at least this share of the window is the one to brief.
_MAJORITY_NUMERATOR = 3
_MAJORITY_DENOMINATOR = 5
_DIRECTORY_COLUMNS = ("directory", "cwd", "project_dir")
_CWD_KEYS = ("cwd", "workspaceRoot", "workspace_root", "directory")


@dataclass(frozen=True)
class RepoCount:
    path: str
    sessions: int


@dataclass(frozen=True)
class RepoChoice:
    brief: str | None
    ask: tuple[RepoCount, ...]
    seen: tuple[RepoCount, ...]
    reason: str


def encoded_project_name(ref: SessionRef) -> str | None:
    """The project-directory name Cursor or Claude used for this session file."""
    parts = ref.path.parts
    if ref.harness == "cursor":
        for index, part in enumerate(parts):
            if part == "agent-transcripts" and index > 0:
                return parts[index - 1]
    if ref.harness == "claude_code":
        for index, part in enumerate(parts[:-1]):
            if part == "projects" and index > 0 and parts[index - 1] == ".claude":
                return parts[index + 1]
    return None


def decode_project_path(encoded: str, *, root: Path = Path("/")) -> Path | None:
    """The directory `encoded` names, when one exists under `root`."""
    body = encoded[1:] if encoded.startswith("-") else encoded
    if not body or body.startswith("-"):
        return None
    matches = _consume(root, body)
    if not matches:
        return None
    naive = root.joinpath(*body.split("-"))
    if naive.is_dir():
        return naive
    return max(matches, key=lambda path: len(path.resolve().parts))


def git_toplevel(path: Path) -> Path | None:
    if not path.exists():
        return None
    start = path if path.is_dir() else path.parent
    result = subprocess.run(
        ["git", "-C", str(start), "rev-parse", "--show-toplevel"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return None
    text = result.stdout.strip()
    return Path(text).resolve() if text else None


def roots_for_sessions(refs: list[SessionRef], *, decode_root: Path | None = None) -> list[RepoCount]:
    """Git roots in `refs`, most sessions first. Sessions outside a git repo are absent."""
    root = decode_root or Path("/")
    opencode = _opencode_directories(refs)
    toplevels: dict[str, str | None] = {}
    counts: dict[str, int] = {}
    for ref in refs:
        hint = _session_directory(ref, decode_root=root, opencode=opencode)
        if hint is None:
            continue
        key = str(hint)
        if key not in toplevels:
            found = git_toplevel(hint)
            toplevels[key] = str(found) if found else None
        repo = toplevels[key]
        if repo is None:
            continue
        counts[repo] = counts.get(repo, 0) + 1
    ordered = sorted(counts.items(), key=lambda item: (-item[1], item[0]))
    return [RepoCount(path, count) for path, count in ordered]


def choose_repo(counts: list[RepoCount], cwd: Path | None) -> RepoChoice:
    """The repo to brief.

    The current directory wins when it sits inside one of the roots. Otherwise the
    root with at least three fifths of the sessions wins. Close counts are a
    question. A root with fewer than half the leader's sessions stays in `seen`.
    """
    ordered = tuple(sorted(counts, key=lambda item: (-item.sessions, item.path)))
    if not ordered:
        return RepoChoice(None, (), (), "none")
    if cwd is not None:
        here = cwd.resolve()
        for item in ordered:
            root = Path(item.path).resolve()
            if here == root or root in here.parents:
                return RepoChoice(item.path, (), ordered, "cwd")
    total = sum(item.sessions for item in ordered)
    top = ordered[0]
    if len(ordered) == 1 or top.sessions * _MAJORITY_DENOMINATOR >= total * _MAJORITY_NUMERATOR:
        return RepoChoice(top.path, (), ordered, "majority")
    leaders = tuple(item for item in ordered if item.sessions * 2 >= top.sessions)
    return RepoChoice(None, leaders, ordered, "ask")


def repo_payload(choice: RepoChoice) -> dict[str, object]:
    return {
        "brief": choice.brief,
        "ask": [item.path for item in choice.ask],
        "seen": [{"path": item.path, "sessions": item.sessions} for item in choice.seen],
        "reason": choice.reason,
    }


def format_repo_choice(choice: RepoChoice) -> str:
    if choice.reason == "none":
        return "Repos: none in this window. Write no repository briefing."
    lines = ["Repos in this window:"]
    lines.extend(f"  {item.path} ({item.sessions} sessions)" for item in choice.seen)
    if choice.reason == "ask":
        lines.append("Ask which repo to brief before you read. The counts are close.")
        return "\n".join(lines)
    why = (
        "This agent is running inside it."
        if choice.reason == "cwd"
        else "It has the most sessions in this window."
    )
    lines.append(f"Brief {choice.brief}. {why}")
    if any(item.path != choice.brief for item in choice.seen):
        lines.append("Leave the others for a later run from inside that repo.")
    return "\n".join(lines)


def _session_directory(
    ref: SessionRef,
    *,
    decode_root: Path,
    opencode: dict[tuple[str, str], str],
) -> Path | None:
    encoded = encoded_project_name(ref)
    if encoded:
        return decode_project_path(encoded, root=decode_root)
    if ref.harness == "codex":
        return _cwd_from_jsonl(ref.path)
    if ref.harness == "opencode":
        directory = opencode.get((str(ref.path), ref.thread_id))
        return Path(directory) if directory else None
    return None


def _consume(directory: Path, rest: str) -> list[Path]:
    if not rest:
        return [directory]
    if not directory.is_dir():
        return []
    found: list[Path] = []
    try:
        children = list(directory.iterdir())
    except OSError:
        return []
    for child in children:
        name = child.name
        if name in {".", ".."} or not child.is_dir():
            continue
        if rest == name:
            found.append(child)
        elif rest.startswith(name + "-"):
            found.extend(_consume(child, rest[len(name) + 1 :]))
    return found


def _cwd_from_jsonl(path: Path, *, max_lines: int = 40) -> Path | None:
    try:
        handle = path.open(encoding="utf-8", errors="replace")
    except OSError:
        return None
    with handle:
        for index, line in enumerate(handle):
            if index >= max_lines:
                break
            if len(line) > 262144:
                continue
            if not any(token in line for token in ('"cwd"', '"directory"', '"workspaceRoot"')):
                continue
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            found = _find_cwd(value)
            if found:
                return Path(found)
    return None


def _find_cwd(value: object, depth: int = 0) -> str | None:
    if depth > 6:
        return None
    if isinstance(value, dict):
        for key in _CWD_KEYS:
            found = value.get(key)
            if isinstance(found, str) and found.startswith("/"):
                return found
        for item in value.values():
            found = _find_cwd(item, depth + 1)
            if found:
                return found
    elif isinstance(value, list):
        for item in value[:12]:
            found = _find_cwd(item, depth + 1)
            if found:
                return found
    return None


def _opencode_directories(refs: list[SessionRef]) -> dict[tuple[str, str], str]:
    by_db: dict[Path, list[str]] = {}
    for ref in refs:
        if ref.harness == "opencode":
            by_db.setdefault(ref.path, []).append(ref.thread_id)
    found: dict[tuple[str, str], str] = {}
    for db_path in by_db:
        if not db_path.is_file():
            continue
        try:
            connection = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
        except sqlite3.Error:
            continue
        try:
            tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            for table in ("session_v2", "session"):
                if table not in tables:
                    continue
                columns = {row[1] for row in connection.execute(f"PRAGMA table_info({table})")}
                column = next((name for name in _DIRECTORY_COLUMNS if name in columns), None)
                if column is None:
                    continue
                query = f"SELECT id, {column} FROM {table}"
                for session_id, directory in connection.execute(query):
                    if isinstance(session_id, str) and isinstance(directory, str) and directory.startswith("/"):
                        found[(str(db_path), session_id)] = directory
                break
        except sqlite3.Error:
            continue
        finally:
            connection.close()
    return found
