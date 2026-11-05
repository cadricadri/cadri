#!/usr/bin/env python3
"""Compatibility shim — Claude hooks live under harnesses/claude/hooks/."""

from __future__ import annotations

import runpy
from pathlib import Path

runpy.run_path(
    str(Path(__file__).resolve().parent.parent / "harnesses" / "claude" / "hooks" / "memory_start.py"),
    run_name="__main__",
)
