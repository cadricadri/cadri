"""Compatibility re-export — prefer shared.cadri_auth."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from shared.cadri_auth import *  # noqa: F403
