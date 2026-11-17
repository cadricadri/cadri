"""Designs an agent writes down for a person to accept before building.

The session page reads the same shapes in `engine/core/src/transcript/parse.rs`.
A plan is a markdown design. A todo checklist (`update_plan`) is not one.

Cursor keeps the document in `~/.cursor/plans/*.plan.md` (or the project's
`.cursor/plans/` after Save to workspace) and also inside a `CreatePlan` tool
call. Claude Code writes `~/.claude/plans/*.md`, or `<project>/.claude/plans/`.
Codex plan mode keeps the plan in the conversation; `~/.codex/plans/` is only
the optional plan skill. OpenCode discusses the plan in the conversation and,
only when asked, writes it under `~/.opencode/plan/`. The transcript holds the
draft; the plan file holds later edits the person made before accepting it.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

MIN_CHARS = 80
BODY_LIMIT = 24_000
PLAN_TOOLS = {"createplan", "exitplanmode", "submitplan", "presentplan", "writeplan"}
SKIP_TOOLS = {"updateplan", "enterplanmode"}
WRITE_TOOLS = {"write", "createfile"}
PLAN_ROOTS = (
    "/.cursor/plans/",
    "/.claude/plans/",
    "/.codex/plans/",
    "/.opencode/plan/",
)


def extract(content: str) -> list[dict]:
    """Plans in a stored session blob, last draft of each title."""
    plans: list[dict] = []
    text = content.lstrip()
    if text.startswith("{") and not _is_jsonl(text):
        try:
            document = json.loads(text)
        except json.JSONDecodeError:
            document = None
        if isinstance(document, (dict, list)):
            _walk(document, plans)
            return plans
    for line in content.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, (dict, list)):
            _walk(value, plans)
    return plans


def overlay(plans: list[dict], home: Path) -> list[dict]:
    """Replace a draft with the plan file of the same title, when the file differs.

    The file is what the person edited before accepting. A missing file leaves
    the draft in place.
    """
    if not plans:
        return []
    by_title = {}
    for directory in (
        home / ".cursor" / "plans",
        home / ".claude" / "plans",
        home / ".codex" / "plans",
        home / ".opencode" / "plan",
    ):
        if not directory.is_dir():
            continue
        for path in directory.rglob("*.md"):
            if not path.is_file():
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace")
            except OSError:
                continue
            plan = _document("", None, None, text, "file")
            if plan:
                by_title[plan["title"].casefold()] = plan
    updated = []
    for plan in plans:
        file_plan = by_title.get(plan["title"].casefold())
        if file_plan and file_plan["body"].strip() != plan["body"].strip():
            revised = dict(plan)
            revised["body"] = file_plan["body"]
            if file_plan.get("overview"):
                revised["overview"] = file_plan["overview"]
            revised["source"] = "file"
            updated.append(revised)
        else:
            updated.append(plan)
    return updated


def select_new(plans: list[dict], seen: list[str]) -> tuple[list[dict], list[str]]:
    """Plans whose text is not in `seen`, plus the hashes to remember."""
    known = set(seen)
    fresh: list[dict] = []
    hashes = list(seen)
    for plan in plans:
        digest = hash_plan(plan)
        if digest in known:
            continue
        stored = dict(plan)
        stored["body"] = clip_body(plan["body"])
        fresh.append(stored)
        hashes.append(digest)
        known.add(digest)
    return fresh, hashes


def absorb_files(plans: list[dict], harness: str | None, session: str | None) -> list[dict]:
    """Use the edited plan file, and remember that text so the next turn does not repeat it."""
    revised = overlay(plans, Path.home())
    if harness and session:
        from .state import Session

        record = Session(harness, session)
        seen = list(record.load().get("seen_plan_hashes") or [])
        _, hashes = select_new(revised, seen)
        if hashes != seen:
            record.update(seen_plan_hashes=hashes)
    stored = []
    for plan in revised:
        copied = dict(plan)
        copied["body"] = clip_body(plan["body"])
        stored.append(copied)
    return stored


def hash_plan(plan: dict) -> str:
    text = f"{plan.get('title', '')}\n{plan.get('body', '')}"
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def clip_body(body: str) -> str:
    if len(body) <= BODY_LIMIT:
        return body
    return body[:BODY_LIMIT].rstrip() + "\n\n…(plan truncated)"


def _is_jsonl(text: str) -> bool:
    lines = text.splitlines()
    return len(lines) > 1 and lines[1].lstrip().startswith("{")


def _walk(value: object, plans: list[dict]) -> None:
    if isinstance(value, list):
        for item in value:
            _walk(item, plans)
        return
    if not isinstance(value, dict):
        return
    name, tool_input = _tool_call(value)
    if name:
        for plan in _from_call(name, tool_input):
            _add(plans, plan)
        for key, child in value.items():
            if key not in {"input", "arguments", "state"}:
                _walk(child, plans)
        return
    for child in value.values():
        _walk(child, plans)


def _tool_call(value: dict) -> tuple[str | None, object]:
    kind = value.get("type")
    if kind in {"tool_use", "server_tool_use"} and isinstance(value.get("name"), str):
        return value["name"], value.get("input")
    if kind == "tool" and isinstance(value.get("tool") or value.get("name"), str):
        state = value.get("state")
        tool_input = state.get("input") if isinstance(state, dict) else None
        if tool_input is None:
            tool_input = value.get("input")
        return str(value.get("tool") or value.get("name")), tool_input
    if kind in {"function_call", "custom_tool_call"} and isinstance(value.get("name"), str):
        return value["name"], value.get("arguments", value.get("input"))
    return None, None


def _from_call(name: str, raw: object) -> list[dict]:
    key = _tool_key(name)
    if key in SKIP_TOOLS:
        return []
    data = _object(raw)
    if key == "applypatch":
        patch = _field(data, "command", "input", "patch") if data else None
        if patch is None and isinstance(raw, str):
            patch = raw
        if not patch:
            return []
        found = []
        for path, body in _added_files(patch):
            plan = _document(path, None, None, body, name)
            if plan:
                found.append(plan)
        return _upsert(found)
    if not data:
        return []
    if key in WRITE_TOOLS:
        path = _field(data, "path", "file_path", "filePath", "filepath")
        body = _field(data, "contents", "content", "file_text")
        plan = _document(path or "", None, None, body or "", name) if path and body else None
        return [plan] if plan else []
    if key in PLAN_TOOLS or "plan" in key:
        body = _field(data, "plan", "plan_markdown", "markdown")
        if not body:
            return []
        plan = _document("", _field(data, "name", "title"), _field(data, "overview"), body, name)
        return [plan] if plan else []
    return []


def _add(plans: list[dict], plan: dict) -> None:
    """A later draft of the same title replaces the earlier one."""
    for index, existing in enumerate(plans):
        if existing["title"].casefold() == plan["title"].casefold():
            plans[index] = plan
            return
    plans.append(plan)


def _upsert(plans: list[dict]) -> list[dict]:
    by_title: dict[str, dict] = {}
    order: list[str] = []
    for plan in plans:
        key = plan["title"].casefold()
        if key not in by_title:
            order.append(key)
        by_title[key] = plan
    return [by_title[key] for key in order]


def _object(raw: object) -> dict | None:
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return None
        return parsed if isinstance(parsed, dict) else None
    return None


def _field(data: dict | None, *keys: str) -> str | None:
    if not data:
        return None
    for key in keys:
        value = data.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def _added_files(patch: str) -> list[tuple[str, str]]:
    path: str | None = None
    lines: list[str] = []
    found: list[tuple[str, str]] = []

    def finish() -> None:
        nonlocal path
        if path and _is_plan_path(path):
            found.append((path, "\n".join(lines)))
        path = None

    for line in patch.splitlines():
        if line.startswith("*** Add File:"):
            finish()
            path = line.split(":", 1)[1].strip()
            lines = []
            continue
        if line.startswith("*** "):
            finish()
            lines = []
            continue
        if path is not None and line.startswith("+"):
            lines.append(line[1:])
    finish()
    return found


def _document(path: str, title: str | None, overview: str | None, raw: str, source: str) -> dict | None:
    if path and not _is_plan_path(path):
        return None
    matter_name, matter_overview, body = _frontmatter(raw)
    body = body.strip()
    if not _is_plan_body(body):
        return None
    resolved = title or matter_name or _markdown_title(body) or _path_title(path) or "Plan"
    plan = {"title": resolved, "body": body, "source": source}
    chosen = overview or matter_overview
    if chosen:
        plan["overview"] = chosen
    return plan


def _is_plan_path(path: str) -> bool:
    lowered = path.replace("\\", "/").lower()
    if not lowered.endswith(".md"):
        return False
    return lowered.endswith(".plan.md") or any(root in lowered for root in PLAN_ROOTS)


def _is_plan_body(body: str) -> bool:
    return len(body) >= MIN_CHARS and any(line.lstrip().startswith("# ") for line in body.splitlines())


def _markdown_title(body: str) -> str | None:
    for line in body.splitlines():
        if line.lstrip().startswith("# "):
            title = line.lstrip()[2:].strip()
            if title:
                return title
    return None


def _path_title(path: str) -> str | None:
    name = path.replace("\\", "/").rsplit("/", 1)[-1]
    if name.endswith(".plan.md"):
        name = name[: -len(".plan.md")]
    elif name.endswith(".md"):
        name = name[: -len(".md")]
    else:
        return None
    title = re.sub(r"[_-]+", " ", name).strip()
    return title or None


def _frontmatter(text: str) -> tuple[str | None, str | None, str]:
    stripped = text.strip()
    if not stripped.startswith("---"):
        return None, None, stripped
    rest = stripped[3:]
    if rest.startswith("\n"):
        rest = rest[1:]
    marker = rest.find("\n---")
    if marker < 0:
        return None, None, stripped
    head, body = rest[:marker], rest[marker + 4 :]
    name = overview = None
    for line in head.splitlines():
        if line.strip().startswith("name:"):
            name = _scalar(line, "name:")
        elif line.strip().startswith("overview:"):
            overview = _scalar(line, "overview:")
    return name, overview, body.lstrip("\n")


def _scalar(line: str, prefix: str) -> str | None:
    value = line.strip()[len(prefix) :].strip().strip("\"'")
    return value or None


def _tool_key(name: str) -> str:
    return "".join(ch for ch in name.lower() if ch.isalnum())
