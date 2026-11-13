"""Claude Code: settings/plugin hooks with JSON on stdin, `hookSpecificOutput` on stdout.

- SessionStart and UserPromptSubmit add `additionalContext` (10,000-character cap).
- `systemMessage` is shown to the user.
- PostToolUse for Write/Edit/MultiEdit/NotebookEdit carries `tool_input.file_path`.
- Stop carries `last_assistant_message`; the transcript can lag behind it.
- Hooks inside a subagent carry `agent_type`.
- SessionStart may append `export` lines to `CLAUDE_ENV_FILE` for later Bash calls.
- Cursor can load these hooks as third-party hooks; its payloads carry `cursor_version`.
"""

from __future__ import annotations

import os
import re
import shlex

from ...memory.context import Names
from .. import events as ev
from ..events import HookInput, HookOutput
from .base import Adapter, absolute, clip, text

EDIT_TOOLS = {"Write", "Edit", "MultiEdit", "NotebookEdit"}
ENV_NAME = re.compile(r"^[A-Z_][A-Z0-9_]*$")


PLUGIN_NAMES = Names(
    memory_skill="cadri:cadri-memory",
    repair_skill="cadri:cadri-memory-repair",
    keeper="cadri:cadri-memory-keeper",
)


class ClaudeAdapter(Adapter):
    name = "claude"
    capture_name = "claude_code"

    @property
    def names(self) -> Names:
        """Plugin skills and agents are namespaced; the settings-file fallback's are not."""
        return PLUGIN_NAMES if os.environ.get("CLAUDE_PLUGIN_ROOT") else Names()
    hook_names = {
        ev.SESSION_START: "SessionStart",
        ev.PROMPT: "UserPromptSubmit",
        ev.EDIT: "PostToolUse",
        ev.STOP: "Stop",
        ev.PRE_COMPACT: "PreCompact",
    }
    event_names = {
        "SessionStart": ev.SESSION_START,
        "UserPromptSubmit": ev.PROMPT,
        "PostToolUse": ev.EDIT,
        "Stop": ev.STOP,
        "PreCompact": ev.PRE_COMPACT,
        "SessionEnd": ev.PRE_COMPACT,
    }

    def foreign(self, raw: dict) -> bool:
        return "cursor_version" in raw or ("conversation_id" in raw and "generation_id" in raw)

    def parse(self, event: str, raw: dict) -> HookInput:
        cwd = text(raw, "cwd")
        paths: list[str] = []
        tool_input = raw.get("tool_input")
        if raw.get("tool_name") in EDIT_TOOLS and isinstance(tool_input, dict):
            path = text(tool_input, "file_path", "notebook_path")
            if path:
                paths.append(absolute(path, cwd))
        return HookInput(
            harness=self.name,
            event=event,
            raw=raw,
            session_id=text(raw, "session_id"),
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

    def apply_env(self, inp: HookInput, out: HookOutput) -> None:
        env_file = os.environ.get("CLAUDE_ENV_FILE")
        if inp.event != ev.SESSION_START or not env_file or not out.env:
            return
        lines = [
            f"export {key}={shlex.quote(value)}\n"
            for key, value in out.env.items()
            if ENV_NAME.match(key) and isinstance(value, str)
        ]
        with open(env_file, "a", encoding="utf-8") as handle:
            handle.writelines(lines)
