"""Shared discovery types."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class SessionRef:
    harness: str
    thread_id: str
    path: Path
    updated_at: float
