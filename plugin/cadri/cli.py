"""Cadri CLI: sign-in, setup, agent hooks, session capture, and the memory books."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
import webbrowser
from pathlib import Path

from . import auth
from .discover import cursor, detected_harnesses, discover_all
from .discover.opencode import export_session_json
from .discover.types import SessionRef
from .hooks import adapters
from .hooks import run as hook_run
from .hooks.events import EVENTS
from .install import bundles, config
from .memory import books as bk
from .memory import context, digests, editapi, push, repair, sync
from .upload import post_session

PLUGIN_ROOT = Path(__file__).resolve().parents[1]
BACKFILL_WINDOW_SECONDS = 30 * 24 * 60 * 60
BACKFILL_MAX_SESSIONS = 200
BACKFILL_MAX_BYTES = 24 * 1024 * 1024
# Seed from whichever set is smaller. A tighter window on `cadri discover --seed` overrides these.
SEED_DAYS = 60
SEED_LIMIT = 100
HARNESS_NAMES = {
    "claude_code": "Claude Code",
    "cursor": "Cursor",
    "codex": "Codex",
    "opencode": "OpenCode",
}


class CliError(Exception):
    pass


def version() -> str:
    try:
        return (PLUGIN_ROOT / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "source"


# --- sign-in --------------------------------------------------------------------


def cmd_login(args: argparse.Namespace) -> int:
    base = auth.server_url(args.url)
    if args.logout:
        auth.update_entry(base, None)
        print(f"Signed out of {base}")
        return 0
    try:
        device = auth.start_device(base)
    except OSError as exc:
        raise CliError(f"could not reach {base}: {exc}") from exc
    url = device.get("verification_uri_complete") or device.get("verification_uri")
    print(f"To sign in to {base}, open:\n\n  {url}\n\nand confirm the code: {device.get('user_code')}\n")
    if not args.no_browser and isinstance(url, str):
        try:
            webbrowser.open(url)
        except webbrowser.Error:
            pass
    print("Waiting for confirmation...", flush=True)
    auth.update_entry(base, auth.poll_device(base, device))
    auth.save_pending(base, None)
    print(f"Signed in. Credentials saved to {auth.credentials_path()}")
    return 0


def ensure_auth(base: str, skip_login: bool) -> None:
    if auth.check(base, timeout=5) in (auth.OK, auth.OFFLINE) or skip_login:
        return
    print("Not signed in; starting browser sign-in…")
    try:
        cmd_login(argparse.Namespace(url=base, logout=False, no_browser=False))
    except auth.SignInUnsupported:
        print(f"{base} runs without sign-in; continuing unsigned.")


# --- setup and capture ----------------------------------------------------------


def read_session_content(ref: SessionRef) -> str:
    if ref.harness == "opencode":
        return export_session_json(ref.path, ref.thread_id)
    try:
        return ref.path.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        raise CliError(f"could not read {ref.path}: {exc}") from exc


def backfill(base: str, since: float, max_sessions: int, max_bytes: int) -> dict:
    sessions = [ref for ref in discover_all() if ref.updated_at >= since][:max_sessions]
    uploaded = skipped = errors = bytes_used = 0
    by_harness: dict[str, int] = {}
    for ref in sessions:
        if bytes_used >= max_bytes:
            skipped += 1
            continue
        try:
            content = read_session_content(ref)
        except CliError as exc:
            print(f"cadri: skip {ref.harness}/{ref.thread_id}: {exc}", file=sys.stderr)
            errors += 1
            continue
        size = len(content.encode())
        if bytes_used + size > max_bytes and uploaded > 0:
            skipped += 1
            continue
        payload = {
            "thread_id": ref.thread_id,
            "content": content,
            "hook_event": "backfill",
            "harness": ref.harness,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ref.updated_at)),
        }
        if ref.harness == "cursor":
            model = cursor.model_name(Path.home(), ref.thread_id)
            if model:
                payload["model"] = model
        try:
            status, body = post_session(base, payload)
        except (OSError, ValueError) as exc:
            raise CliError(f"could not reach {base}: {exc}") from exc
        if status in (200, 201):
            uploaded += 1
            bytes_used += size
            by_harness[ref.harness] = by_harness.get(ref.harness, 0) + 1
        elif status == 403 and body.get("error") == "no_organization":
            raise CliError("create your team in the Cadri browser app, then re-run cadri init")
        elif status in (401, 403):
            raise CliError("auth failed; run cadri login or set CADRI_API_TOKEN")
        else:
            print(f"cadri: upload {ref.harness}/{ref.thread_id} failed ({status})", file=sys.stderr)
            errors += 1
    return {
        "uploaded": uploaded,
        "skipped": skipped,
        "errors": errors,
        "bytes": bytes_used,
        "by_harness": by_harness,
        "detected": detected_harnesses(),
    }


def start_local_engine() -> None:
    repo = PLUGIN_ROOT.parent
    engine_dir = repo / "engine"
    if not engine_dir.is_dir():
        raise CliError(f"missing {engine_dir}; run ./plugin/bin/cadri init --local --start-engine from a cadri checkout")
    log = Path.home() / ".cadri" / "engine.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        "bash",
        "-lc",
        f'cd "{engine_dir}" && CADRI_BIND=127.0.0.1:8080 CADRI_DB="{Path.home() / ".cadri" / "transcripts.db"}" '
        f'nohup cargo run --bin cadri-engine > "{log}" 2>&1 & echo $!',
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    print(f"Started local engine (log {log}). pid={proc.stdout.strip() or 'unknown'}")


def clone_books(url: str | None) -> str:
    if not bk.git_ready():
        return (
            "Memory: skipped, git is not installed. Run `xcode-select --install`, "
            "then `cadri memory clone` and `cadri memory clone --scope user`."
        )
    started = time.monotonic()
    paths = []
    try:
        for scope in ("team", "user"):
            book = bk.open_book(scope, url)
            bk.ensure_clone(book)
            paths.append(str(book.path))
    except bk.MemoryFailure as exc:
        if paths:
            return f"Memory: cloned {', '.join(paths)}, then failed ({exc})"
        return f"Memory: not cloned ({exc}); run `cadri memory clone` later"
    return (
        f"Memory: cloned in {time.monotonic() - started:.1f}s to {', '.join(paths)}. "
        "Books stay empty until seeding writes the first memories."
    )


def cmd_init(args: argparse.Namespace) -> int:
    if args.local:
        os.environ.setdefault("CADRI_URL", "http://127.0.0.1:8080")
        if args.start_engine:
            start_local_engine()
            time.sleep(1)
    # init picks the server afresh; the saved URL is what it leaves behind for hooks.
    base = auth.normalize_url(args.url or os.environ.get("CADRI_URL") or auth.DEFAULT_URL)
    os.environ["CADRI_URL"] = base
    ensure_auth(base, skip_login=args.local and not args.login)
    auth.save_url(base)
    harnesses = sorted(
        set(config.installed_harnesses()) | set(detected_harnesses()),
        key=list(HARNESS_NAMES).index,
    )
    found = ", ".join(HARNESS_NAMES[name] for name in harnesses)
    print(f"Agents on this machine: {found or 'none found'}")
    print("Installing Cadri into each agent…")
    for line in bundles.install(harnesses):
        print(f"  {line}")
    print(f"  CLI: {bundles.shim_path()}")
    print(f"Uploading the last 30 days of sessions to {base}…")
    summary = backfill(base, time.time() - BACKFILL_WINDOW_SECONDS, BACKFILL_MAX_SESSIONS, BACKFILL_MAX_BYTES)
    print(
        f"  uploaded {summary['uploaded']} sessions ({summary['bytes'] // 1024} KiB); "
        f"skipped {summary['skipped']}; errors {summary['errors']}"
    )
    if summary["by_harness"]:
        print("  " + ", ".join(f"{HARNESS_NAMES[k]} {v}" for k, v in summary["by_harness"].items()))
    print(clone_books(args.url))
    if "codex" in harnesses:
        print("Codex runs new hooks only after you trust them: open Codex, type /hooks, and trust the Cadri hooks.")
    chosen, in_window, newest = select_seed_sessions(discover_all(), time.time(), SEED_DAYS, SEED_LIMIT)
    print(
        f"Seed window: {len(chosen)} sessions "
        f"({in_window} in the last {SEED_DAYS} days, {newest} most recent)."
    )
    print("Seeding usually takes several minutes. Do it in this same task, before you stop.")
    print(
        "Next: follow the cadri-memory-seed skill. "
        "If the user asked for a smaller window, pass it to `cadri discover --seed`."
    )
    return 0


def cmd_uninstall(_: argparse.Namespace) -> int:
    try:
        removed = bundles.uninstall()
    except config.InstallError as exc:
        raise CliError(str(exc)) from exc
    print(f"Removed: {', '.join(removed) if removed else 'Cadri was not installed in any agent'}")
    print("Memory books stay in ~/.cadri/memory. Credentials stay in ~/.config/cadri; run `cadri login --logout` to remove them.")
    return 0


def cmd_hook(args: argparse.Namespace) -> int:
    if args.target in EVENTS:
        if not args.harness:
            raise CliError("cadri hook <event> needs --harness")
        return hook_run.run(args.target, args.harness, sys.stdin.read())
    # `cadri hook <harness>`, written by releases before hooks named their event.
    return hook_run.run(None, args.target, sys.stdin.read())


def resolve_discover_bounds(days: float | None, limit: int | None, seed: bool) -> tuple[float, int]:
    if seed:
        return (SEED_DAYS if days is None else days, SEED_LIMIT if limit is None else limit)
    return (30.0 if days is None else days, 50 if limit is None else limit)


def select_seed_sessions(refs: list[SessionRef], now: float, days: float, limit: int) -> tuple[list[SessionRef], int, int]:
    """The smaller of sessions in the last `days` and the `limit` most recent.

    Both sets are prefixes of a newest-first list, so the smaller one is that prefix.
    Returns (chosen, how many fall in the day window, how many the recency cap kept).
    """
    ordered = sorted(refs, key=lambda item: item.updated_at, reverse=True)
    since = now - days * 24 * 60 * 60
    in_window = [ref for ref in ordered if ref.updated_at >= since]
    newest = ordered[: max(limit, 0)]
    chosen = in_window if len(in_window) <= len(newest) else newest
    return chosen, len(in_window), len(newest)


def cmd_discover(args: argparse.Namespace) -> int:
    days, limit = resolve_discover_bounds(args.days, args.limit, args.seed)
    chosen, in_window, newest = select_seed_sessions(discover_all(), time.time(), days, limit)
    refs = chosen
    if args.seed:
        summary = (
            f"seed window: {len(chosen)} sessions "
            f"({in_window} in the last {days:g} days, {newest} most recent)"
        )
        print(summary, file=sys.stderr)
    if args.json:
        rows = [
            {
                "harness": ref.harness,
                "thread_id": ref.thread_id,
                "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ref.updated_at)),
            }
            for ref in refs
        ]
        print(json.dumps(rows, indent=2))
        return 0
    for ref in refs:
        print(f"{ref.harness}\t{ref.thread_id}\t{ref.path}")
    return 0


def cmd_show(args: argparse.Namespace) -> int:
    for ref in discover_all():
        if ref.thread_id == args.thread_id:
            sys.stdout.write(read_session_content(ref))
            return 0
    raise CliError(f"no local session {args.thread_id}; list them with cadri discover")


# --- memory -------------------------------------------------------------------------


def err(message: str) -> None:
    print(message, file=sys.stderr)


def selected_books(scope: str, url: str | None) -> list[bk.Book]:
    scopes = ("team", "user") if scope == "all" else (scope,)
    found = {book.scope: book for book in bk.local_books(url)}
    if all(name in found for name in scopes):
        return [found[name] for name in scopes]
    return [bk.open_book(name, url) for name in scopes]


def human_author(base: str) -> push.Author:
    ident = bk.cached_identity(base)
    if not ident.get("user_id"):
        ident = bk.identity(base)
    return push.Author("human", ident["user_id"], ident.get("email") or "Cadri user")


def claim_started(digest: str | None) -> float | None:
    """When the keeper's claim was made, from its `<time_ns>-<session>` directory name."""
    if not digest:
        return None
    directory = Path(digest).expanduser()
    if directory.name == "reflect.md":
        directory = directory.parent
    try:
        return int(directory.name.split("-", 1)[0]) / 1e9
    except ValueError:
        return None


def memory_push(args: argparse.Namespace) -> int:
    books = selected_books(args.book, args.url)
    if args.abort or args.resume:
        status = 0
        for book in books:
            if repair.active(book):
                status = max(status, (push.abort if args.abort else push.resume)(book, log=err))
        return status
    base = auth.server_url(args.url)
    human = human_author(base) if args.human else None
    caller = push.caller_from(args.session, args.harness, args.agent, human, since=claim_started(args.digest))
    message = "\n\n".join(args.message) if args.message else None
    status = 0
    for book in books:
        status = max(status, push.push_book(book, caller, message, log=err))
    if args.digest and status != push.EXIT_CONFLICT:
        digests.finish(args.digest)
    return status


def memory_context(args: argparse.Namespace) -> int:
    adapter = adapters.get(args.harness) if args.harness else None
    names = adapter.names if adapter else context.Names()
    books = [book for book in bk.local_books(args.url) if book.path.is_dir()]
    if not books:
        err("cadri: no memory books on this machine; run cadri memory clone")
        return 1
    print(context.render(context.Inputs(books, {}, bk.session_id_from_env(), [], names), budget=args.budget), end="")
    return 0


def memory_status(args: argparse.Namespace) -> int:
    books = bk.local_books(args.url)
    if not books:
        print("No memory books yet; run cadri memory clone")
        return 1
    for book in books:
        print(f"{book.label} book: {book.path}")
        if not book.cloned():
            print("  not cloned")
            continue
        synced = bk.last_synced(book)
        print(f"  last synced: {time.strftime('%Y-%m-%d %H:%M', time.localtime(synced)) if synced else 'never'}")
        ahead = push.ahead_count(book)
        if ahead:
            print(f"  unpushed commits: {ahead}")
        changed = push.changed_paths(book)
        if changed:
            print(f"  uncommitted files: {', '.join(changed)}")
        if repair.active(book):
            print(f"  repair in progress at {repair.worktree(book)}: {', '.join(repair.marker(book).get('conflicts', []))}")
    waiting = digests.pending()
    if waiting:
        print(f"Turns waiting for reflection: {len(waiting)}")
    return 0


def memory_sync(args: argparse.Namespace) -> int:
    status = 0
    for book in selected_books(args.scope, args.url):
        status = max(status, sync.pull(book, quiet=args.quiet))
    return status


def cmd_memory(args: argparse.Namespace) -> int:
    action = args.memory_command
    try:
        if action == "push":
            return memory_push(args)
        if action == "context":
            return memory_context(args)
        if action == "status":
            return memory_status(args)
        if action == "sync":
            return memory_sync(args)
        book = bk.open_book(args.scope, args.url)
        if action == "clone":
            bk.ensure_clone(book)
            if sync.pull(book, quiet=True):
                return 1
            print(book.path)
            return 0
        if action == "path":
            bk.ensure_clone(book)
            print(book.path)
            return 0
        if action == "write":
            if args.file:
                try:
                    content = Path(args.file).read_text(encoding="utf-8")
                except OSError as exc:
                    raise CliError(f"could not read {args.file}: {exc}") from exc
            else:
                content = sys.stdin.read()
            return editapi.write(book, args.path, content, "\n\n".join(args.message), args.agent, args.session)
    except bk.MemoryFailure as exc:
        err(f"cadri: {exc}")
        return exc.code
    raise CliError(f"unknown memory command {action}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="cadri", description="Cadri: shared memory for coding agents")
    parser.add_argument("--version", action="version", version=f"cadri {version()}")
    sub = parser.add_subparsers(dest="command", required=True)

    login = sub.add_parser("login", help="Sign in via browser device flow")
    login.add_argument("--url", default=None)
    login.add_argument("--logout", action="store_true")
    login.add_argument("--no-browser", action="store_true")
    login.set_defaults(func=cmd_login)

    init = sub.add_parser("init", help="Sign in, install Cadri into each agent, upload recent sessions, clone memory")
    init.add_argument("--url", default=None)
    init.add_argument("--local", action="store_true", help="Target localhost Cadri (no WorkOS by default)")
    init.add_argument("--login", action="store_true", help="Force device login even with --local")
    init.add_argument("--start-engine", action="store_true", help="With --local, start cadri-engine in background")
    init.set_defaults(func=cmd_init)

    uninstall = sub.add_parser("uninstall", help="Remove Cadri's hooks, plugins, and skills")
    uninstall.set_defaults(func=cmd_uninstall)

    hook = sub.add_parser("hook", help="Run by agent hooks with the hook JSON on stdin")
    hook.add_argument("target", help=f"the event ({', '.join(EVENTS)}), or a harness name for older configs")
    hook.add_argument("--harness", default=None, choices=sorted(adapters.ADAPTERS) + sorted(adapters.ALIASES))
    hook.set_defaults(func=cmd_hook)

    discover = sub.add_parser("discover", help="List local agent sessions, newest first")
    discover.add_argument("--json", action="store_true")
    discover.add_argument("--seed", action="store_true", help="Sessions to seed: the smaller of the day window and the recency cap")
    discover.add_argument("--limit", type=int, default=None, help="Recency cap (default 50, or 100 with --seed)")
    discover.add_argument("--days", type=float, default=None, help="Day window (default 30, or 60 with --seed)")
    discover.set_defaults(func=cmd_discover)

    show = sub.add_parser("show", help="Print one local session")
    show.add_argument("thread_id")
    show.set_defaults(func=cmd_show)

    mem = sub.add_parser("memory", help="The team and personal memory books")
    mem_sub = mem.add_subparsers(dest="memory_command", required=True)

    def child(name: str, help_text: str) -> argparse.ArgumentParser:
        parser = mem_sub.add_parser(name, help=help_text)
        parser.add_argument("--url", default=None)
        parser.set_defaults(func=cmd_memory)
        return parser

    def scoped(name: str, help_text: str, choices=("team", "user"), default="team") -> argparse.ArgumentParser:
        parser = child(name, help_text)
        parser.add_argument("--scope", choices=list(choices), default=default)
        return parser

    scoped("clone", "Clone the book if missing, otherwise sync; print the path")
    scoped("path", "Print the clone path (cloning first if missing)")
    scoped("sync", "Bring the clone up to date and publish unpushed commits", ("team", "user", "all"), "all").add_argument(
        "--quiet", action="store_true"
    )

    push_cmd = child("push", "Commit your memory edits and push them")
    push_cmd.add_argument("--book", choices=["team", "user", "all"], default="all")
    push_cmd.add_argument("-m", "--message", action="append", help="commit message; repeat for more paragraphs")
    push_cmd.add_argument("--human", action="store_true", help="the user asked for this change; commit as them")
    push_cmd.add_argument("--agent", default=None, help="who is pushing (default: main agent)")
    push_cmd.add_argument("--session", default=None, help="the agent session (default: $CADRI_SESSION_ID)")
    push_cmd.add_argument("--harness", default=None, help="the agent harness (default: $CADRI_HARNESS)")
    push_cmd.add_argument("--digest", default=None, help="the digest file a memory keeper reflected on")
    push_cmd.add_argument("--continue", dest="resume", action="store_true", help="finish a push after resolving a conflict")
    push_cmd.add_argument("--abort", action="store_true", help="give up on a conflicted push; commits stay local")

    ctx = child("context", "Print the memory context agents get at session start")
    ctx.add_argument("--harness", default=None)
    ctx.add_argument("--budget", type=int, default=context.BUDGET)

    child("status", "Show sync state, unpushed work, repairs, and waiting reflections")

    write = scoped("write", "Write one file through the edit API")
    write.add_argument("path")
    write.add_argument("--file", default=None, help="read content from this file instead of stdin")
    write.add_argument("-m", "--message", required=True, action="append", help="repeat for more paragraphs, like git")
    write.add_argument("--agent", default=None)
    write.add_argument("--session", default=None, help="id of the agent session writing this memory (default: $CADRI_SESSION_ID)")

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in {"--url", "--logout", "--no-browser"}:
        argv = ["login", *argv]
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except (CliError, auth.AuthFailure) as exc:
        err(f"cadri: {exc}")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
