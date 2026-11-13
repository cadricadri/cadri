"""Codex: Claude-compatible hook JSON, with differences that matter here.

- SessionStart and UserPromptSubmit add `additionalContext` (about 2,500 tokens unless
  the handler sets `additionalContextLimit`; the hooks file sets it).
- Stop must print JSON; `{}` lets the turn end.
- File edits go through `apply_patch`; the patch text is in `tool_input.command`, with
  `*** Add File:`, `*** Update File:`, `*** Delete File:`, and `*** Move to:` lines.
- Hooks inside a subagent carry `agent_id` and `agent_type`.
"""

from __future__ import annotations

import re

from .. import events as ev
from ..events import HookInput, HookOutput
from .base import Adapter, absolute, clip, text

PATCH_PATH = re.compile(r"^\*\*\* (?:Add File|Update File|Delete File|Move to): (.+?)\s*$", re.MULTILINE)
EDIT_TOOLS = {"Edit", "Write"}


def patch_paths(patch: str) -> list[str]:
    return list(dict.fromkeys(match.group(1) for match in PATCH_PATH.finditer(patch)))


class CodexAdapter(Adapter):
    name = "codex"
    capture_name = "codex"
    hook_names = {ev.SESSION_START: "SessionStart", ev.PROMPT: "UserPromptSubmit"}
    event_names = {
        "SessionStart": ev.SESSION_START,
        "UserPromptSubmit": ev.PROMPT,
        "PostToolUse": ev.EDIT,
        "Stop": ev.STOP,
        "PreCompact": ev.PRE_COMPACT,
    }

    def foreign(self, raw: dict) -> bool:
        return "cursor_version" in raw

    def parse(self, event: str, raw: dict) -> HookInput:
        cwd = text(raw, "cwd")
        paths: list[str] = []
        tool_input = raw.get("tool_input")
        tool = raw.get("tool_name")
        if tool == "apply_patch":
            patch = tool_input if isinstance(tool_input, str) else (text(tool_input, "command", "input", "patch") if isinstance(tool_input, dict) else None)
            paths = [absolute(path, cwd) for path in patch_paths(patch or "")]
        elif tool in EDIT_TOOLS and isinstance(tool_input, dict):
            path = text(tool_input, "file_path", "path")
            if path:
                paths.append(absolute(path, cwd))
        return HookInput(
            harness=self.name,
            event=event,
            raw=raw,
            session_id=text(raw, "session_id"),
            turn_id=text(raw, "turn_id"),
            agent=text(raw, "agent_type"),
            source=text(raw, "source"),
            prompt=text(raw, "prompt"),
            edited_paths=paths,
            last_message=text(raw, "last_assistant_message"),
            transcript_path=text(raw, "transcript_path"),
            model=text(raw, "model"),
            cwd=cwd,
        )

    def render(self, event: str, out: HookOutput) -> str:
        if event == ev.STOP:
            return "{}"
        if event not in (ev.SESSION_START, ev.PROMPT):
            return ""
        data: dict = {}
        if out.context:
            data["hookSpecificOutput"] = {
                "hookEventName": self.hook_names[event],
                "additionalContext": clip(out.context),
            }
        if out.notice:
            data["systemMessage"] = out.notice
        return self.dump(data) if data else ""

    def empty(self, event: str) -> str:
        return "{}" if event == ev.STOP else ""
