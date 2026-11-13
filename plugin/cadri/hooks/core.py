"""What Cadri does at each hook, independent of harness.

Each handler returns what the agent should see now and the jobs to run after the
harness has its answer (in a detached process). Handlers do no network I/O beyond the
session-start budget; everything slow goes in `background`.

    session-start  check sign-in, fast-forward both books, print the memory context
    prompt         list relevant memory files, ask for a keeper when a reflection is due
    tool-after     Cursor only: deliver what session-start and prompt could not
    edit           record which session edited which memory file
    response       Cursor only: keep the reply text for the turn digest
    stop           save the turn digest; upload the transcript and push leftovers
    pre-compact    upload the transcript before it is compacted
"""

from __future__ import annotations

import shlex
import time
from dataclasses import dataclass, field
from typing import Callable

from .. import auth, capture, state
from ..memory import books as bk
from ..memory import context, digests, edits, push, recall, repair, sync
from . import events as ev
from .adapters.base import Adapter
from .events import HookInput, HookOutput

SESSION_START_NETWORK_SECONDS = 1.5
AUTH_SECONDS = 1.0
RECALL_LIMIT = 5
STATE_KEEP_SECONDS = 14 * 24 * 60 * 60

Job = Callable[[], object]


@dataclass
class Result:
    output: HookOutput = field(default_factory=HookOutput)
    background: list[Job] = field(default_factory=list)


def handle(inp: HookInput, adapter: Adapter) -> Result:
    handler = HANDLERS.get(inp.event)
    return handler(inp, adapter) if handler else Result()


def session(inp: HookInput) -> state.Session | None:
    return state.Session(inp.harness, inp.session_id) if inp.session_id else None


def present(books: list[bk.Book]) -> list[bk.Book]:
    return [book for book in books if book.path.is_dir()]


# --- session start --------------------------------------------------------------


def sign_in_notice(entry: dict) -> str:
    code = f" and enter the code {entry['user_code']}" if entry.get("user_code") else ""
    return f"Cadri needs you to sign in again: open {entry.get('url')}{code}. Memory sync resumes once you do."


def check_auth(base: str, deadline: float, result: Result, notices: list[str]) -> str:
    status = auth.check(base, timeout=AUTH_SECONDS)
    if status in (auth.MISSING, auth.EXPIRED):
        try:
            entry = auth.begin_background_login(base, timeout=max(0.3, deadline - time.monotonic()))
        except auth.SignInUnsupported:
            return auth.OK
        except (auth.AuthFailure, OSError, ValueError):
            notices.append("Cadri is not signed in and could not start a sign-in. Memory below is the local copy; ask the user to run `cadri login`.")
            return status
        result.output.notice = sign_in_notice(entry)
        notices.append(f"Cadri is not signed in; the user was shown a sign-in link ({entry.get('url')}). Memory below is the local copy.")
        result.background.append(lambda: auth.finish_background_login(base, entry))
    elif status == auth.OFFLINE:
        notices.append("The Cadri server is not reachable; memory below is the local copy.")
    return status


def find_books(base: str, signed_in: bool, deadline: float) -> list[bk.Book]:
    found = bk.local_books(base)
    if found or not signed_in:
        return found
    remaining = deadline - time.monotonic()
    if remaining < 0.2:
        return []
    try:
        return bk.books_for(base, bk.identity(base, timeout=remaining))
    except bk.MemoryFailure:
        return []


def background_sync(base: str, books: list[bk.Book], refresh_identity: bool) -> Job:
    def run() -> None:
        if refresh_identity:
            try:
                bk.identity(base, refresh=True)
            except bk.MemoryFailure:
                pass
        for book in books:
            try:
                sync.full(book, log=lambda message: print(message))
            except (bk.MemoryFailure, push.Offline, push.Rejected) as exc:
                print(f"cadri: {book.label} book sync failed: {exc}")
        state.prune(state.state_dir() / "sessions", STATE_KEEP_SECONDS)

    return run


def repair_request(book: bk.Book, names: context.Names) -> str:
    return (
        f"The {book.label} memory book has an unresolved push conflict. Unless a `{names.keeper}` is already on it, "
        f"launch `{names.keeper}` in the background with the task: \"Repair the {book.label} memory book.\""
    )


def session_start(inp: HookInput, adapter: Adapter) -> Result:
    deadline = time.monotonic() + SESSION_START_NETWORK_SECONDS
    base = auth.server_url()
    result = Result()
    notices: list[str] = []
    status = check_auth(base, deadline, result, notices)
    signed_in = status == auth.OK
    books = find_books(base, signed_in, deadline)
    if inp.session_id:
        result.output.env = {"CADRI_SESSION_ID": inp.session_id, "CADRI_HARNESS": inp.harness}
    if not books:
        if signed_in:
            result.output.context = "Cadri memory is not set up on this machine yet. If the user asks about it, suggest `cadri memory clone`."
        return result
    statuses: dict[str, sync.Status] = {}
    if bk.git_ready():
        network_deadline = deadline if signed_in else time.monotonic()
        for book in books:
            statuses[book.scope] = sync.quick(book, network_deadline)
        if signed_in:
            stale = time.time() - float(bk.cached_identity(base).get("checked_at") or 0) > bk.IDENTITY_MAX_AGE
            waiting = [book for book in books if statuses[book.scope].needs_background]
            if waiting or stale:
                result.background.append(background_sync(base, waiting, stale))
    else:
        notices.append("git is not installed, so memory cannot sync.")
    notices += [repair_request(book, adapter.names) for book in books if statuses.get(book.scope) and statuses[book.scope].state == sync.REPAIR]
    result.output.context = context.render(context.Inputs(books, statuses, inp.session_id, notices, adapter.names))
    current = session(inp)
    if current:
        current.update(seeded_at=time.time(), cwd=inp.cwd, seed_delivered=inp.harness != "cursor")
    return result


# --- prompt -----------------------------------------------------------------------


def recall_block(books: list[bk.Book], prompt: str) -> str:
    candidates = recall.search(books, prompt, limit=RECALL_LIMIT, skip={"MEMORY.md", "README.md"})  # indexes, not topics
    if not candidates:
        return ""
    lines = ["Cadri memory files that may be relevant to this prompt:"]
    for candidate in candidates:
        description = f" — {candidate.description}" if candidate.description else ""
        lines.append(f"- {candidate.book.path / candidate.path} ({candidate.book.label}){description}")
    reminder = context.reminder()
    return "\n".join(lines) + (f"\n\n{reminder}" if reminder else "")


def keeper_task(claim: digests.Claim, session_id: str) -> str:
    return f"Reflect on {claim.file} for session {session_id}."


def keeper_request(claim: digests.Claim, task: str, names: context.Names) -> str:
    turns = "turn is" if claim.turns == 1 else f"{claim.turns} turns are"
    return (
        f"Cadri: the last {turns} worth reflecting on ({'; '.join(claim.reasons)}). In your first tool batch, "
        f"launch the `{names.keeper}` subagent in the background with this task, then carry on without waiting for it:\n\n"
        f"    {task}"
    )


def remember_request(session_id: str | None) -> str:
    session = f" --session {shlex.quote(session_id)}" if session_id else ""
    return (
        "The user asked you to remember, forget, or correct something. Do it yourself in this turn: edit the right "
        "memory book (team for shared knowledge, personal for this person's preferences), then run "
        f'`cadri memory push --human{session} -m "<what>"`.'
    )


def is_subagent(inp: HookInput) -> bool:
    return push.normalize_agent(inp.agent) != "main"


def prompt(inp: HookInput, adapter: Adapter) -> Result:
    result = Result()
    if is_subagent(inp):
        return result
    base = auth.server_url()
    books = present(bk.local_books(base))
    text = inp.prompt or ""
    parts: list[str] = []
    if books and text:
        block = recall_block(books, text)
        if block:
            parts.append(block)
    current = session(inp)
    if current and inp.session_id:
        digests.add_reaction(inp.session_id, text)
        claim = digests.gate(inp.session_id, books={book.label: book.path for book in books})
        if claim:
            parts.append(keeper_request(claim, keeper_task(claim, inp.session_id), adapter.names))
    if "remember" in digests.signals(text):
        parts.append(remember_request(inp.session_id))
    parts += [repair_request(book, adapter.names) for book in books if repair.active(book)]
    pending = auth.pending_login(base)
    if pending:
        parts.append(f"Cadri is still waiting for the user to sign in at {pending.get('url')}; remind them once if memory matters for this task.")
    context_text = "\n\n".join(parts) or None
    if current:
        changes: dict = {"last_prompt": text, "turn_started": time.time(), "turn_edits": 0, "turn_id": inp.turn_id, "last_reply": None}
        if adapter.name == "cursor":
            changes.update(pending_context=context_text, pending_turn=inp.turn_id)
        current.update(**changes)
    if adapter.name != "cursor":
        result.output.context = context_text
    return result


# --- Cursor delivery ----------------------------------------------------------------


def tool_after(inp: HookInput, adapter: Adapter) -> Result:
    current = session(inp)
    if current is None:
        return Result()
    data = current.load()
    parts = []
    if not data.get("seed_delivered"):
        books = bk.local_books()
        if books:
            parts.append(context.render(context.Inputs(books, {}, inp.session_id, [], adapter.names)))
    pending = data.get("pending_context")
    if pending and data.get("pending_turn") == inp.turn_id:
        parts.append(pending)
    if not parts and data.get("seed_delivered"):
        return Result()
    current.update(seed_delivered=True, pending_context=None)
    return Result(HookOutput(context="\n\n".join(parts) or None))


# --- edits and replies --------------------------------------------------------------


def edit(inp: HookInput, adapter: Adapter) -> Result:
    if not inp.edited_paths:
        return Result()
    books = bk.local_books()
    agent = push.normalize_agent(inp.agent)
    for path in inp.edited_paths:
        for book in books:
            if book.contains(path):
                edits.record(book, book.relative(path), inp.session_id, agent, inp.harness)
                break
    current = session(inp)
    if current and agent == "main":
        current.bump("turn_edits", len(inp.edited_paths))
    return Result()


def response(inp: HookInput, adapter: Adapter) -> Result:
    current = session(inp)
    if current and inp.last_message:
        current.update(last_reply=inp.last_message)
    return Result()


# --- stop and compaction ------------------------------------------------------------


def upload_job(inp: HookInput, adapter: Adapter) -> Job:
    return lambda: capture.capture(adapter.capture_name, inp.raw)


def sweep_job(books: list[bk.Book], harness: str) -> Job:
    def run() -> None:
        for book in books:
            try:
                push.sweep(book, harness, log=lambda message: print(message))
            except bk.MemoryFailure as exc:
                print(f"cadri: {book.label} book push failed: {exc}")

    return run


def stop(inp: HookInput, adapter: Adapter) -> Result:
    result = Result(background=[upload_job(inp, adapter)])
    books = present(bk.local_books())
    current = None if is_subagent(inp) else session(inp)
    if current and inp.session_id:
        data = current.load()
        started = float(data.get("turn_started") or 0)
        if started and data.get("digest_turn") != started:
            memory_edits = [
                f"{book.label}:{record.path}"
                for book in books
                for record in edits.by_session(book, inp.session_id)
                if record.at >= started
            ]
            digests.save(
                inp.harness,
                inp.session_id,
                data.get("last_prompt"),
                inp.last_message or data.get("last_reply"),
                int(data.get("turn_edits") or 0),
                sorted(set(memory_edits)),
            )
            current.update(digest_turn=started, last_reply=None)
    if books and bk.git_ready():
        result.background.append(sweep_job(books, inp.harness))
    return result


def pre_compact(inp: HookInput, adapter: Adapter) -> Result:
    return Result(background=[upload_job(inp, adapter)])


HANDLERS: dict[str, Callable[[HookInput, Adapter], Result]] = {
    ev.SESSION_START: session_start,
    ev.PROMPT: prompt,
    ev.TOOL_AFTER: tool_after,
    ev.EDIT: edit,
    ev.RESPONSE: response,
    ev.STOP: stop,
    ev.PRE_COMPACT: pre_compact,
}
