"""Entry point for `cadri hook`: answer the harness, then do slow work detached.

A hook must never break the agent: any failure is logged to ~/.cadri/hooks.log and
the harness gets the empty answer it expects. Background jobs run in a forked child
after stdout is flushed and closed; CADRI_HOOK_FOREGROUND=1 runs them inline (tests).
"""

from __future__ import annotations

import json
import os
import sys
import time
import traceback

from ..state import cadri_home, log_path
from . import adapters, core
from .events import EVENTS


def log(message: str) -> None:
    try:
        path = log_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "a", encoding="utf-8") as handle:
            handle.write(f"{time.strftime('%Y-%m-%dT%H:%M:%S')} {message.rstrip()}\n")
    except OSError:
        pass


def parse(stdin: str) -> dict:
    if not stdin.strip():
        return {}
    data = json.loads(stdin)
    if not isinstance(data, dict):
        raise ValueError("hook input must be a JSON object")
    return data


def foreground() -> bool:
    return os.environ.get("CADRI_HOOK_FOREGROUND") == "1"


def tracing() -> bool:
    """One line per hook call in the hook log, for checking that a harness calls Cadri."""
    return os.environ.get("CADRI_HOOK_TRACE") == "1" or (cadri_home() / "trace-hooks").exists()


def detach() -> bool:
    """Fork; True in the child, which carries on with stdio sent to the hook log."""
    try:
        pid = os.fork()
    except OSError:
        return True
    if pid:
        return False
    os.setsid()
    try:
        path = log_path()
        path.parent.mkdir(parents=True, exist_ok=True)
        out = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    except OSError:
        out = os.open(os.devnull, os.O_WRONLY)
    devnull = os.open(os.devnull, os.O_RDONLY)
    os.dup2(devnull, 0)
    os.dup2(out, 1)
    os.dup2(out, 2)
    return True


def run_jobs(jobs: list, label: str) -> None:
    for job in jobs:
        try:
            job()
        except Exception:  # noqa: BLE001
            log(f"{label} background job failed:\n{traceback.format_exc()}")


def run(event: str | None, harness: str, stdin: str) -> int:
    adapter = adapters.get(harness)
    if adapter is None:
        log(f"unknown harness {harness!r}")
        return 0
    jobs: list = []
    answer = ""
    started = time.monotonic()
    note = "ignored"
    try:
        raw = parse(stdin)
        event = event or adapter.legacy_event(raw)
        if event in EVENTS and adapter.foreign(raw):
            note = "foreign"
        elif event in EVENTS:
            inp = adapter.parse(event, raw)
            note = f"session={inp.session_id} agent={inp.agent or '-'}"
            result = core.handle(inp, adapter)
            try:
                adapter.apply_env(inp, result.output)
            except OSError as exc:
                log(f"{harness} {event}: env not exported: {exc}")
            answer = adapter.render(event, result.output)
            jobs = result.background
        elif event is not None:
            answer = adapter.empty(event)
    except Exception:  # noqa: BLE001
        log(f"{harness} {event} hook failed:\n{traceback.format_exc()}")
        answer = adapter.empty(event) if event else ""
    if tracing():
        elapsed = (time.monotonic() - started) * 1000
        log(f"trace {harness} {event} {note} ms={elapsed:.0f} answer={len(answer)} jobs={len(jobs)}")
    if answer:
        sys.stdout.write(answer if answer.endswith("\n") else answer + "\n")
    sys.stdout.flush()
    if not jobs:
        return 0
    label = f"{harness} {event}"
    if foreground():
        run_jobs(jobs, label)
        return 0
    sys.stderr.flush()
    if detach():
        run_jobs(jobs, label)
        sys.stdout.flush()
        sys.stderr.flush()
        os._exit(0)
    return 0
