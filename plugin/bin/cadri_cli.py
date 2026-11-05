#!/usr/bin/env python3
"""Cadri CLI: login, init (hooks + backfill + memory clone), agent hooks, memory books."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import webbrowser
from pathlib import Path

PLUGIN_ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN_ROOT))

from shared import cadri_auth, hooks, install, memory  # noqa: E402
from shared.discover import detected_harnesses, discover_all  # noqa: E402
from shared.discover.opencode import export_session_json  # noqa: E402
from shared.discover.types import SessionRef  # noqa: E402
from shared.upload import post_transcript  # noqa: E402

BACKFILL_WINDOW_SECONDS = 30 * 24 * 60 * 60
BACKFILL_MAX_SESSIONS = 200
BACKFILL_MAX_BYTES = 24 * 1024 * 1024
REQUEST_TIMEOUT = 15
SLOW_DOWN_SECONDS = 5
HARNESS_NAMES = {
    "claude_code": "Claude Code",
    "cursor": "Cursor",
    "codex": "Codex",
    "opencode": "OpenCode",
}


class CliError(Exception):
    pass


def plugin_root() -> Path:
    return PLUGIN_ROOT


def version() -> str:
    try:
        return (PLUGIN_ROOT / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "source"


def cmd_login(args: argparse.Namespace) -> int:
    base = cadri_auth.server_url(args.url)
    if args.logout:
        cadri_auth.update_entry(base, None)
        print(f"Signed out of {base}")
        return 0
    device = start_device(base)
    url = device.get("verification_uri_complete") or device.get("verification_uri")
    print(f"To sign in to {base}, open:\n\n  {url}\n\nand confirm the code: {device.get('user_code')}\n")
    if not args.no_browser and isinstance(url, str):
        try:
            webbrowser.open(url)
        except webbrowser.Error:
            pass
    print("Waiting for confirmation...", flush=True)
    cadri_auth.update_entry(base, poll_device(base, device))
    print(f"Signed in. Credentials saved to {cadri_auth.credentials_path()}")
    return 0


def start_device(base: str) -> dict:
    status, body = cadri_auth.post_json(f"{base}/auth/device/start", {}, REQUEST_TIMEOUT)
    if status == 404:
        raise CliError(f"{base} does not support sign-in; set CADRI_API_TOKEN instead")
    if status != 200 or not isinstance(body.get("device_code"), str):
        raise CliError(f"could not start sign-in ({status}): {body.get('error', 'bad response')}")
    return body


def seconds(value: object, default: float) -> float:
    return float(value) if isinstance(value, (int, float)) and value >= 0 else default


def poll_device(base: str, device: dict) -> dict:
    interval = max(1.0, seconds(device.get("interval"), 5))
    deadline = time.monotonic() + seconds(device.get("expires_in"), 600)
    while time.monotonic() < deadline:
        time.sleep(interval)
        status, body = cadri_auth.post_json(
            f"{base}/auth/device/token",
            {"device_code": device["device_code"]},
            REQUEST_TIMEOUT,
        )
        error = body.get("error")
        if status == 200:
            entry = cadri_auth.token_entry(body)
            if entry is None:
                raise CliError("server returned an invalid token response")
            return entry
        if status == 428:
            continue
        if status == 429:
            interval += SLOW_DOWN_SECONDS
            continue
        if error == "expired_token":
            raise CliError("the code expired; run cadri login again")
        if error == "access_denied":
            raise CliError("sign-in was denied")
        raise CliError(f"unexpected response ({status}): {error or 'no error message'}")
    raise CliError("the code expired; run cadri login again")


def ensure_auth(base: str, skip_login: bool) -> None:
    if os.environ.get("CADRI_API_TOKEN"):
        return
    entry = cadri_auth.load_credentials().get(base)
    if isinstance(entry, dict) and entry.get("access_token"):
        return
    if skip_login:
        return
    print("No credentials found; starting browser sign-in…")
    cmd_login(argparse.Namespace(url=base, logout=False, no_browser=False))


def read_session_content(ref: SessionRef) -> str:
    if ref.harness == "opencode":
        return export_session_json(ref.path, ref.thread_id)
    try:
        return ref.path.read_text(encoding="utf-8", errors="replace")
    except OSError as exc:
        raise CliError(f"could not read {ref.path}: {exc}") from exc


def backfill(base: str, since: float, max_sessions: int, max_bytes: int) -> dict:
    sessions = [
        ref
        for ref in discover_all()
        if ref.updated_at >= since
    ][:max_sessions]
    uploaded = 0
    skipped = 0
    errors = 0
    bytes_used = 0
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
            "cwd": ref.cwd,
            "hook_event": "backfill",
            "harness": ref.harness,
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ref.updated_at)),
        }
        try:
            status, body = post_transcript(base, payload)
        except (OSError, ValueError) as exc:
            raise CliError(f"could not reach {base}: {exc}") from exc
        if status in (200, 201):
            uploaded += 1
            bytes_used += size
            by_harness[ref.harness] = by_harness.get(ref.harness, 0) + 1
        elif status == 403 and body.get("error") == "no_organization":
            raise CliError(
                "create your team in the Cadri browser app, then re-run cadri init"
            )
        elif status in (401, 403):
            raise CliError("auth failed; run cadri login or set CADRI_API_TOKEN")
        else:
            print(
                f"cadri: upload {ref.harness}/{ref.thread_id} failed ({status})",
                file=sys.stderr,
            )
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
    repo = plugin_root().parent
    script = repo / "scripts" / "run"
    if not script.is_file():
        raise CliError(f"missing {script}; run ./plugin/bin/cadri init --local --start-engine from a cadri checkout")
    log = Path.home() / ".cadri" / "engine.log"
    log.parent.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["CADRI_BIND"] = "127.0.0.1:8080"
    # Engine-only: run cargo in background via nohup of a small helper.
    engine_dir = repo / "engine"
    cmd = [
        "bash",
        "-lc",
        f'cd "{engine_dir}" && CADRI_BIND=127.0.0.1:8080 CADRI_DB="{Path.home() / ".cadri" / "transcripts.db"}" '
        f'nohup cargo run --bin cadri-engine > "{log}" 2>&1 & echo $!',
    ]
    proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
    print(f"Started local engine (log {log}). pid={proc.stdout.strip() or 'unknown'}")


def git_ready() -> bool:
    """Whether git runs. On a fresh Mac /usr/bin/git is a stub that asks to install the Command Line Tools."""
    path = shutil.which("git")
    if path is None:
        return False
    if sys.platform == "darwin" and Path(path) == Path("/usr/bin/git"):
        return subprocess.run(["xcode-select", "-p"], capture_output=True, check=False).returncode == 0
    return True


def install_hooks(harnesses: list[str]) -> list[str]:
    install.remove_legacy(Path.home())
    lines = []
    for name in harnesses:
        try:
            lines.append(install.install_harness(name))
        except (install.InstallError, OSError) as exc:
            lines.append(f"{HARNESS_NAMES[name]}: not installed: {exc}")
    for root in install.install_skills(Path.home(), harnesses):
        lines.append(f"Skills: {root}")
    return lines


def clone_books(url: str | None) -> str:
    if not git_ready():
        return (
            "Memory: skipped, git is not installed. Run `xcode-select --install`, "
            "then `cadri memory clone` and `cadri memory clone --scope user`."
        )
    started = time.monotonic()
    paths = []
    try:
        for scope in ("team", "user"):
            book = memory.open_book(scope, url)
            memory.ensure_clone(book)
            paths.append(str(book.path))
    except memory.MemoryFailure as exc:
        return f"Memory: not cloned ({exc}); run `cadri memory clone` later"
    return f"Memory: cloned in {time.monotonic() - started:.1f}s to {', '.join(paths)}"


def cmd_init(args: argparse.Namespace) -> int:
    if args.local:
        os.environ.setdefault("CADRI_URL", "http://127.0.0.1:8080")
        if args.start_engine:
            start_local_engine()
            time.sleep(1)
    # init picks the server afresh; the saved URL is what it leaves behind for hooks.
    base = cadri_auth.normalize_url(args.url or os.environ.get("CADRI_URL") or cadri_auth.DEFAULT_URL)
    os.environ["CADRI_URL"] = base
    ensure_auth(base, skip_login=args.local and not args.login)
    cadri_auth.save_url(base)
    harnesses = sorted(
        set(install.installed_harnesses()) | set(detected_harnesses()),
        key=list(HARNESS_NAMES).index,
    )
    found = ", ".join(HARNESS_NAMES[name] for name in harnesses)
    print(f"Agents on this machine: {found or 'none found'}")
    print("Installing capture hooks…")
    for line in install_hooks(harnesses):
        print(f"  {line}")
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
    print("Done. New sessions upload when each agent turn finishes.")
    print("Next: load the cadri-memory-seed skill to turn these sessions into memories.")
    return 0


def cmd_uninstall(_: argparse.Namespace) -> int:
    try:
        removed = install.uninstall_all()
    except install.InstallError as exc:
        raise CliError(str(exc)) from exc
    print(f"Removed: {', '.join(removed) if removed else 'no Cadri hooks were installed'}")
    print("Skills removed. Credentials stay in ~/.config/cadri; run `cadri login --logout` to remove them.")
    return 0


def cmd_hook(args: argparse.Namespace) -> int:
    return hooks.run(args.harness, sys.stdin.read())


def cmd_discover(args: argparse.Namespace) -> int:
    since = time.time() - args.days * 24 * 60 * 60
    refs = [ref for ref in discover_all() if ref.updated_at >= since][: args.limit]
    if args.json:
        rows = [
            {
                "harness": ref.harness,
                "thread_id": ref.thread_id,
                "updated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(ref.updated_at)),
                "cwd": ref.cwd,
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


def cmd_memory(args: argparse.Namespace) -> int:
    action = args.memory_command
    try:
        if action == "watch":
            if args.detach:
                memory.detach_watcher(Path(__file__).resolve())
                return 0
            return memory.watch(args.url)
        book = memory.open_book(args.scope, args.url)
        if action == "clone":
            memory.ensure_clone(book)
            if memory.sync(book, quiet=True):
                return 1
            print(book.path)
            return 0
        if action == "path":
            memory.ensure_clone(book)
            print(book.path)
            return 0
        if action == "sync":
            return memory.sync(book, quiet=args.quiet)
        if action == "write":
            if args.file:
                try:
                    content = Path(args.file).read_text(encoding="utf-8")
                except OSError as exc:
                    raise CliError(f"could not read {args.file}: {exc}") from exc
            else:
                content = sys.stdin.read()
            return memory.write(book, args.path, content, "\n\n".join(args.message), args.agent)
    except memory.MemoryFailure as exc:
        print(f"cadri: {exc}", file=sys.stderr)
        return exc.code
    raise CliError(f"unknown memory command {action}")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="cadri", description="Cadri agent bootstrap CLI")
    parser.add_argument("--version", action="version", version=f"cadri {version()}")
    sub = parser.add_subparsers(dest="command", required=True)

    login = sub.add_parser("login", help="Sign in via browser device flow")
    login.add_argument("--url", default=None)
    login.add_argument("--logout", action="store_true")
    login.add_argument("--no-browser", action="store_true")
    login.set_defaults(func=cmd_login)

    init = sub.add_parser("init", help="Sign in (if needed), install capture hooks, upload recent sessions")
    init.add_argument("--url", default=None)
    init.add_argument("--local", action="store_true", help="Target localhost Cadri (no WorkOS by default)")
    init.add_argument("--login", action="store_true", help="Force device login even with --local")
    init.add_argument("--start-engine", action="store_true", help="With --local, start cadri-engine in background")
    init.set_defaults(func=cmd_init)

    uninstall = sub.add_parser("uninstall", help="Remove Cadri's hooks, OpenCode plugin, and skills")
    uninstall.set_defaults(func=cmd_uninstall)

    hook = sub.add_parser("hook", help="Run by agent hooks: upload the session named on stdin")
    hook.add_argument("harness", choices=[*hooks.HARNESSES, *hooks.ALIASES])
    hook.set_defaults(func=cmd_hook)

    discover = sub.add_parser("discover", help="List local agent sessions, newest first")
    discover.add_argument("--json", action="store_true")
    discover.add_argument("--limit", type=int, default=50)
    discover.add_argument("--days", type=float, default=30)
    discover.set_defaults(func=cmd_discover)

    show = sub.add_parser("show", help="Print one local session transcript")
    show.add_argument("thread_id")
    show.set_defaults(func=cmd_show)

    mem = sub.add_parser("memory", help="Shared clone of the team and personal memory books")
    mem_sub = mem.add_subparsers(dest="memory_command", required=True)

    def scoped(name: str, help_text: str) -> argparse.ArgumentParser:
        child = mem_sub.add_parser(name, help=help_text)
        child.add_argument("--scope", choices=["team", "user"], default="team")
        child.add_argument("--url", default=None)
        child.set_defaults(func=cmd_memory)
        return child

    scoped("clone", "Clone the book if missing, otherwise sync; print the path")
    scoped("path", "Print the clone path (cloning first if missing)")
    scoped("sync", "Fast-forward the clone to the server tip").add_argument("--quiet", action="store_true")
    write = scoped("write", "Write one file through the edit API")
    write.add_argument("path")
    write.add_argument("--file", default=None, help="read content from this file instead of stdin")
    write.add_argument("-m", "--message", required=True, action="append", help="repeat for more paragraphs, like git")
    write.add_argument("--agent", default=None)
    watch = mem_sub.add_parser("watch", help="Keep clones within a minute of the server")
    watch.add_argument("--detach", action="store_true")
    watch.add_argument("--url", default=None)
    watch.set_defaults(func=cmd_memory)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    # Allow `python bin/cadri_cli.py --no-browser` via cadri-login shim that only
    # forwards argv after injecting "login" when the first arg is a login flag.
    argv = list(sys.argv[1:] if argv is None else argv)
    if argv and argv[0] in {"--url", "--logout", "--no-browser"}:
        argv = ["login", *argv]
    args = parser.parse_args(argv)
    try:
        return int(args.func(args))
    except CliError as exc:
        print(f"cadri: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
