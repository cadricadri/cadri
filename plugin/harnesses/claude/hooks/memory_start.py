#!/usr/bin/env python3
"""SessionStart: touch the memory heartbeat and start the background watcher."""

from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))


def main() -> int:
    try:
        from shared import memory

        memory.touch_heartbeat()
        memory.detach_watcher(ROOT / "bin" / "cadri_cli.py")
    except Exception as exc:  # noqa: BLE001
        print(f"cadri: memory watcher not started: {exc}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
