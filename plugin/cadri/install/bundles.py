"""Build each harness's Cadri bundle and install it where that harness loads it.

    Claude Code  plugin in a local marketplace (~/.cadri/plugins/claude), installed with
                 the `claude plugin` CLI; without the CLI, hooks in settings.json and
                 copies of the skills and keeper agent instead
    Codex        hooks in ~/.codex/hooks.json, skills in ~/.agents/skills, the keeper
                 role in ~/.codex/agents
    Cursor       local plugin in ~/.cursor/plugins/local/cadri
    OpenCode     ~/.config/opencode/plugins/cadri.ts and agents/cadri-memory-keeper.md

Every hook command runs the `cadri` shim by absolute path with an explicit timeout.
There are no SessionEnd hooks: harnesses rarely call them.
"""

from __future__ import annotations

import json
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from ..state import cadri_home
from . import permissions
from .config import (
    HARNESS_NAMES,
    InstallError,
    claude_dir,
    codex_dir,
    opencode_dir,
    remove_tree,
    set_grouped_hooks,
    strip_cadri_hooks,
    strip_cursor_hooks,
    write_json,
    write_text,
)

PLUGIN_ROOT = Path(__file__).resolve().parents[2]
SKILLS = PLUGIN_ROOT / "skills"
HARNESS_FILES = PLUGIN_ROOT / "harnesses"
KEEPER_SOURCE = PLUGIN_ROOT / "agents" / "cadri-memory-keeper.md"
MAINTAIN = SKILLS / "cadri-memory" / "references" / "maintain.md"
SHIPPED_SKILLS = (
    "cadri-memory",
    "cadri-memory-repair",
    "cadri-install",
    "cadri-memory-seed",
    "cadri-memory-join",
    "cadri-initializing-memory",
)
KEEPER = "cadri-memory-keeper"
KEEPER_DESCRIPTION = (
    "Cadri memory keeper. Launch in the background when Cadri asks: reflects on recent turns from a digest file, "
    "updates the team and personal memory books, and pushes. Also repairs memory push conflicts."
)
MARKER = "cadri-managed"
MARKETPLACE = "cadri"
PLUGIN_ID = "cadri@cadri"


@dataclass(frozen=True)
class Hook:
    event: str  # the harness's event name
    cadri_event: str
    timeout: int
    matcher: str | None = None
    extra: tuple[tuple[str, object], ...] = ()


CLAUDE_HOOKS = (
    Hook("SessionStart", "session-start", 15),
    Hook("UserPromptSubmit", "prompt", 10),
    Hook("PostToolUse", "edit", 10, "Write|Edit|MultiEdit|NotebookEdit", (("async", True),)),
    Hook("Stop", "stop", 15),
    Hook("PreCompact", "pre-compact", 15),
)
CODEX_HOOKS = (
    Hook("SessionStart", "session-start", 15, extra=(("additionalContextLimit", 4000),)),
    Hook("UserPromptSubmit", "prompt", 10, extra=(("additionalContextLimit", 4000),)),
    Hook("PostToolUse", "edit", 10, "apply_patch", (("async", True),)),
    Hook("Stop", "stop", 15),
    Hook("PreCompact", "pre-compact", 15),
)
CURSOR_HOOKS = (
    Hook("sessionStart", "session-start", 15),
    Hook("beforeSubmitPrompt", "prompt", 10),
    Hook("postToolUse", "tool-after", 5),
    Hook("afterFileEdit", "edit", 5),
    Hook("afterAgentResponse", "response", 5),
    Hook("stop", "stop", 15),
    Hook("preCompact", "pre-compact", 15),
)


# --- the shim -----------------------------------------------------------------------


def shim_path() -> Path:
    return cadri_home() / "bin" / "cadri"


def write_shim() -> Path:
    """Point ~/.cadri/bin/cadri at the Python and checkout running this install."""
    path = shim_path()
    script = f'#!/bin/sh\nexec {shlex.quote(sys.executable)} {shlex.quote(str(PLUGIN_ROOT / "bin" / "cadri_cli.py"))} "$@"\n'
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_text(encoding="utf-8") != script:
        write_text(path, script)
        path.chmod(0o755)
    return path


def command(cli: str, hook: Hook, harness: str) -> str:
    return f"{cli} hook {hook.cadri_event} --harness {harness}"


def default_cli() -> str:
    return shlex.quote(str(shim_path()))


# --- generated files ----------------------------------------------------------------


def grouped_hooks(hooks: tuple[Hook, ...], cli: str, harness: str) -> dict:
    """Claude Code and Codex: event → [{matcher?, hooks: [handler]}]."""
    result: dict[str, list] = {}
    for hook in hooks:
        handler = {"type": "command", "command": command(cli, hook, harness), "timeout": hook.timeout, **dict(hook.extra)}
        group: dict = {"hooks": [handler]}
        if hook.matcher:
            group = {"matcher": hook.matcher, **group}
        result.setdefault(hook.event, []).append(group)
    return result


def cursor_hooks(cli: str) -> dict:
    return {
        "version": 1,
        "hooks": {hook.event: [{"command": command(cli, hook, "cursor"), "timeout": hook.timeout}] for hook in CURSOR_HOOKS},
    }


def keeper_body(repair_skill: str = "cadri-memory-repair") -> str:
    body = KEEPER_SOURCE.read_text(encoding="utf-8").strip()
    maintain = MAINTAIN.read_text(encoding="utf-8").strip()
    if repair_skill != "cadri-memory-repair":
        body = body.replace("`cadri-memory-repair`", f"`{repair_skill}`")
    return f"{body}\n\n---\n\n{maintain}\n"


def frontmatter(fields: dict[str, object], managed: bool = False) -> str:
    lines = ["---"] + ([f"# {MARKER}: written by `cadri init`, removed by `cadri uninstall`."] if managed else [])
    for key, value in fields.items():
        if isinstance(value, bool):
            lines.append(f"{key}: {'true' if value else 'false'}")
        elif isinstance(value, dict):
            lines.append(f"{key}:")
            for name, setting in value.items():
                if isinstance(setting, dict):
                    lines.append(f"  {name}:")
                    lines += [f"    {json.dumps(k)}: {v}" for k, v in setting.items()]
                else:
                    lines.append(f"  {name}: {setting}")
        elif isinstance(value, list):
            lines.append(f"{key}:")
            for item in value:
                if isinstance(item, dict):
                    first, *rest = item.items()
                    lines.append(f"  - {first[0]}: {json.dumps(first[1])}")
                    lines += [f"    {k}: {json.dumps(v)}" for k, v in rest]
                else:
                    lines.append(f"  - {item}")
        else:
            lines.append(f"{key}: {json.dumps(value) if any(c in str(value) for c in ':#') else value}")
    return "\n".join(lines + ["---", ""])


def keeper_markdown(harness: str, in_plugin: bool = True) -> str:
    if harness == "claude":
        fields: dict[str, object] = {"name": KEEPER, "description": KEEPER_DESCRIPTION, "model": "sonnet", "background": True}
        if in_plugin:
            return frontmatter(fields) + "\n" + keeper_body("cadri:cadri-memory-repair")
        return frontmatter(fields, managed=True) + "\n" + keeper_body()
    if harness == "cursor":
        return frontmatter({"name": KEEPER, "description": KEEPER_DESCRIPTION, "is_background": True}) + "\n" + keeper_body()
    if harness == "opencode":
        memory = "~/.cadri/memory/**"
        fields = {
            "description": KEEPER_DESCRIPTION,
            "mode": "all",
            # OpenCode 1.x reads `permission` (its memory directory rule is global, in
            # opencode.json); 2.x reads `permissions` and ignores the other.
            "permission": {"edit": "allow", "bash": {"cadri memory *": "allow"}},
            "permissions": [
                {"action": "edit", "resource": memory, "effect": "allow"},
                {"action": "read", "resource": memory, "effect": "allow"},
                {"action": "external_directory", "resource": memory, "effect": "allow"},
                {"action": "shell", "resource": "cadri memory *", "effect": "allow"},
            ],
        }
        return frontmatter(fields, managed=True) + "\n" + keeper_body()
    raise InstallError(f"no Markdown keeper for {harness}")


def keeper_toml() -> str:
    body = keeper_body()
    if "'''" in body:
        raise InstallError("the keeper instructions contain ''' and cannot be written as a TOML literal string")
    return (
        f"# {MARKER}: written by `cadri init`, removed by `cadri uninstall`.\n"
        f'name = "{KEEPER}"\n'
        f"description = {json.dumps(KEEPER_DESCRIPTION)}\n"
        f"developer_instructions = '''\n{body}'''\n"
    )


def copy_skills(dest: Path) -> list[Path]:
    copied = []
    for name in SHIPPED_SKILLS:
        source = SKILLS / name
        if not (source / "SKILL.md").is_file():
            continue
        target = dest / name
        if target.is_symlink() or target.is_file():
            target.unlink()
        elif target.is_dir():
            shutil.rmtree(target)
        shutil.copytree(source, target, ignore=shutil.ignore_patterns("__pycache__", ".DS_Store"))
        copied.append(target)
    return copied


def remove_skill_copies(root: Path) -> None:
    """Remove skill directories this install copies into a harness skills root."""
    for name in SHIPPED_SKILLS:
        target = root / name
        if (target / "SKILL.md").is_file() and not target.is_symlink():
            remove_tree(target)


def fresh(path: Path) -> Path:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    return path


def build(harness: str, dest: Path, cli: str | None = None, installed_at: str | None = None) -> Path:
    """Write `harness`'s bundle into `dest` (replacing it). Returns the plugin root.

    `installed_at` is where the bundle will be read from at runtime, when that is not `dest`.
    """
    cli = cli or default_cli()
    fresh(dest)
    if harness == "claude":
        root = dest / "cadri"
        shutil.copytree(HARNESS_FILES / "claude", root)
        write_json(root / "hooks" / "hooks.json", {"hooks": grouped_hooks(CLAUDE_HOOKS, cli, "claude")})
        write_text(root / "agents" / f"{KEEPER}.md", keeper_markdown("claude"))
        copy_skills(root / "skills")
        manifest = json.loads((root / ".claude-plugin" / "plugin.json").read_text(encoding="utf-8"))
        write_json(
            dest / ".claude-plugin" / "marketplace.json",
            {
                "name": MARKETPLACE,
                "owner": {"name": "Cadri"},
                "description": "The Cadri plugin, installed by `cadri init`.",
                "plugins": [{"name": "cadri", "source": "./cadri", "description": manifest["description"], "version": manifest["version"]}],
            },
        )
        return root
    if harness == "cursor":
        shutil.copytree(HARNESS_FILES / "cursor", dest, dirs_exist_ok=True)
        write_json(dest / "hooks" / "hooks.json", cursor_hooks(cli))
        write_text(dest / "agents" / f"{KEEPER}.md", keeper_markdown("cursor"))
        copy_skills(dest / "skills")
        return dest
    if harness == "codex":
        write_json(dest / "hooks.json", {"hooks": grouped_hooks(CODEX_HOOKS, cli, "codex")})
        write_text(dest / "agents" / f"{KEEPER}.toml", keeper_toml())
        copy_skills(dest / "skills")
        return dest
    if harness == "opencode":
        write_text(dest / "cadri.ts", opencode_plugin(cli, f"{installed_at or dest}/skills"))
        write_text(dest / "agents" / f"{KEEPER}.md", keeper_markdown("opencode"))
        copy_skills(dest / "skills")
        return dest
    raise InstallError(f"unknown harness {harness!r}")


def opencode_plugin(cli: str, skills_dir: str) -> str:
    argv = shlex.split(cli)
    template = (HARNESS_FILES / "opencode" / "cadri.ts").read_text(encoding="utf-8")
    return template.replace("__CADRI_COMMAND__", json.dumps(argv)).replace("__CADRI_SKILLS__", json.dumps(skills_dir))


# --- install and uninstall ------------------------------------------------------------


def bundle_dir(harness: str) -> Path:
    return cadri_home() / "plugins" / harness


def run_cli(*argv: str) -> subprocess.CompletedProcess | None:
    try:
        return subprocess.run(list(argv), capture_output=True, text=True, timeout=120, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return None


def ok(result: subprocess.CompletedProcess | None) -> bool:
    return result is not None and result.returncode == 0


def install_claude(home: Path) -> str:
    marketplace = bundle_dir("claude")
    build("claude", marketplace)
    settings = claude_dir(home) / "settings.json"
    remove_claude_fallback(home)
    permissions.set_claude(settings, install=True)
    claude = shutil.which("claude")
    if claude:
        if not ok(run_cli(claude, "plugin", "marketplace", "add", str(marketplace))):
            run_cli(claude, "plugin", "marketplace", "update", MARKETPLACE)
        installed = ok(run_cli(claude, "plugin", "install", PLUGIN_ID, "--scope", "user"))
        if not installed:
            installed = ok(run_cli(claude, "plugin", "update", PLUGIN_ID, "--scope", "user"))
        if installed:
            return "Claude Code: plugin cadri installed (restart open sessions to load it)"
    install_claude_fallback(home)
    return f"Claude Code: hooks in {settings} (install the `claude` CLI and re-run init for the plugin)"


def install_claude_fallback(home: Path) -> None:
    base = claude_dir(home)
    set_grouped_hooks(base / "settings.json", grouped_hooks(CLAUDE_HOOKS, default_cli(), "claude"))
    copy_skills(base / "skills")
    write_text(base / "agents" / f"{KEEPER}.md", keeper_markdown("claude", in_plugin=False))


def remove_claude_fallback(home: Path) -> bool:
    """Remove hooks, skill copies, and the keeper written outside the plugin."""
    base = claude_dir(home)
    removed = strip_cadri_hooks(base / "settings.json")
    remove_skill_copies(base / "skills")
    agent = base / "agents" / f"{KEEPER}.md"
    if agent.is_file() and MARKER in agent.read_text(encoding="utf-8", errors="replace"):
        agent.unlink()
        removed = True
    return removed


def uninstall_claude(home: Path) -> bool:
    claude = shutil.which("claude")
    removed = False
    if claude:
        removed = ok(run_cli(claude, "plugin", "uninstall", PLUGIN_ID, "--scope", "user"))
        run_cli(claude, "plugin", "marketplace", "remove", MARKETPLACE)
    removed = remove_claude_fallback(home) or removed
    permissions.set_claude(claude_dir(home) / "settings.json", install=False)
    shutil.rmtree(bundle_dir("claude"), ignore_errors=True)
    return removed


def cursor_plugin_dir(home: Path) -> Path:
    return home / ".cursor" / "plugins" / "local" / "cadri"


def install_cursor(home: Path) -> str:
    strip_cursor_hooks(home / ".cursor" / "hooks.json")
    target = cursor_plugin_dir(home)
    build("cursor", target)
    return f"Cursor: plugin at {target} (reload Cursor to load it)"


def uninstall_cursor(home: Path) -> bool:
    removed = strip_cursor_hooks(home / ".cursor" / "hooks.json")
    target = cursor_plugin_dir(home)
    if target.exists():
        shutil.rmtree(target)
        removed = True
    return removed


def codex_agent(home: Path) -> Path:
    return codex_dir(home) / "agents" / f"{KEEPER}.toml"


def install_codex(home: Path) -> str:
    base = codex_dir(home)
    set_grouped_hooks(base / "hooks.json", grouped_hooks(CODEX_HOOKS, default_cli(), "codex"))
    agent = codex_agent(home)
    if agent.exists() and MARKER not in agent.read_text(encoding="utf-8", errors="replace"):
        raise InstallError(f"{agent} exists and was not written by Cadri; move it aside and run cadri init again")
    write_text(agent, keeper_toml())
    copy_skills(home / ".agents" / "skills")
    note = permissions.set_codex(base / "config.toml", install=True)
    return f"Codex: hooks in {base / 'hooks.json'} (open /hooks in Codex once and trust them){note}"


def uninstall_codex(home: Path) -> bool:
    removed = strip_cadri_hooks(codex_dir(home) / "hooks.json")
    agent = codex_agent(home)
    if agent.is_file() and MARKER in agent.read_text(encoding="utf-8", errors="replace"):
        agent.unlink()
        removed = True
    permissions.set_codex(codex_dir(home) / "config.toml", install=False)
    return removed


def opencode_files(home: Path) -> tuple[Path, Path]:
    base = opencode_dir(home)
    return base / "plugins" / "cadri.ts", base / "agents" / f"{KEEPER}.md"


def install_opencode(home: Path) -> str:
    plugin, agent = opencode_files(home)
    for path in (plugin, agent):
        if path.exists() and MARKER not in path.read_text(encoding="utf-8", errors="replace"):
            raise InstallError(f"{path} exists and was not written by Cadri; move it aside and run cadri init again")
    bundle = bundle_dir("opencode")
    build("opencode", bundle)
    write_text(plugin, (bundle / "cadri.ts").read_text(encoding="utf-8"))
    write_text(agent, (bundle / "agents" / f"{KEEPER}.md").read_text(encoding="utf-8"))
    note = permissions.set_opencode(opencode_dir(home), install=True)
    return f"OpenCode: plugin at {plugin}{note}"


def uninstall_opencode(home: Path) -> bool:
    removed = False
    for path in opencode_files(home):
        if path.is_file() and MARKER in path.read_text(encoding="utf-8", errors="replace"):
            path.unlink()
            removed = True
    permissions.set_opencode(opencode_dir(home), install=False)
    shutil.rmtree(bundle_dir("opencode"), ignore_errors=True)
    return removed


INSTALLERS = {
    "claude_code": (install_claude, uninstall_claude, "Claude Code plugin"),
    "codex": (install_codex, uninstall_codex, "Codex hooks"),
    "cursor": (install_cursor, uninstall_cursor, "Cursor plugin"),
    "opencode": (install_opencode, uninstall_opencode, "OpenCode plugin"),
}


def install(harnesses: list[str], home: Path | None = None) -> list[str]:
    home = home or Path.home()
    write_shim()
    lines = []
    for name in harnesses:
        try:
            lines.append(INSTALLERS[name][0](home))
        except (InstallError, OSError) as exc:
            lines.append(f"{HARNESS_NAMES.get(name, name)}: not installed: {exc}")
    if "codex" not in harnesses:
        remove_skill_copies(home / ".agents" / "skills")
    return lines


def uninstall(home: Path | None = None) -> list[str]:
    home = home or Path.home()
    removed = []
    for name, (_, remove, label) in INSTALLERS.items():
        try:
            if remove(home):
                removed.append(label)
        except (InstallError, OSError) as exc:
            removed.append(f"{label} (failed: {exc})")
    remove_skill_copies(home / ".agents" / "skills")
    return removed


def main(argv: list[str] | None = None) -> int:
    """`python -m cadri.install.bundles <dest>`: build every bundle for a release."""
    argv = list(sys.argv[1:] if argv is None else argv)
    if len(argv) != 1:
        print("usage: python -m cadri.install.bundles <dest>", file=sys.stderr)
        return 2
    dest = Path(argv[0])
    for harness in ("claude", "codex", "cursor", "opencode"):
        print(build(harness, dest / harness, cli='"$HOME/.cadri/bin/cadri"', installed_at=f"$HOME/.cadri/plugins/{harness}"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
