"""Cursor: camelCase events, snake_case outputs, and stdout must always be JSON.

- sessionStart returns `additional_context` and `env`, but the agent does not wait for
  it, so the first postToolUse of a session delivers the same context again.
- beforeSubmitPrompt can only answer `{"continue": true}`; it cannot add context. The
  prompt hook saves its text and the next postToolUse of that generation delivers it.
- afterFileEdit carries `file_path`; afterAgentResponse carries the reply as `text`.
- Every payload has `conversation_id`, `generation_id`, and `cursor_version`.
"""

from __future__ import annotations

from .. import events as ev
from ..events import HookInput, HookOutput
from .base import Adapter, absolute, clip, text


class CursorAdapter(Adapter):
    name = "cursor"
    capture_name = "cursor"
    event_names = {
        "sessionStart": ev.SESSION_START,
        "beforeSubmitPrompt": ev.PROMPT,
        "postToolUse": ev.TOOL_AFTER,
        "afterFileEdit": ev.EDIT,
        "afterAgentResponse": ev.RESPONSE,
        "stop": ev.STOP,
        "preCompact": ev.PRE_COMPACT,
        "sessionEnd": ev.PRE_COMPACT,
    }

    def parse(self, event: str, raw: dict) -> HookInput:
        roots = raw.get("workspace_roots")
        cwd = roots[0] if isinstance(roots, list) and roots and isinstance(roots[0], str) else None
        paths = []
        if event == ev.EDIT:
            path = text(raw, "file_path")
            if path:
                paths.append(absolute(path, cwd))
        return HookInput(
            harness=self.name,
            event=event,
            raw=raw,
            session_id=text(raw, "conversation_id", "session_id"),
            turn_id=text(raw, "generation_id"),
            agent=text(raw, "subagent_type", "agent_type"),
            source=text(raw, "source"),
            prompt=text(raw, "prompt"),
            edited_paths=paths,
            last_message=text(raw, "text") if event == ev.RESPONSE else None,
            transcript_path=text(raw, "transcript_path"),
            model=text(raw, "model"),
            cwd=cwd,
        )

    def render(self, event: str, out: HookOutput) -> str:
        if event == ev.PROMPT:
            return '{"continue": true}'
        data: dict = {}
        if event in (ev.SESSION_START, ev.TOOL_AFTER) and out.context:
            data["additional_context"] = clip(out.context)
        if event == ev.SESSION_START and out.env:
            data["env"] = out.env
        return self.dump(data)

    def empty(self, event: str) -> str:
        return '{"continue": true}' if event == ev.PROMPT else "{}"
