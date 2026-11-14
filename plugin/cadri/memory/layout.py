"""The shape of a book: its root file, directory indexes, and the seeded team layout.

Every directory's index is `MEMORY.md` (no frontmatter). The book root `MEMORY.md` is
also what session start loads in full. Older books may still have `README.md` indexes;
readers accept either, writers always create `MEMORY.md`.
"""

from __future__ import annotations

import os
from pathlib import Path

from .books import Book

ROOT_FILE = "MEMORY.md"
INDEX_FILE = "MEMORY.md"
LEGACY_INDEX_FILE = "README.md"

# Allowed top-level directories in the team book. Subfolders under these are free-form
# (within the depth limit). The personal book is not constrained.
TEAM_AREAS: tuple[str, ...] = (
    "architecture",
    "components",
    "features",
    "incidents",
    "operations",
    "path-to-prod",
)

# Index text for an empty team book. Init does not publish this on its own.
TEAM_LAYOUT: tuple[tuple[str, str], ...] = (
    (
        ROOT_FILE,
        """# Team memory

What every agent working for this team should know up front. Keep this file short:
it is loaded into every session. Details live in the directories below.

## Where team knowledge lives

- [architecture](architecture/MEMORY.md) — design decisions and why
- [components](components/MEMORY.md) — one directory per service or module
- [features](features/MEMORY.md) — incremental features and why they exist
- [incidents](incidents/MEMORY.md) — failures grouped by pattern, and the lesson
- [operations](operations/MEMORY.md) — how to ship, debug, and respond
- [path-to-prod](path-to-prod/MEMORY.md) — CI/CD and how code reaches production

A fact that belongs to one component lives under `components/<name>/`, linked from that
directory's `MEMORY.md`. Do not copy it into this file. Preferences that are one person's
go in the personal book.
""",
    ),
    (
        "architecture/MEMORY.md",
        """# Architecture

Decisions that are hard to recover from the code: what the pieces are, and why they are
shaped this way. Point at components instead of restating how they work.

Typical files here: `component-map.md`, `design-decisions.md`. Subfolders are fine when a
topic needs room.
""",
    ),
    (
        "components/MEMORY.md",
        """# Components

One directory per service or module. When changing that part of the system, open its
`MEMORY.md` first.

Add one by writing `components/<name>/MEMORY.md` and linking it here. That index points at
how the component works and what has gone wrong in it.

Example shape (create these only when you have something to say):

```text
components/
├── MEMORY.md
├── auth-service/
│   ├── MEMORY.md
│   ├── how-it-works.md
│   └── gotchas.md
└── api-gateway/
    └── MEMORY.md
```
""",
    ),
    (
        "features/MEMORY.md",
        """# Features

Incremental product or platform features and their raison d'être: what shipped, why it
exists, and what a future change must not break. One directory or file per feature; name
it after the feature, not the date.

Example:

```text
features/
├── MEMORY.md
├── memory-books/
│   ├── MEMORY.md
│   └── why.md
└── session-capture.md
```
""",
    ),
    (
        "incidents/MEMORY.md",
        """# Incidents

Group failures that share a cause (`incidents/startup-death-spirals/…`), not one file per
date at this level. Say what broke, why, and what to do differently. Link the component it
belongs to.

Example:

```text
incidents/
├── MEMORY.md
└── startup-death-spirals/
    ├── MEMORY.md
    └── 2026-05-api-gateway.md
```
""",
    ),
    (
        "operations/MEMORY.md",
        """# Operations

How to debug a live problem, which signals matter, and recurring runbooks. When a
procedure is specific to one component, link that component instead of copying it.

Typical files: `common-alerts.md`, `debugging-runbooks.md`.
""",
    ),
    (
        "path-to-prod/MEMORY.md",
        """# Path to production

How code gets from a change to production: CI, review, deploy, rollback. Keep the pipeline
shape here; component-specific deploy quirks live under that component.

Typical files: `ci-cd.md`, `deployment-process.md`.
""",
    ),
)


def is_index(name: str) -> bool:
    return name in (INDEX_FILE, LEGACY_INDEX_FILE)


def has_lessons(root: Path) -> bool:
    """True when `root` contains a markdown file that is not a directory index.

    `MEMORY.md` and `README.md` are indexes. An index-only book is still waiting
    for its first lesson, matching the memories page.
    """
    if not root.is_dir():
        return False
    for _dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = [name for name in dirnames if name != ".git"]
        for name in filenames:
            if name.endswith(".md") and not is_index(name):
                return True
    return False


def index_path(directory: Path) -> Path | None:
    """The index file in `directory`, preferring MEMORY.md over a legacy README.md."""
    for name in (INDEX_FILE, LEGACY_INDEX_FILE):
        path = directory / name
        if path.is_file():
            return path
    return None


def root_file(book: Book) -> Path | None:
    """The book's always-loaded file: MEMORY.md, or README.md in books seeded before the rename."""
    return index_path(book.path)


def team_area(path: str) -> str | None:
    """The top-level directory of a team-book relative path, or None for a root file."""
    path = path.strip("/")
    if not path or "/" not in path and path.endswith(".md"):
        return None
    return path.split("/", 1)[0]
