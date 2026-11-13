"""The harness-neutral shape of a hook call: one input, one output, seven events.

Adapters translate each harness's hook JSON into a HookInput and render a HookOutput
back into exactly what that harness expects, so core.py never sees harness quirks.
"""

from __future__ import annotations

from dataclasses import dataclass, field

SESSION_START = "session-start"
PROMPT = "prompt"
TOOL_AFTER = "tool-after"
EDIT = "edit"
RESPONSE = "response"
STOP = "stop"
PRE_COMPACT = "pre-compact"

EVENTS = (SESSION_START, PROMPT, TOOL_AFTER, EDIT, RESPONSE, STOP, PRE_COMPACT)


@dataclass
class HookInput:
    harness: str
    event: str
    raw: dict
    session_id: str | None = None
    turn_id: str | None = None
    agent: str | None = None
    source: str | None = None
    prompt: str | None = None
    edited_paths: list[str] = field(default_factory=list)
    last_message: str | None = None
    transcript_path: str | None = None
    model: str | None = None
    cwd: str | None = None


@dataclass
class HookOutput:
    context: str | None = None
    notice: str | None = None
    env: dict[str, str] | None = None
