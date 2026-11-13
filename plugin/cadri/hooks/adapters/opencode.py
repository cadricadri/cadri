"""OpenCode: the TypeScript plugin calls the CLI with a small JSON of its own.

Input: {"session_id", "turn_id", "prompt", "paths", "last_message", "source", "agent", "cwd"}.
Output: {"context", "notice"}. The plugin adds session-start context to every model
call through the `context` hook and prompt context as a session instruction entry.
"""

from __future__ import annotations

from .. import events as ev
from ..events import HookInput, HookOutput
from .base import Adapter, absolute, text


class OpenCodeAdapter(Adapter):
    name = "opencode"
    capture_name = "opencode"
    event_names = {"session.created": ev.SESSION_START, "session.idle": ev.STOP, "session.compacted": ev.PRE_COMPACT}

    def parse(self, event: str, raw: dict) -> HookInput:
        cwd = text(raw, "cwd")
        paths = raw.get("paths")
        return HookInput(
            harness=self.name,
            event=event,
            raw=raw,
            session_id=text(raw, "session_id"),
            turn_id=text(raw, "turn_id"),
            agent=text(raw, "agent"),
            source=text(raw, "source"),
            prompt=text(raw, "prompt"),
            edited_paths=[absolute(path, cwd) for path in paths if isinstance(path, str)] if isinstance(paths, list) else [],
            last_message=text(raw, "last_message"),
            model=text(raw, "model"),
            cwd=cwd,
        )

    def render(self, event: str, out: HookOutput) -> str:
        data = {"context": out.context, "notice": out.notice}
        return self.dump({key: value for key, value in data.items() if value})

    def empty(self, event: str) -> str:
        return "{}"
