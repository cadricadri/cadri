#!/usr/bin/env python3
"""Run the Cadri CLI from a plugin checkout or an installed copy."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from cadri.cli import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
