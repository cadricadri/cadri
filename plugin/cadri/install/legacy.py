"""Remove what earlier Cadri releases installed and the current one replaces."""

from __future__ import annotations

from pathlib import Path

from .config import remove_tree

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

# Every skill an earlier release copied into ~/.claude/skills or ~/.agents/skills.
COPIED_SKILLS = (
    "cadri-install",
    "cadri-memory",
    "cadri-memory-repair",
    "cadri-memory-seed",
    "context-doctor",
    "initializing-memory",
    "maintaining-core-memory",
    "maintaining-deferred-memory",
    "syncing-memory-filesystem",
)


def remove_legacy(home: Path) -> None:
    for relative in LEGACY_PATHS:
        remove_tree(home / relative)


def remove_skill_copies(root: Path) -> None:
    for name in COPIED_SKILLS:
        target = root / name
        if (target / "SKILL.md").is_file() and not target.is_symlink():
            remove_tree(target)
