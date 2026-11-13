"""Turn digests: what happened in a turn, kept until a memory keeper reflects on it.

Stop saves one per turn. The next prompt adds the user's reply to it — the best evidence
of what was worth remembering — and asks whether it is time to wake the keeper.

`gate` only decides *when* to wake the keeper. *What* to remember is not decided here:
the cadri-memory-keeper subagent reads the digest and follows
`skills/cadri-memory/references/maintain.md`. Often the answer is nothing.

Wake when the user has replied to a prior turn in this session (so the keeper can see
prompt, reply, and reaction together), or when digests from a session that ended without
another prompt have sat idle. Explicit "remember" / "forget" language is a separate path:
the main agent edits and pushes with `--human` in the same turn.

    Digests from sessions that ended without another prompt are picked up by the next
    session's first gate. Nothing here leaves the machine.
"""

from __future__ import annotations

import os
import re
import shutil
import time
from dataclasses import dataclass, field
from pathlib import Path

from ..state import read_json, safe_part, state_dir, write_json, write_text

PROMPT_CHARS = 2000
REPLY_HEAD = 1200
REPLY_TAIL = 1800
REACTION_CHARS = 1500
ORPHAN_IDLE_SECONDS = 30 * 60
CLAIM_SECONDS = 30 * 60
KEEP_SECONDS = 7 * 24 * 60 * 60
MAX_CLAIM = 8

# Cheap hints for the *main agent* when the user explicitly asks to remember/forget.
# They do not decide what the keeper writes into memory.
REMEMBER = re.compile(
    r"\b(remember|don'?t forget|forget that|note that|keep in mind|make a note|save this|write this down)\b",
    re.IGNORECASE,
)


@dataclass
class Claim:
    directory: Path
    file: Path
    turns: int
    reasons: list[str] = field(default_factory=list)


def root() -> Path:
    return state_dir() / "digests"


def pending_dir() -> Path:
    return root() / "pending"


def claimed_dir() -> Path:
    return root() / "claimed"


def signals(text: str | None) -> set[str]:
    """Hints that the *main agent* should edit memory this turn (not the keeper gate)."""
    if not text:
        return set()
    return {"remember"} if REMEMBER.search(text) else set()


def clip(text: str | None, limit: int) -> str:
    text = (text or "").strip()
    return text if len(text) <= limit else text[:limit].rstrip() + " …"


def clip_reply(text: str | None) -> str:
    text = (text or "").strip()
    if len(text) <= REPLY_HEAD + REPLY_TAIL:
        return text
    return f"{text[:REPLY_HEAD].rstrip()}\n…\n{text[-REPLY_TAIL:].lstrip()}"


def save(
    harness: str,
    session: str,
    prompt: str | None,
    reply: str | None,
    edit_count: int,
    memory_edits: list[str],
) -> Path | None:
    if not (prompt or reply):
        return None
    now = time.time()
    path = pending_dir() / f"{time.time_ns()}-{safe_part(harness)}-{safe_part(session)}.json"
    write_json(
        path,
        {
            "harness": harness,
            "session": session,
            "at": now,
            "prompt": clip(prompt, PROMPT_CHARS),
            "reply": clip_reply(reply),
            "reply_chars": len(reply or ""),
            "edit_count": edit_count,
            "memory_edits": memory_edits,
            "reaction": None,
        },
    )
    return path


def pending() -> list[tuple[Path, dict]]:
    found = []
    cutoff = time.time() - KEEP_SECONDS
    try:
        entries = sorted(pending_dir().glob("*.json"))
    except OSError:
        return []
    for path in entries:
        data = read_json(path)
        if not data or float(data.get("at") or 0) < cutoff:
            path.unlink(missing_ok=True)
            continue
        found.append((path, data))
    return found


def add_reaction(session: str, prompt: str) -> dict | None:
    """Attach the user's next message to this session's latest digest that has none."""
    for path, data in reversed(pending()):
        if data.get("session") != session:
            continue
        if data.get("reaction") is not None:
            return None
        data["reaction"] = clip(prompt, REACTION_CHARS)
        write_json(path, data)
        return data
    return None


def release_stale(now: float) -> None:
    try:
        claims = list(claimed_dir().iterdir())
    except OSError:
        return
    for claim in claims:
        try:
            if now - claim.stat().st_mtime < CLAIM_SECONDS:
                continue
        except OSError:
            continue
        for path in claim.glob("*.json"):
            try:
                os.replace(path, pending_dir() / path.name)
            except OSError:
                pass
        shutil.rmtree(claim, ignore_errors=True)


def gate(session: str, now: float | None = None, books: dict[str, Path] | None = None) -> Claim | None:
    """Wake the keeper when there is an unreflected turn ready for an LLM to review.

    This is not a classifier of what belongs in memory. The keeper reads the claimed
    digest and applies `references/maintain.md`; most turns change nothing.

    Ready means: this session has at least one digest whose next user message is
    attached (the usual turn N+1 case), or digests from another session have been
    idle long enough that no further reply is coming.
    """
    now = time.time() if now is None else now
    release_stale(now)
    waiting = pending()
    mine = [(path, data) for path, data in waiting if data.get("session") == session]
    orphans = [
        (path, data)
        for path, data in waiting
        if data.get("session") != session and now - float(data.get("at") or 0) >= ORPHAN_IDLE_SECONDS
    ]
    reasons = []
    ready = [(path, data) for path, data in mine if data.get("reaction") is not None]
    if ready:
        reasons.append("a prior turn is ready for the keeper to review")
    if orphans:
        reasons.append("turns from an earlier session were never reflected on")
    if not reasons:
        return None
    directory = claimed_dir() / f"{time.time_ns()}-{safe_part(session)}"
    directory.mkdir(parents=True, exist_ok=True)
    taken: list[dict] = []
    # Prefer digests that already have the user's next message; then orphans.
    for path, data in (ready + orphans)[:MAX_CLAIM]:
        try:
            os.replace(path, directory / path.name)
        except OSError:
            continue
        taken.append(data)
    if not taken:
        shutil.rmtree(directory, ignore_errors=True)
        return None
    file = directory / "reflect.md"
    write_text(file, render(taken, books or {}))
    return Claim(directory, file, len(taken), reasons)


def render(turns: list[dict], books: dict[str, Path] | None = None) -> str:
    parts = [
        "# Turns to reflect on",
        "",
        "Each section is one agent turn: what the user asked, how the agent replied,",
        "and the user's next message when there was one.",
        "",
        "Decide what is worth keeping using the Cadri maintain rules (corrections, preferences,",
        "decisions, durable discoveries). Most turns change nothing — that is a fine outcome.",
        "Do not treat this file as a checklist of facts to copy in.",
    ]
    if books:
        parts += ["", "Memory books:", ""]
        parts += [f"- {label}: {path}" for label, path in books.items()]
    for number, data in enumerate(sorted(turns, key=lambda item: item.get("at") or 0), start=1):
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(float(data.get("at") or 0)))
        parts += ["", f"## Turn {number} — {data.get('harness')} session {data.get('session')} at {when}", ""]
        parts += ["**User asked:**", "", data.get("prompt") or "(not captured)", ""]
        parts += ["**Agent replied:**", "", data.get("reply") or "(not captured)", ""]
        if data.get("memory_edits"):
            parts += ["**Memory files the agent already edited:** " + ", ".join(data["memory_edits"]), ""]
        if data.get("edit_count"):
            parts += [f"**Files edited during the turn:** {data['edit_count']}", ""]
        reaction = data.get("reaction")
        parts += ["**User's next message:**", "", reaction if reaction else "(none; the session ended or moved on)"]
    return "\n".join(parts) + "\n"


def finish(path: str | Path) -> bool:
    """Delete a claim once its keeper is done. Accepts the claim directory or its reflect.md."""
    target = Path(path).expanduser()
    directory = target.parent if target.name == "reflect.md" else target
    try:
        directory.resolve().relative_to(claimed_dir().resolve())
    except (ValueError, OSError):
        return False
    if not directory.is_dir():
        return False
    shutil.rmtree(directory, ignore_errors=True)
    return True
