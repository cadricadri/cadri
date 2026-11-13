"""Let agents edit the memory books and run `cadri memory` without a prompt for each.

Claude Code: an additional directory plus allow rules in ~/.claude/settings.json.
Codex: the memory directory as a writable root of the workspace-write sandbox.
OpenCode 1.x: allow rules for the memory directory and `cadri memory` in opencode.json.
Cursor needs nothing.
Uninstall removes exactly the entries install adds, and nothing else.
"""

from __future__ import annotations

import json
from pathlib import Path

from ..memory import digests
from ..memory.books import memory_root
from .config import InstallError, read_json, write_json, write_text

CODEX_TABLE = "[sandbox_workspace_write]"


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
