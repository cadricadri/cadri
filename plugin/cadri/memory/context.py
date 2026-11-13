"""The memory context printed at session start (and by `cadri memory context`).

In priority order, within a character budget: how to use memory (from the
cadri-memory skill), the status of both books, each book's MEMORY.md, and a tree of
every file with its description. Over budget, the tree shrinks first, then each
MEMORY.md is cut with a pointer to the full file.
"""

from __future__ import annotations

from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from . import recall
from .books import Book
from .layout import root_file
from .sync import Status

PLUGIN_ROOT = Path(__file__).resolve().parents[2]
SKILLS = PLUGIN_ROOT / "skills"
BUDGET = 9000
ROOT_FILE_SHARE = 3200
TREE_PER_DIR = (12, 6, 3, 0)


@dataclass
class Names:
    """How this harness names Cadri's skills and agent (Claude Code namespaces plugin ones)."""

    memory_skill: str = "cadri-memory"
    repair_skill: str = "cadri-memory-repair"
    keeper: str = "cadri-memory-keeper"


@dataclass
class Inputs:
    books: list[Book]
    statuses: dict[str, Status] = field(default_factory=dict)
    session_id: str | None = None
    notices: list[str] = field(default_factory=list)
    names: Names = field(default_factory=Names)


def skill_text(relative: str) -> str:
    try:
        return (SKILLS / relative).read_text(encoding="utf-8")
    except OSError:
        return ""


def reminder() -> str:
    """The short search reminder the prompt hook prints, from references/search.md."""
    text = skill_text("cadri-memory/references/search.md")
    start, end = text.find("<!-- reminder -->"), text.find("<!-- /reminder -->")
    if start < 0 or end < 0:
        return ""
    return text[start + len("<!-- reminder -->") : end].strip()


def fill(template: str, values: dict[str, str]) -> str:
    for key, value in values.items():
        template = template.replace("{{" + key + "}}", value)
    return template


def instructions(inputs: Inputs) -> str:
    paths = {book.scope: str(book.path) for book in inputs.books}
    return fill(
        skill_text("cadri-memory/references/session-start.md").strip(),
        {
            "team_path": paths.get("team", "not cloned"),
            "personal_path": paths.get("user", "not cloned"),
            "keeper": inputs.names.keeper,
            "memory_skill": inputs.names.memory_skill,
            "repair_skill": inputs.names.repair_skill,
        },
    )


def status_block(inputs: Inputs) -> str:
    lines = ["## Status"]
    if inputs.session_id:
        lines.append(f"- Session id: `{inputs.session_id}`")
    for book in inputs.books:
        status = inputs.statuses.get(book.scope)
        detail = f" — {status.detail}" if status and status.detail else ""
        lines.append(f"- {book.label.capitalize()} book: `{book.path}`{detail}")
        if status and status.conflicts:
            lines.append(f"  Conflicted files: {', '.join(status.conflicts)}")
    lines += [f"- {notice}" for notice in inputs.notices]
    return "\n".join(lines)


def root_text(book: Book, limit: int) -> str:
    path = root_file(book)
    heading = f"## {book.label.capitalize()} MEMORY.md"
    if path is None:
        return f"{heading}\n\n(empty — no MEMORY.md yet)"
    try:
        text = path.read_text(encoding="utf-8", errors="replace").strip()
    except OSError:
        return f"{heading}\n\n(unreadable: {path})"
    if path.name != "MEMORY.md":
        heading += f" (still named {path.name})"
    if len(text) > limit:
        text = text[:limit].rstrip() + f"\n\n… (cut; read the rest at {path})"
    return f"{heading}\n\n{text}"


def tree(book: Book, per_dir: int) -> str:
    entries = recall.build(book)
    if not entries:
        return ""
    grouped: dict[str, list[str]] = defaultdict(list)
    for relative in sorted(entries):
        directory = relative.rsplit("/", 1)[0] if "/" in relative else ""
        grouped[directory].append(relative)
    lines = [f"{book.label} — {book.path}"]
    for directory in sorted(grouped):
        files = grouped[directory]
        if directory == "":
            files = [name for name in files if name not in ("MEMORY.md", "README.md")]
        # Prefer MEMORY.md as the directory index; older books may still use README.md.
        index = None
        if directory:
            for name in (f"{directory}/MEMORY.md", f"{directory}/README.md"):
                if name in files or name in entries:
                    index = name
                    break
        ordered = sorted(files, key=lambda name: (name != index, name))
        cap = per_dir if per_dir > 0 else (1 if index in ordered else 0)
        shown = ordered[:cap]
        for relative in shown:
            description = entries[relative].get("description") or ""
            lines.append(f"  {relative}" + (f" — {description}" if description else ""))
        hidden = len(ordered) - len(shown)
        if hidden > 0:
            lines.append(f"  … {hidden} more in {directory or 'the root'}/")
    return "\n".join(lines)


def tree_block(books: list[Book], room: int) -> str:
    for per_dir in TREE_PER_DIR:
        body = "\n".join(filter(None, (tree(book, per_dir) for book in books)))
        if not body:
            return ""
        block = f"## What is in memory\n\n{body}"
        if len(block) <= room:
            return block
    return block[: max(0, room - 40)].rstrip() + "\n  … (tree cut; list the book directories to see more)"


def render(inputs: Inputs, budget: int = BUDGET) -> str:
    head = instructions(inputs)
    status = status_block(inputs)
    fixed = len(head) + len(status) + 8
    roots = [root_text(book, ROOT_FILE_SHARE) for book in inputs.books if book.path.is_dir()]
    room = budget - fixed - sum(len(part) for part in roots)
    if room < 600:
        share = max(800, (budget - fixed - 600) // max(1, len(roots)))
        roots = [root_text(book, share) for book in inputs.books if book.path.is_dir()]
        room = budget - fixed - sum(len(part) for part in roots)
    tree_part = tree_block([book for book in inputs.books if book.path.is_dir()], max(0, room))
    return "\n\n".join(part for part in (head, status, *roots, tree_part) if part).strip() + "\n"
