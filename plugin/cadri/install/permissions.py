"""Let agents edit the memory books and run `cadri memory` without a prompt for each.

Claude Code: an additional directory plus allow rules in ~/.claude/settings.json.
Codex: the memory directory as a writable root of the workspace-write sandbox.
OpenCode 1.x: allow rules for the memory directory and `cadri memory` in opencode.json.
Cursor: `cadri memory` on the command allowlist in ~/.cursor/permissions.json, so those
commands run without an approval card, including from a background agent. `~/.cadri` is
also an extra read-write path in ~/.cursor/sandbox.json, so a sandboxed shell can write
the books. The editor still asks before it creates a file outside the workspace; the
memory skills publish with `cadri memory` instead.
Uninstall removes exactly the entries install adds, and nothing else.
"""

from __future__ import annotations

import json
import sqlite3
import sys
from pathlib import Path

from ..memory import digests
from ..memory.books import memory_root
from ..state import cadri_home
from .config import InstallError, read_json, write_json, write_text

CODEX_TABLE = "[sandbox_workspace_write]"
CURSOR_COMMAND = "cadri memory"
# composerState.yoloCommandAllowlist lives in this row. A terminalAllowlist key in
# permissions.json replaces that in-app list, so install copies it across instead of
# starting a new list that would drop it.
CURSOR_STATE_KEY = (
    "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl"
    ".persistentStorage.applicationUser"
)


def claude_path(path: Path) -> str:
    """A path in Claude Code's permission-rule syntax: `~/…` under home, `//abs` otherwise."""
    try:
        return "~/" + path.relative_to(Path.home()).as_posix()
    except ValueError:
        return "/" + path.as_posix()


def claude_entries() -> tuple[str, list[str]]:
    """Claude Code's `Edit(…)` rules cover every file-editing tool; it ignores `Write(…)`."""
    directory = claude_path(memory_root())
    reflections = claude_path(digests.root())
    return directory, [
        f"Edit({directory}/**)",
        f"Read({directory}/**)",
        f"Read({reflections}/**)",
        "Bash(cadri memory:*)",
    ]


def retired_claude_entries() -> list[str]:
    return [f"Write({claude_path(memory_root())}/**)"]


def set_claude(settings: Path, install: bool) -> bool:
    data = read_json(settings)
    if not data and not install:
        return False
    permissions = data.get("permissions", {})
    if not isinstance(permissions, dict):
        raise InstallError(f"{settings}: \"permissions\" must be an object")
    directory, rules = claude_entries()
    retired = retired_claude_entries()
    before = repr(permissions)
    for key, wanted in (("additionalDirectories", [directory]), ("allow", rules)):
        current = permissions.get(key, [])
        if not isinstance(current, list):
            raise InstallError(f"{settings}: \"permissions.{key}\" must be a list")
        kept = [entry for entry in current if entry not in wanted and entry not in retired]
        permissions[key] = kept + wanted if install else kept
        if not permissions[key]:
            del permissions[key]
    if repr(permissions) == before:
        return False
    if permissions:
        data["permissions"] = permissions
    else:
        data.pop("permissions", None)
    write_json(settings, data)
    return True


def opencode_rules() -> list[tuple[str, str, str]]:
    """The (permission key, pattern, action) rules Cadri adds for OpenCode 1.x; 2.x allows these by default.

    The memory keeper reads its digest from the reflection queue before touching memory.
    """
    return [
        ("external_directory", f"{memory_root().as_posix()}/*", "allow"),
        ("external_directory", f"{digests.root().as_posix()}/*", "allow"),
        ("bash", "cadri memory *", "allow"),
    ]


def set_opencode(directory: Path, install: bool) -> str:
    """Add or remove Cadri's rules in opencode.json. Returns a note for the user, or ""."""
    rules = opencode_rules()
    path = directory / "opencode.json"
    if (directory / "opencode.jsonc").exists():
        path = directory / "opencode.jsonc"
        try:
            data = json.loads(path.read_text(encoding="utf-8") or "{}")
        except (OSError, json.JSONDecodeError):
            # Comments or trailing commas: leave the file to its owner.
            if not install:
                return ""
            wanted = ", ".join(f'"{key}": {{"{pattern}": "{action}"}}' for key, pattern, action in rules)
            return f"; add to \"permission\" in {path} so agents can use memory without asking: {wanted}"
        if not isinstance(data, dict):
            raise InstallError(f"{path} must hold a JSON object")
    else:
        data = read_json(path)
    if not data and not install:
        return ""
    permission = data.get("permission", {})
    if isinstance(permission, str):
        if permission != "allow" and install:
            return f"; your OpenCode permission is \"{permission}\" for everything, so agents will ask before using memory"
        return ""
    if not isinstance(permission, dict):
        raise InstallError(f"{path}: \"permission\" must be an object")
    before = json.dumps(permission, sort_keys=True)
    for key, pattern, action in rules:
        current = permission.get(key)
        if install:
            if current == "allow":
                continue
            entries = dict(current) if isinstance(current, dict) else ({"*": current} if isinstance(current, str) else {})
            entries.pop(pattern, None)
            entries[pattern] = action
            permission[key] = entries
        elif isinstance(current, dict) and current.get(pattern) == action:
            del current[pattern]
            if not current:
                del permission[key]
            elif list(current) == ["*"]:
                permission[key] = current["*"]
    if json.dumps(permission, sort_keys=True) == before:
        return ""
    if permission:
        data["permission"] = permission
    else:
        data.pop("permission", None)
    write_json(path, {"$schema": "https://opencode.ai/config.json", **data})
    return ""


def codex_block() -> str:
    return f'{CODEX_TABLE}\n# cadri-managed: lets agents edit the Cadri memory books\nwritable_roots = ["{memory_root().as_posix()}"]\n'


def set_codex(config: Path, install: bool) -> str:
    """Add or remove Cadri's writable root. Returns a note for the user, or ""."""
    try:
        text = config.read_text(encoding="utf-8")
    except FileNotFoundError:
        text = ""
    block = codex_block()
    if not install:
        if block in text:
            write_text(config, text.replace("\n" + block, "").replace(block, ""))
        return ""
    if block in text:
        return ""
    if CODEX_TABLE in text:
        return (
            f"; add {memory_root()} to writable_roots under {CODEX_TABLE} in {config} "
            "so agents can edit memory without asking"
        )
    write_text(config, text + ("\n" if text and not text.endswith("\n") else "") + ("\n" if text else "") + block)
    return ""


def cursor_state_db(home: Path) -> Path:
    """Where Cursor stores the in-app command allowlist for this home directory."""
    if sys.platform == "darwin":
        base = home / "Library" / "Application Support"
    elif sys.platform == "win32":
        base = home / "AppData" / "Roaming"
    else:
        base = home / ".config"
    return base / "Cursor" / "User" / "globalStorage" / "state.vscdb"


def cursor_settings_allowlist(home: Path) -> list[str] | None:
    """Commands allowed in Cursor Settings.

    [] when Cursor has no saved list. None when the database is there but cannot be
    read, so install must not replace it with a shorter list.
    """
    path = cursor_state_db(home)
    if not path.is_file():
        return []
    try:
        con = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
        try:
            row = con.execute("SELECT value FROM ItemTable WHERE key = ?", (CURSOR_STATE_KEY,)).fetchone()
        finally:
            con.close()
    except (OSError, sqlite3.Error):
        return None
    if row is None:
        return []
    raw = row[0]
    if isinstance(raw, bytes):
        try:
            raw = raw.decode()
        except UnicodeDecodeError:
            return None
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    state = data.get("composerState") if isinstance(data, dict) else None
    if not isinstance(state, dict):
        return []
    allow = state.get("yoloCommandAllowlist", [])
    if allow is None:
        return []
    if not isinstance(allow, list) or not all(isinstance(item, str) for item in allow):
        return None
    return allow


def _load_cursor_json(path: Path) -> tuple[dict | None, bool]:
    """Return (object, unreadable). Missing or empty is ({}, False)."""
    try:
        raw = path.read_text(encoding="utf-8") if path.exists() else ""
    except OSError as exc:
        raise InstallError(f"could not read {path}: {exc}") from exc
    if not raw.strip():
        return {}, False
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None, True
    if not isinstance(data, dict):
        raise InstallError(f"{path} must hold a JSON object")
    return data, False


def _string_list(path: Path, data: dict, key: str) -> list[str] | None:
    values = data.get(key, [])
    if values is None:
        return []
    if not isinstance(values, list) or not all(isinstance(item, str) for item in values):
        raise InstallError(f'{path}: "{key}" must be a list of strings')
    return values


def _command_allowed(entries: list[str]) -> bool:
    """True when an entry already matches `cadri memory` by Cursor's prefix rule."""
    return any(CURSOR_COMMAND == entry or CURSOR_COMMAND.startswith(entry + " ") for entry in entries)


def _with_command(entries: list[str]) -> list[str]:
    kept: list[str] = []
    seen: set[str] = set()
    for entry in entries:
        if not entry or entry == CURSOR_COMMAND or entry in seen:
            continue
        seen.add(entry)
        kept.append(entry)
    kept.append(CURSOR_COMMAND)
    return kept


def set_cursor_allowlist(home: Path, install: bool) -> str:
    """Allow `cadri memory` without a card. Returns a note, or ""."""
    path = home / ".cursor" / "permissions.json"
    data, unreadable = _load_cursor_json(path)
    manual = (
        f"; add `{CURSOR_COMMAND}` to terminalAllowlist in {path}, so memory commands run "
        "without asking. The file was left unchanged so commands you already allowed are not dropped"
    )
    if unreadable:
        return manual if install else ""
    assert data is not None
    if not data and not install:
        return ""
    current = _string_list(path, data, "terminalAllowlist")
    assert current is not None
    if install:
        if "terminalAllowlist" in data:
            if _command_allowed(current):
                return ""
            merged = current
        else:
            settings = cursor_settings_allowlist(home)
            if settings is None:
                return (
                    f"; add `{CURSOR_COMMAND}` to the command allowlist in Cursor Settings → Agents, "
                    "so memory commands run without asking. Cursor's saved allowlist could not be "
                    "read, and writing a new one would drop it"
                )
            if _command_allowed(settings):
                return ""
            merged = settings
        data["terminalAllowlist"] = _with_command(merged)
        write_json(path, data)
        return (
            f"; `{CURSOR_COMMAND}` is on the command allowlist in {path}, so those commands run "
            "without asking, including in the background. That file replaces the command allowlist "
            "in Cursor Settings"
        )
    if CURSOR_COMMAND not in current:
        return ""
    kept = [entry for entry in current if entry != CURSOR_COMMAND]
    settings = cursor_settings_allowlist(home)
    # An empty allowlist does not fall back to Cursor Settings. Drop the key when the
    # file only repeated commands Settings already has, so those stay editable there.
    if kept and (settings is None or not set(kept) <= set(settings)):
        data["terminalAllowlist"] = kept
    else:
        data.pop("terminalAllowlist", None)
    if data:
        write_json(path, data)
    elif path.exists():
        path.unlink()
    return ""


def set_cursor_sandbox(home: Path, install: bool) -> str:
    """Let sandboxed shell commands write under ~/.cadri. Returns a note, or ""."""
    path = home / ".cursor" / "sandbox.json"
    root = cadri_home().as_posix()
    try:
        raw = path.read_text(encoding="utf-8") if path.exists() else ""
    except OSError as exc:
        raise InstallError(f"could not read {path}: {exc}") from exc
    if not raw.strip():
        data: dict = {}
    else:
        try:
            data = json.loads(raw)
        except json.JSONDecodeError:
            if install:
                return (
                    f"; {path} is not plain JSON, so add {root} to additionalReadwritePaths "
                    "so shell commands can write memory without leaving the sandbox"
                )
            return ""
    if not isinstance(data, dict):
        raise InstallError(f"{path} must hold a JSON object")
    paths = data.get("additionalReadwritePaths", [])
    if paths is None:
        paths = []
    if not isinstance(paths, list) or not all(isinstance(item, str) for item in paths):
        raise InstallError(f"{path}: \"additionalReadwritePaths\" must be a list of strings")
    if install:
        if root in paths:
            return ""
        data["additionalReadwritePaths"] = [*paths, root]
        write_json(path, data)
        return ""
    if root not in paths:
        return ""
    kept = [item for item in paths if item != root]
    if kept:
        data["additionalReadwritePaths"] = kept
    else:
        data.pop("additionalReadwritePaths", None)
    if data:
        write_json(path, data)
    elif path.exists():
        path.unlink()
    return ""


def set_cursor(home: Path, install: bool) -> str:
    """Allow `cadri memory` and sandboxed writes under ~/.cadri. Returns notes, or ""."""
    return set_cursor_sandbox(home, install) + set_cursor_allowlist(home, install)
