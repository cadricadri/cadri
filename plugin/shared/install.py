"""Install and remove Cadri's capture hooks and skills in each harness's user config.

Cadri owns only the hook entries whose command runs `cadri_cli.py ... hook`. Every
other key and hook in a harness config is read and written back unchanged, so a
re-install replaces Cadri's entries and an uninstall removes only them.
"""

from __future__ import annotations

import json
import os
import shlex
import shutil
import sys
import tempfile
from pathlib import Path

PLUGIN_ROOT = Path(__file__).resolve().parent.parent
CLI = PLUGIN_ROOT / "bin" / "cadri_cli.py"
SKILLS = PLUGIN_ROOT / "skills"
OPENCODE_TEMPLATE = PLUGIN_ROOT / "harnesses" / "opencode" / "cadri.ts"
OPENCODE_MARKER = "cadri-managed-opencode-plugin"
HOOK_TIMEOUT_SECONDS = 30

CLAUDE_EVENTS = ("SessionStart", "Stop", "PreCompact", "SessionEnd")
CODEX_EVENTS = ("SessionStart", "Stop")
CURSOR_EVENTS = ("sessionStart", "stop", "preCompact", "sessionEnd")

# Paths earlier releases wrote that no harness ever read.
LEGACY_PATHS = (
    ".claude/plugins/cadri",
    ".cursor/cadri",
    ".cursor/cadri-mcp.json",
    ".codex/cadri",
    ".config/opencode/plugins/cadri",
    ".claude/skills/cadri-capture",
    ".agents/skills/cadri-capture",
)


class InstallError(Exception):
    pass


def hook_command(harness: str) -> str:
    return f"{shlex.quote(sys.executable)} {shlex.quote(str(CLI))} hook {harness}"


def is_cadri_command(command: object) -> bool:
    return isinstance(command, str) and "cadri_cli.py" in command and " hook " in command


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
        if path.exists():
            os.chmod(tmp, path.stat().st_mode & 0o777)
        else:
            os.chmod(tmp, 0o644)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def write_json(path: Path, data: dict) -> None:
    write_text(path, json.dumps(data, indent=2) + "\n")


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


def set_grouped_hooks(path: Path, events: tuple[str, ...], command: str | None, extra: dict | None = None) -> bool:
    """Replace Cadri's entries in a Claude-style hooks map. Returns whether the file changed."""
    data = read_json(path)
    if not data and command is None:
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
    if command is not None:
        for event in events:
            hook = {"type": "command", "command": command, "timeout": HOOK_TIMEOUT_SECONDS, **(extra or {})}
            hooks.setdefault(event, []).append({"hooks": [hook]})
    if hooks:
        data["hooks"] = hooks
    else:
        data.pop("hooks", None)
    if json.dumps(data, sort_keys=True) == before:
        return False
    write_json(path, data)
    return True


def set_cursor_hooks(path: Path, command: str | None) -> bool:
    data = read_json(path)
    if not data and command is None:
        return False
    hooks = data.get("hooks")
    if hooks is None:
        hooks = {}
    if not isinstance(hooks, dict):
        raise InstallError(f"{path}: \"hooks\" must be an object")
    before = json.dumps(data, sort_keys=True)
    for event in list(hooks):
        entries = hooks[event] if isinstance(hooks[event], list) else []
        kept = [entry for entry in entries if not (isinstance(entry, dict) and is_cadri_command(entry.get("command")))]
        if kept or not entries:
            hooks[event] = kept
        else:
            del hooks[event]
    if command is not None:
        for event in CURSOR_EVENTS:
            hooks.setdefault(event, []).append({"command": command, "timeout": HOOK_TIMEOUT_SECONDS})
    if not hooks and set(data) <= {"version", "hooks"}:
        path.unlink(missing_ok=True)
        return True
    data["hooks"] = hooks
    if not isinstance(data.get("version"), int) or data["version"] < 1:
        data["version"] = 1
    if json.dumps(data, sort_keys=True) == before:
        return False
    write_json(path, data)
    return True


def opencode_plugin(home: Path) -> Path:
    return opencode_dir(home) / "plugins" / "cadri.ts"


def set_opencode_plugin(home: Path, install: bool) -> bool:
    path = opencode_plugin(home)
    if path.exists() and OPENCODE_MARKER not in path.read_text(encoding="utf-8", errors="replace"):
        raise InstallError(f"{path} exists and was not written by Cadri; move it aside and run cadri init again")
    if not install:
        if path.exists():
            path.unlink()
            return True
        return False
    command = [sys.executable, str(CLI), "hook", "opencode"]
    text = OPENCODE_TEMPLATE.read_text(encoding="utf-8").replace("__CADRI_COMMAND__", json.dumps(command))
    write_text(path, text)
    return True


def skill_dirs(home: Path, harnesses: list[str]) -> list[Path]:
    dirs = [home / ".agents" / "skills"]
    if "claude_code" in harnesses:
        dirs.append(claude_dir(home) / "skills")
    return dirs


def install_skills(home: Path, harnesses: list[str]) -> list[Path]:
    installed = []
    for root in skill_dirs(home, harnesses):
        for skill in sorted(SKILLS.iterdir()):
            if not (skill / "SKILL.md").is_file():
                continue
            target = root / skill.name
            if target.is_symlink() or target.is_file():
                target.unlink()
            elif target.is_dir():
                shutil.rmtree(target)
            shutil.copytree(skill, target)
        installed.append(root)
    return installed


def remove_skills(home: Path) -> None:
    for root in skill_dirs(home, ["claude_code"]):
        for skill in SKILLS.iterdir():
            target = root / skill.name
            if target.is_dir() and not target.is_symlink():
                shutil.rmtree(target)


def remove_legacy(home: Path) -> None:
    for relative in LEGACY_PATHS:
        path = home / relative
        if path.is_symlink() or path.is_file():
            path.unlink()
        elif path.is_dir():
            shutil.rmtree(path)


def install_harness(name: str, home: Path | None = None) -> str:
    root = home or Path.home()
    if name == "claude_code":
        path = claude_dir(root) / "settings.json"
        set_grouped_hooks(path, CLAUDE_EVENTS, hook_command("claude_code"))
        return f"Claude Code: hooks in {path}"
    if name == "cursor":
        path = root / ".cursor" / "hooks.json"
        set_cursor_hooks(path, hook_command("cursor"))
        return f"Cursor: hooks in {path}"
    if name == "codex":
        path = codex_dir(root) / "hooks.json"
        set_grouped_hooks(path, CODEX_EVENTS, hook_command("codex"))
        return f"Codex: hooks in {path} (open /hooks in Codex once and trust them)"
    if name == "opencode":
        set_opencode_plugin(root, install=True)
        return f"OpenCode: plugin at {opencode_plugin(root)}"
    raise InstallError(f"unknown harness {name!r}")


def uninstall_all(home: Path | None = None) -> list[str]:
    root = home or Path.home()
    removed = []
    if set_grouped_hooks(claude_dir(root) / "settings.json", CLAUDE_EVENTS, None):
        removed.append("Claude Code hooks")
    if set_cursor_hooks(root / ".cursor" / "hooks.json", None):
        removed.append("Cursor hooks")
    if set_grouped_hooks(codex_dir(root) / "hooks.json", CODEX_EVENTS, None):
        removed.append("Codex hooks")
    if set_opencode_plugin(root, install=False):
        removed.append("OpenCode plugin")
    remove_skills(root)
    remove_legacy(root)
    return removed


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
