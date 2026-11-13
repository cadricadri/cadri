"""What every harness adapter provides."""

from __future__ import annotations

import json
import os

from ...memory.context import Names
from ..events import HookInput, HookOutput

# Claude Code saves context over 10,000 characters to a file and sends a preview.
CONTEXT_LIMIT = 9500


def text(raw: dict, *keys: str) -> str | None:
    for key in keys:
        value = raw.get(key)
        if isinstance(value, str) and value:
            return value
    return None


def clip(context: str, limit: int = CONTEXT_LIMIT) -> str:
    if len(context) <= limit:
        return context
    return context[: limit - 60].rstrip() + "\n… (cut; run `cadri memory context` for all of it)"


def absolute(path: str, cwd: str | None) -> str:
    expanded = os.path.expanduser(path)
    if os.path.isabs(expanded) or not cwd:
        return expanded
    return os.path.join(cwd, expanded)


class Adapter:
    name = ""
    capture_name = ""
    names = Names()
    # hook_event_name → canonical event, for configs written by older releases that
    # call `cadri hook <harness>` without naming the event.
    event_names: dict[str, str] = {}

    def foreign(self, raw: dict) -> bool:
        """Whether this payload came from another harness running our hooks file."""
        return False

    def legacy_event(self, raw: dict) -> str | None:
        return self.event_names.get(text(raw, "hook_event_name") or "")

    def parse(self, event: str, raw: dict) -> HookInput:
        raise NotImplementedError

    def render(self, event: str, out: HookOutput) -> str:
        raise NotImplementedError

    def empty(self, event: str) -> str:
        return ""

    def apply_env(self, inp: HookInput, out: HookOutput) -> None:
        """Make out.env visible to later shell commands, where the harness offers a way."""

    @staticmethod
    def dump(data: dict) -> str:
        return json.dumps(data, ensure_ascii=False)
