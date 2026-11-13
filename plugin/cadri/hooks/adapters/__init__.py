"""One adapter per harness."""

from __future__ import annotations

from .base import Adapter
from .claude import ClaudeAdapter
from .codex import CodexAdapter
from .cursor import CursorAdapter
from .opencode import OpenCodeAdapter

ADAPTERS: dict[str, Adapter] = {
    adapter.name: adapter for adapter in (ClaudeAdapter(), CodexAdapter(), CursorAdapter(), OpenCodeAdapter())
}
ALIASES = {"claude_code": "claude"}


def get(name: str) -> Adapter | None:
    return ADAPTERS.get(ALIASES.get(name, name))
