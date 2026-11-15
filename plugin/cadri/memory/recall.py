"""Local memory lookup for the session-start tree and the prompt hook.

The tree is a cached description of each file, refreshed when the file changes.
The prompt hook does not rank or choose files. It reads the markdown on disk and
returns lines that contain a word from the prompt, rare words first. The agent
reads those lines and searches further with grep.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path

from ..state import write_text
from .books import Book

MAX_FILES = 2000
HITS_PER_TERM = 3
MAX_TERMS = 6
SNIPPET_CHARS = 160
WORD = re.compile(r"[a-z0-9]+")
STOPWORDS = frozenset(
    """
    a about above after again all also am an and any are as at be because been before being below
    between both but by can could did do does doing done down during each few for from further get
    got had has have having he her here hers him his how i if in into is it its itself just let like
    make me more most my no nor not now of off on once only or other our out over own please same
    she should so some such than that the their them then there these they this those through to too
    under until up us use using very want was we were what when where which while who why will with
    would you your yours file files code thing things way need needs look see run show tell help new
    md readme memory
    """.split()
)


def frontmatter(text: str) -> dict[str, str]:
    if not text.startswith("---\n"):
        return {}
    end = text.find("\n---", 4)
    if end < 0:
        return {}
    fields = {}
    for line in text[4:end].splitlines():
        key, sep, value = line.partition(":")
        if sep and key.strip() and not key.startswith((" ", "\t")):
            fields[key.strip().lower()] = value.strip().strip("\"'")
    return fields


def strip_frontmatter(text: str) -> str:
    if text.startswith("---\n"):
        end = text.find("\n---", 4)
        if end >= 0:
            return text[end + 4 :].lstrip("\n")
    return text


def describe(text: str) -> str:
    """The frontmatter description, else the first heading or line."""
    fields = frontmatter(text)
    if fields.get("description"):
        return fields["description"][:200]
    for line in strip_frontmatter(text).splitlines():
        line = line.strip().lstrip("#").strip()
        if line:
            return line[:200]
    return ""


def entry_for(file: Path, _relative: str) -> dict:
    stat = file.stat()
    text = file.read_text(encoding="utf-8", errors="replace")
    return {"mtime": stat.st_mtime, "size": stat.st_size, "description": describe(text)}


def index_path(book: Book) -> Path:
    return book.sibling(".index.json")


def markdown_files(root: Path) -> list[Path]:
    found = []
    for file in sorted(root.rglob("*.md")):
        if ".git" in file.relative_to(root).parts:
            continue
        found.append(file)
        if len(found) >= MAX_FILES:
            break
    return found


def build(book: Book) -> dict[str, dict]:
    """The index of every markdown file in the book, refreshing only files that changed."""
    if not book.path.is_dir():
        return {}
    try:
        cached = json.loads(index_path(book).read_text(encoding="utf-8")).get("files", {})
    except (OSError, ValueError, AttributeError):
        cached = {}
    files: dict[str, dict] = {}
    changed = False
    for file in markdown_files(book.path):
        relative = file.relative_to(book.path).as_posix()
        try:
            stat = file.stat()
        except OSError:
            continue
        old = cached.get(relative)
        if isinstance(old, dict) and old.get("mtime") == stat.st_mtime and old.get("size") == stat.st_size:
            files[relative] = old
            continue
        try:
            files[relative] = entry_for(file, relative)
        except OSError:
            continue
        changed = True
    if changed or set(files) != set(cached):
        try:
            write_text(index_path(book), json.dumps({"files": files}))
        except OSError:
            pass
    return files


@dataclass(frozen=True)
class Hit:
    book: Book
    path: str
    line: int
    text: str


@dataclass(frozen=True)
class Mention:
    """Lines that contain `term`. `total` counts matches not already shown under a rarer word."""

    term: str
    total: int
    shown: tuple[Hit, ...]


def terms(text: str) -> list[str]:
    """Distinctive words, in order, with no stemming. Stemming would miss the word on disk."""
    found: list[str] = []
    seen: set[str] = set()
    for word in WORD.findall(text.lower()):
        if len(word) < 3 or word in STOPWORDS or word.isdigit() or word in seen:
            continue
        seen.add(word)
        found.append(word)
    return found


def mentions(books: list[Book], prompt: str, per_term: int = HITS_PER_TERM, max_terms: int = MAX_TERMS) -> list[Mention]:
    """Lines in the books that contain a word from `prompt`.

    Scans the files themselves, including past the opening. Words with fewer
    matches come first, and a word with no match is left out. A line is shown
    under the rarest word that contains it.
    """
    query = terms(prompt)
    if not query or per_term < 1 or max_terms < 1:
        return []
    patterns = {term: re.compile(rf"\b{re.escape(term)}s?\b", re.IGNORECASE) for term in query}
    found: dict[str, list[Hit]] = {term: [] for term in query}
    for book in books:
        if not book.path.is_dir():
            continue
        for file in markdown_files(book.path):
            relative = file.relative_to(book.path).as_posix()
            try:
                lines = file.read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                continue
            for number, line in enumerate(lines, start=1):
                for term in query:
                    if patterns[term].search(line):
                        found[term].append(Hit(book, relative, number, line.strip()))
    order = sorted((term for term in query if found[term]), key=lambda term: (len(found[term]), term))[:max_terms]
    used: set[tuple[str, str, int]] = set()
    results: list[Mention] = []
    for term in order:
        fresh: list[Hit] = []
        for hit in found[term]:
            key = (str(hit.book.path), hit.path, hit.line)
            if key not in used:
                fresh.append(hit)
        if not fresh:
            continue
        for hit in fresh[:per_term]:
            used.add((str(hit.book.path), hit.path, hit.line))
        results.append(Mention(term, len(fresh), tuple(fresh[:per_term])))
    return results


def status_line(count: int) -> str:
    """The sentence the user sees when a search has something to show."""
    word = "thing" if count == 1 else "things"
    return f"Cadri remembered {count} {word} that may help"


def matched_files(found: list[Mention]) -> int:
    return len({(str(hit.book.path), hit.path) for mention in found for hit in mention.shown})


def render(books: list[Book], prompt: str) -> str:
    """The search tool's text. Empty when nothing matched, so the caller shows nothing."""
    found = mentions(books, prompt)
    count = matched_files(found)
    if count == 0:
        return ""
    lines = [status_line(count), ""]
    for mention in found:
        lines.append(f"- {mention.term}")
        for hit in mention.shown:
            text = hit.text if len(hit.text) <= SNIPPET_CHARS else hit.text[:SNIPPET_CHARS].rstrip() + "…"
            lines.append(f"  {hit.book.path / hit.path}:{hit.line}: {text}")
        extra = mention.total - len(mention.shown)
        if extra:
            lines.append(f"  … {extra} more for {mention.term}")
    return "\n".join(lines)
