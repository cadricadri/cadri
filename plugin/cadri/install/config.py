"""Read and write harness config files without disturbing anything Cadri does not own.

Cadri owns only hook entries whose command runs Cadri's `hook` subcommand, either through
the `cadri` shim or (in older releases) `cadri_cli.py`. Every other key and hook is read
and written back unchanged, so a re-install replaces Cadri's entries and an uninstall
removes only them.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import tempfile
from pathlib import Path

HARNESS_NAMES = {"claude_code": "Claude Code", "codex": "Codex", "cursor": "Cursor", "opencode": "OpenCode"}
CADRI_HOOK = re.compile(r"(cadri_cli\.py|/bin/cadri['\"]?)\s+hook\s")


class InstallError(Exception):
    pass


def is_cadri_command(command: object) -> bool:
    return isinstance(command, str) and bool(CADRI_HOOK.search(command))


def claude_dir(home: Path) -> Path:
    configured = os.environ.get("CLAUDE_CONFIG_DIR")
    return Path(configured) if configured else home / ".claude"


def codex_dir(home: Path) -> Path:
    configured = os.environ.get("CODEX_HOME")
    return Path(configured) if configured else home / ".codex"


def opencode_dir(home: Path) -> Path:
    configured = os.environ.get("XDG_CONFIG_HOME")
    return (Path(configured) if configured else home / ".config") / "opencode"


def read_json(path: Path) -> dict:
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        return {}
    except OSError as exc:
        raise InstallError(f"could not read {path}: {exc}") from exc
    if not raw.strip():
        return {}
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise InstallError(f"{path} is not valid JSON ({exc}); fix it and run cadri init again") from exc
    if not isinstance(data, dict):
        raise InstallError(f"{path} must hold a JSON object")
    return data


def write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}-")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        os.chmod(tmp, path.stat().st_mode & 0o777 if path.exists() else 0o644)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def write_json(path: Path, data: dict) -> None:
    write_text(path, json.dumps(data, indent=2) + "\n")


def remove_tree(path: Path) -> bool:
    if path.is_symlink() or path.is_file():
        path.unlink()
        return True
    if path.is_dir():
        shutil.rmtree(path)
        return True
    return False


def strip_groups(groups: object) -> list:
    """Claude/Codex hook groups with Cadri's commands removed; emptied groups are dropped."""
    kept = []
    for group in groups if isinstance(groups, list) else []:
        if not isinstance(group, dict) or not isinstance(group.get("hooks"), list):
            kept.append(group)
            continue
        hooks = [hook for hook in group["hooks"] if not (isinstance(hook, dict) and is_cadri_command(hook.get("command")))]
        if hooks:
            kept.append({**group, "hooks": hooks})
        elif not group["hooks"]:
            kept.append(group)
    return kept


def set_grouped_hooks(path: Path, groups: dict[str, list] | None) -> bool:
    """Replace Cadri's entries in a Claude-style hooks map (None removes them). Returns whether the file changed."""
    data = read_json(path)
    if not data and groups is None:
        return False
    hooks = data.get("hooks")
    if hooks is None:
        hooks = {}
    if not isinstance(hooks, dict):
        raise InstallError(f"{path}: \"hooks\" must be an object")
    before = json.dumps(data, sort_keys=True)
    for event in list(hooks):
        kept = strip_groups(hooks[event])
        if kept:
            hooks[event] = kept
        else:
            del hooks[event]
    for event, entries in (groups or {}).items():
        hooks.setdefault(event, []).extend(entries)
    if hooks:
        data["hooks"] = hooks
    else:
        data.pop("hooks", None)
    if json.dumps(data, sort_keys=True) == before:
        return False
    write_json(path, data)
    return True


def strip_cadri_hooks(path: Path) -> bool:
    return set_grouped_hooks(path, None)


def strip_cursor_hooks(path: Path) -> bool:
    """Remove Cadri's entries from a user-level Cursor hooks.json (earlier releases wrote there)."""
    data = read_json(path)
    hooks = data.get("hooks")
    if not isinstance(hooks, dict):
        return False
    before = json.dumps(data, sort_keys=True)
    for event in list(hooks):
        entries = hooks[event] if isinstance(hooks[event], list) else []
        kept = [entry for entry in entries if not (isinstance(entry, dict) and is_cadri_command(entry.get("command")))]
        if kept or not entries:
            hooks[event] = kept
        else:
            del hooks[event]
    if json.dumps(data, sort_keys=True) == before:
        return False
    if not hooks and set(data) <= {"version", "hooks"}:
        path.unlink(missing_ok=True)
    else:
        write_json(path, data)
    return True


def installed_harnesses(home: Path | None = None) -> list[str]:
    """Harnesses that look installed on this machine, whether or not they have sessions yet."""
    root = home or Path.home()
    found = []
    if claude_dir(root).is_dir() or shutil.which("claude"):
        found.append("claude_code")
    if (root / ".cursor").is_dir() or Path("/Applications/Cursor.app").is_dir() or shutil.which("cursor"):
        found.append("cursor")
    if codex_dir(root).is_dir() or shutil.which("codex"):
        found.append("codex")
    if opencode_dir(root).is_dir() or shutil.which("opencode"):
        found.append("opencode")
    return found
