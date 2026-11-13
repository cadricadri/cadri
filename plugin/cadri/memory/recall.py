"""Find memory files that look relevant to a prompt, locally and fast.

The prompt hook runs on every turn with a budget of a few hundred milliseconds and no
network, so this is keyword matching over a cached index of each book: file paths,
frontmatter descriptions, headings, and the opening text. It only has to point the
agent at a few files worth opening; the agent reads and judges them itself.
"""

from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from pathlib import Path

from ..state import write_text
from .books import Book

BODY_CHARS = 1500
MAX_FILES = 2000
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


@dataclass(frozen=True)
class Candidate:
    book: Book
    path: str
    description: str
    score: float


def words(text: str) -> list[str]:
    found = []
    for word in WORD.findall(text.lower()):
        if len(word) < 3 or word in STOPWORDS or word.isdigit():
            continue
        if len(word) > 4 and word.endswith("s") and not word.endswith("ss"):
            word = word[:-1]
        found.append(word)
    return found


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


def entry_for(file: Path, relative: str) -> dict:
    stat = file.stat()
    text = file.read_text(encoding="utf-8", errors="replace")
    body = strip_frontmatter(text)
    headings = " ".join(line.lstrip("#") for line in body.splitlines() if line.startswith("#"))
    fields = frontmatter(text)
    return {
        "mtime": stat.st_mtime,
        "size": stat.st_size,
        "description": describe(text),
        "path": sorted(set(words(relative.replace("/", " ").replace("-", " ").replace("_", " ")))),
        "head": sorted(set(words(headings + " " + fields.get("name", "")))),
        "desc": sorted(set(words(fields.get("description", "")))),
        "body": sorted(set(words(body[:BODY_CHARS]))),
    }


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


WEIGHTS = (("path", 3.0), ("desc", 2.5), ("head", 1.5), ("body", 1.0))
MIN_SCORE = 2.5


def search(books: list[Book], prompt: str, limit: int = 5, skip: set[str] | None = None) -> list[Candidate]:
    query = set(words(prompt))
    if not query:
        return []
    corpus: list[tuple[Book, str, dict]] = []
    for book in books:
        for relative, entry in build(book).items():
            if skip and relative in skip:
                continue
            corpus.append((book, relative, entry))
    if not corpus:
        return []
    frequency: dict[str, int] = {}
    for _, _, entry in corpus:
        seen = set().union(*(entry.get(field, []) for field, _ in WEIGHTS))
        for word in query & seen:
            frequency[word] = frequency.get(word, 0) + 1
    total = len(corpus)
    scored = []
    for book, relative, entry in corpus:
        score = 0.0
        for word in query:
            if word not in frequency:
                continue
            idf = math.log(1 + total / frequency[word])
            score += idf * sum(weight for field, weight in WEIGHTS if word in entry.get(field, ()))
        if score >= MIN_SCORE:
            scored.append(Candidate(book, relative, entry.get("description", ""), round(score, 2)))
    scored.sort(key=lambda candidate: (-candidate.score, candidate.path))
    return scored[:limit]
