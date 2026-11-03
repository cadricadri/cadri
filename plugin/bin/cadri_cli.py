#!/usr/bin/env python3
"""Cadri CLI: login, init (discover + backfill + harness install), local helpers."""

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

from shared import cadri_auth  # noqa: E402
from shared.discover import detected_harnesses, discover_all  # noqa: E402
from shared.discover.opencode import export_session_json  # noqa: E402
from shared.discover.types import SessionRef  # noqa: E402
from shared.upload import post_transcript  # noqa: E402

BACKFILL_WINDOW_SECONDS = 30 * 24 * 60 * 60
BACKFILL_MAX_SESSIONS = 200
BACKFILL_MAX_BYTES = 24 * 1024 * 1024
REQUEST_TIMEOUT = 15
SLOW_DOWN_SECONDS = 5

HARNESS_INSTALL = {
    "claude_code": "harnesses/claude",
    "cursor": "harnesses/cursor",
    "codex": "harnesses/codex",
    "opencode": "harnesses/opencode",
}


class CliError(Exception):
    pass


def plugin_root() -> Path:
    return PLUGIN_ROOT


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
        raise CliError("not signed in; run cadri login or set CADRI_API_TOKEN")
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
        status, body = post_transcript(base, payload)
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


def install_harness(name: str) -> str:
    rel = HARNESS_INSTALL.get(name)
    if not rel:
        return f"{name}: unknown harness"
    source = plugin_root() / rel
    if not source.is_dir():
        return f"{name}: package missing at {source}"
    home = Path.home()
    if name == "claude_code":
        dest = home / ".claude" / "plugins" / "cadri"
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists() or dest.is_symlink():
            if dest.is_dir() and not dest.is_symlink():
                shutil.rmtree(dest)
            else:
                dest.unlink()
        shutil.copytree(source, dest)
        # Also drop skills into ~/.claude/skills and ~/.agents/skills
        for skills_root in (home / ".claude" / "skills", home / ".agents" / "skills"):
            skills_root.mkdir(parents=True, exist_ok=True)
            src_skills = source / "skills"
            if src_skills.is_dir():
                for skill in src_skills.iterdir():
                    target = skills_root / skill.name
                    if target.exists():
                        shutil.rmtree(target) if target.is_dir() else target.unlink()
                    shutil.copytree(skill, target)
        return f"{name}: installed plugin to {dest}"
    if name == "cursor":
        dest = home / ".cursor" / "cadri"
        dest.mkdir(parents=True, exist_ok=True)
        for item in source.iterdir():
            target = dest / item.name
            if target.exists():
                shutil.rmtree(target) if target.is_dir() else target.unlink()
            if item.is_dir():
                shutil.copytree(item, target)
            else:
                shutil.copy2(item, target)
        mcp_src = source / "mcp" / "cadri.json"
        if mcp_src.is_file():
            mcp_dir = home / ".cursor"
            mcp_dir.mkdir(parents=True, exist_ok=True)
            # Bake the effective server URL (hosted / staging / localhost) at install time.
            base = cadri_auth.server_url()
            text = mcp_src.read_text(encoding="utf-8").replace("${CADRI_URL}", base)
            (mcp_dir / "cadri-mcp.json").write_text(text, encoding="utf-8")
            dest_mcp = dest / "mcp" / "cadri.json"
            if dest_mcp.is_file():
                dest_mcp.write_text(text, encoding="utf-8")
        return f"{name}: installed files under {dest} (CADRI_URL={cadri_auth.server_url()})"
    if name == "codex":
        dest = home / ".codex" / "cadri"
        dest.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source, dest, dirs_exist_ok=True)
        return f"{name}: installed files under {dest}"
    if name == "opencode":
        dest = home / ".config" / "opencode" / "plugins" / "cadri"
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.exists():
            shutil.rmtree(dest)
        shutil.copytree(source, dest)
        return f"{name}: installed plugin to {dest}"
    return f"{name}: no installer"


def start_local_engine() -> None:
    repo = plugin_root().parent
    script = repo / "scripts" / "run"
    if not script.is_file():
        raise CliError(f"missing {script}; clone the cadri repo for localhost mode")
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


def cmd_init(args: argparse.Namespace) -> int:
    if args.local:
        os.environ.setdefault("CADRI_URL", "http://127.0.0.1:8080")
        if args.start_engine:
            start_local_engine()
            time.sleep(1)
    base = cadri_auth.server_url(args.url)
    ensure_auth(base, skip_login=args.local and not args.login)
    since = time.time() - BACKFILL_WINDOW_SECONDS
    print(f"Discovering harness sessions under {Path.home()}…")
    detected = detected_harnesses()
    print(f"Detected: {', '.join(detected) if detected else '(none)'}")
    summary = backfill(base, since, BACKFILL_MAX_SESSIONS, BACKFILL_MAX_BYTES)
    print(
        f"Backfill: uploaded {summary['uploaded']} sessions "
        f"({summary['bytes']} bytes); skipped {summary['skipped']}; errors {summary['errors']}"
    )
    if summary["by_harness"]:
        print("  " + ", ".join(f"{k} {v}" for k, v in summary["by_harness"].items()))
    print("Installing harness packages…")
    for name in detected or list(HARNESS_INSTALL):
        # Only install detected; if none detected, still install claude package as default.
        if not detected and name != "claude_code":
            continue
        if detected and name not in detected:
            continue
        print("  " + install_harness(name))
    print("Done. Open Cadri — the Transcripts tab should populate shortly.")
    print("Next: ask your agent to follow the memory seeding steps in the install docs.")
    return 0


def cmd_discover(_: argparse.Namespace) -> int:
    for ref in discover_all()[:50]:
        print(f"{ref.harness}\t{ref.thread_id}\t{ref.path}")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="cadri", description="Cadri agent bootstrap CLI")
    sub = parser.add_subparsers(dest="command", required=True)

    login = sub.add_parser("login", help="Sign in via browser device flow")
    login.add_argument("--url", default=None)
    login.add_argument("--logout", action="store_true")
    login.add_argument("--no-browser", action="store_true")
    login.set_defaults(func=cmd_login)

    init = sub.add_parser("init", help="Login (if needed), backfill transcripts, install harness plugins")
    init.add_argument("--url", default=None)
    init.add_argument("--local", action="store_true", help="Target localhost Cadri (no WorkOS by default)")
    init.add_argument("--login", action="store_true", help="Force device login even with --local")
    init.add_argument("--start-engine", action="store_true", help="With --local, start cadri-engine in background")
    init.set_defaults(func=cmd_init)

    discover = sub.add_parser("discover", help="List local harness sessions")
    discover.set_defaults(func=cmd_discover)

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
