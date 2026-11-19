"""Cursor canvases, captured the way plans are.

The transcript names the file and the session. The file under
`~/.cursor/projects/<workspace>/canvases/` is the current document, including
edits made after the write. The session page stores a markdown reading, because
the TSX only renders inside Cursor.
"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

BODY_LIMIT = 24_000
WRITE_TOOLS = {"write", "createfile"}
PATCH_TOOLS = {"strreplace", "strreplaceeditor"}


def extract(content: str) -> list[dict]:
    """Canvases named in a transcript. A later write of the same file replaces the earlier draft."""
    found: dict[str, dict] = {}
    order: list[str] = []

    def add(canvas: dict) -> None:
        slug = canvas["slug"]
        if slug not in found:
            order.append(slug)
        found[slug] = canvas

    for value in _documents(content):
        _walk(value, add)
    return [found[slug] for slug in order]


def overlay(canvases: list[dict], home: Path) -> list[dict]:
    """Replace a draft with the canvas file when the file has moved on."""
    updated = []
    for canvas in canvases:
        text = _read(home, canvas.get("slug") or "")
        projected = project(text) if text is not None else None
        if projected and projected["body"].strip() != (canvas.get("body") or "").strip():
            revised = dict(canvas)
            revised["title"] = projected["title"]
            revised["body"] = clip_body(projected["body"])
            revised["source"] = "file"
            updated.append(revised)
        elif canvas.get("body"):
            updated.append(canvas)
    return updated


def refresh(content: str, home: Path) -> str:
    """Put the current canvas file into the uploaded transcript so the engine parses it.

    The transcript is not modified on disk. A write that was later edited, including
    by StrReplace, carries the file text instead of the original draft.
    """
    if ".canvas.tsx" not in content:
        return content
    trailing = content.endswith("\n")
    decoded: list[object] = []
    for line in content.splitlines():
        if not line.lstrip().startswith("{"):
            decoded.append(line)
            continue
        try:
            decoded.append(json.loads(line))
        except json.JSONDecodeError:
            decoded.append(line)
    writes: dict[str, list[dict]] = {}
    paths: dict[str, str] = {}
    for value in decoded:
        if isinstance(value, (dict, list)):
            _collect(value, writes, paths)
    for slug, path in paths.items():
        text = _read(home, slug)
        if text is None:
            continue
        targets = writes.get(slug) or []
        if targets:
            _set_body(targets[-1], text)
            continue
        decoded.append(_synthetic(path, text))
        trailing = True
    rendered = [value if isinstance(value, str) else json.dumps(value, ensure_ascii=False) for value in decoded]
    text = "\n".join(rendered)
    if trailing:
        text += "\n"
    return text


def select_new(canvases: list[dict], seen: list[str]) -> tuple[list[dict], list[str]]:
    known = set(seen)
    fresh: list[dict] = []
    hashes = list(seen)
    for canvas in canvases:
        digest = hash_canvas(canvas)
        if digest in known:
            continue
        stored = dict(canvas)
        stored["body"] = clip_body(canvas.get("body") or "")
        fresh.append(stored)
        hashes.append(digest)
        known.add(digest)
    return fresh, hashes


def absorb_files(canvases: list[dict], harness: str | None, session: str | None) -> list[dict]:
    revised = overlay(canvases, Path.home())
    if harness and session:
        from .state import Session

        record = Session(harness, session)
        seen = list(record.load().get("seen_canvas_hashes") or [])
        _, hashes = select_new(revised, seen)
        if hashes != seen:
            record.update(seen_canvas_hashes=hashes)
    stored = []
    for canvas in revised:
        copied = dict(canvas)
        copied["body"] = clip_body(canvas.get("body") or "")
        stored.append(copied)
    return stored


def hash_canvas(canvas: dict) -> str:
    text = f"{canvas.get('slug', '')}\n{canvas.get('body', '')}"
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def clip_body(body: str) -> str:
    if len(body) <= BODY_LIMIT:
        return body
    return body[:BODY_LIMIT].rstrip() + "\n\n…(canvas truncated)"


def project(source: str) -> dict | None:
    """Markdown for the parts of a canvas the session page can show."""
    title = ""
    blocks: list[str] = []
    index = 0
    while True:
        tag = _next_tag(source, index)
        if tag is None:
            break
        name, attrs, inner, open_end, end, self_closing = tag
        if name in {"Stack", "Grid", "div", "Fragment"}:
            index = end if self_closing else open_end
            continue
        index = end
        if name == "H1":
            text = _plain(inner)
            if not text:
                continue
            if not title:
                title = text
            else:
                blocks.append(f"# {text}")
        elif name == "H2":
            _heading(blocks, "##", inner)
        elif name == "H3":
            _heading(blocks, "###", inner)
        elif name == "Text":
            text = _plain(inner)
            if text:
                blocks.append(text)
        elif name == "Callout":
            text = _plain(inner)
            heading = _attr(attrs, "title")
            if not text and not heading:
                continue
            lines = []
            if heading:
                lines.append(f"> **{heading}**")
                lines.append(">")
            for line in text.split("\n"):
                line = line.strip()
                lines.append(">" if not line else f"> {line}")
            blocks.append("\n".join(lines))
        elif name == "Stat":
            value = _attr(attrs, "value") or ""
            label = _attr(attrs, "label") or ""
            if value or label:
                blocks.append(f"**{value}** — {label}")
        elif name == "Table":
            table = _markdown_table(attrs)
            if table:
                blocks.append(table)
        elif name == "BarChart":
            chart = _markdown_chart(attrs)
            if chart:
                blocks.append(chart)
    if not title and not blocks:
        return None
    body = ""
    if title:
        body = f"# {title}\n\n"
    body = (body + "\n\n".join(blocks)).strip()
    if not body:
        return None
    return {"title": title or "Canvas", "body": body}


def _documents(content: str) -> list[object]:
    text = content.lstrip()
    if text.startswith("{") and not _is_jsonl(text):
        try:
            document = json.loads(text)
        except json.JSONDecodeError:
            return []
        return [document] if isinstance(document, (dict, list)) else []
    found = []
    for line in content.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, (dict, list)):
            found.append(value)
    return found


def _is_jsonl(text: str) -> bool:
    lines = text.splitlines()
    return len(lines) > 1 and lines[1].lstrip().startswith("{")


def _walk(value: object, add) -> None:
    if isinstance(value, list):
        for item in value:
            _walk(item, add)
        return
    if not isinstance(value, dict):
        return
    name, raw = _tool_call(value)
    if name:
        for canvas in _from_call(name, raw):
            add(canvas)
    for child in value.values():
        _walk(child, add)


def _from_call(name: str, raw: object) -> list[dict]:
    key = _tool_key(name)
    data = _object(raw)
    if not data:
        return []
    path = _field(data, "path", "file_path", "filePath", "filepath")
    if not path or not _is_canvas_path(path):
        return []
    slug = _slug(path)
    if key in WRITE_TOOLS:
        body = _field(data, "contents", "content", "file_text") or ""
        projected = project(body)
        if not projected:
            return []
        return [
            {
                "slug": slug,
                "title": projected["title"],
                "body": projected["body"],
                "source": name,
            }
        ]
    if key in PATCH_TOOLS:
        return [{"slug": slug, "title": _slug_title(slug), "body": "", "source": name}]
    return []


def _collect(value: object, writes: dict[str, list[dict]], paths: dict[str, str]) -> None:
    if isinstance(value, list):
        for item in value:
            _collect(item, writes, paths)
        return
    if not isinstance(value, dict):
        return
    name, raw = _tool_call(value)
    if name and isinstance(raw, dict):
        path = _field(raw, "path", "file_path", "filePath", "filepath")
        if path and _is_canvas_path(path):
            slug = _slug(path)
            paths.setdefault(slug, path)
            if _tool_key(name) in WRITE_TOOLS:
                writes.setdefault(slug, []).append(raw)
    for child in value.values():
        _collect(child, writes, paths)


def _set_body(raw: dict, text: str) -> None:
    if "content" in raw and "contents" not in raw:
        raw["content"] = text
    else:
        raw["contents"] = text


def _synthetic(path: str, text: str) -> dict:
    return {
        "role": "assistant",
        "message": {
            "content": [
                {
                    "type": "tool_use",
                    "name": "Write",
                    "input": {"path": path, "contents": text},
                }
            ]
        },
    }


def _read(home: Path, slug: str) -> str | None:
    if not slug or "/" in slug or "\\" in slug or slug.startswith("."):
        return None
    root = home / ".cursor" / "projects"
    if not root.is_dir():
        return None
    matches = [path for path in root.glob(f"*/canvases/{slug}.canvas.tsx") if path.is_file()]
    if not matches:
        return None
    newest = max(matches, key=lambda path: path.stat().st_mtime)
    try:
        return newest.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None


def _is_canvas_path(path: str) -> bool:
    lowered = path.replace("\\", "/").lower()
    return "/canvases/" in lowered and lowered.endswith(".canvas.tsx")


def _slug(path: str) -> str:
    name = path.replace("\\", "/").rsplit("/", 1)[-1]
    if name.lower().endswith(".canvas.tsx"):
        name = name[: -len(".canvas.tsx")]
    return name


def _slug_title(slug: str) -> str:
    title = re.sub(r"[_-]+", " ", slug).strip()
    return title or "Canvas"


def _tool_call(value: dict) -> tuple[str | None, object]:
    kind = value.get("type")
    if kind in {"tool_use", "server_tool_use"} and isinstance(value.get("name"), str):
        return value["name"], value.get("input")
    if kind == "tool" and isinstance(value.get("tool") or value.get("name"), str):
        state = value.get("state")
        tool_input = state.get("input") if isinstance(state, dict) else None
        if tool_input is None:
            tool_input = value.get("input")
        return str(value.get("tool") or value.get("name")), tool_input
    if kind in {"function_call", "custom_tool_call"} and isinstance(value.get("name"), str):
        return value["name"], value.get("arguments", value.get("input"))
    return None, None


def _object(raw: object) -> dict | None:
    if isinstance(raw, dict):
        return raw
    if isinstance(raw, str):
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            return None
        return parsed if isinstance(parsed, dict) else None
    return None


def _field(data: dict | None, *keys: str) -> str | None:
    if not data:
        return None
    for key in keys:
        value = data.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def _tool_key(name: str) -> str:
    return "".join(ch for ch in name.lower() if ch.isalnum())


def _heading(blocks: list[str], marks: str, inner: str) -> None:
    text = _plain(inner)
    if text:
        blocks.append(f"{marks} {text}")


def _next_tag(source: str, start: int) -> tuple[str, str, str, int, int, bool] | None:
    index = start
    while index < len(source):
        if source[index] != "<":
            index += 1
            continue
        after = source[index + 1 :]
        name = _jsx_name(after)
        if not name or after.startswith("/") or after.startswith("!"):
            index += 1
            continue
        attr_start = index + 1 + len(name)
        opened = _scan_open(source[attr_start:])
        if opened is None:
            index += 1
            continue
        attrs, open_len, self_closing = opened
        open_end = attr_start + open_len
        if self_closing:
            return name, attrs, "", open_end, open_end, True
        closed = _matching_close(source, open_end, name)
        if closed is None:
            index += 1
            continue
        inner, end = closed
        return name, attrs, inner, open_end, end, False
    return None


def _jsx_name(source: str) -> str:
    match = re.match(r"[A-Za-z_][A-Za-z0-9_]*", source)
    return match.group(0) if match else ""


def _scan_open(source: str) -> tuple[str, int, bool] | None:
    quote = None
    escaped = False
    for index, char in enumerate(source):
        if quote:
            if escaped:
                escaped = False
                continue
            if char == "\\":
                escaped = True
                continue
            if char == quote:
                quote = None
            continue
        if char in {'"', "'"}:
            quote = char
            continue
        if source.startswith("/>", index):
            return source[:index], index + 2, True
        if char == ">":
            return source[:index], index + 1, False
    return None


def _matching_close(source: str, start: int, name: str) -> tuple[str, int] | None:
    open_tag = f"<{name}"
    close_tag = f"</{name}>"
    depth = 1
    index = start
    while index < len(source):
        if source.startswith(close_tag, index):
            depth -= 1
            if depth == 0:
                return source[start:index], index + len(close_tag)
            index += len(close_tag)
            continue
        if source.startswith(open_tag, index) and _tag_boundary(source, index + len(open_tag)):
            depth += 1
            index += len(open_tag)
            continue
        index += 1
    return None


def _tag_boundary(source: str, index: int) -> bool:
    if index >= len(source):
        return False
    char = source[index]
    return char.isspace() or char in {">", "/"}


def _plain(inner: str) -> str:
    stripped = re.sub(r"\{[^{}]*\}", "", inner)
    stripped = re.sub(r"<[^>]+>", "", stripped)
    return " ".join(stripped.split())


def _attr(attrs: str, name: str) -> str | None:
    quoted = re.search(rf"\b{name}\s*=\s*([\"'])(.*?)\1", attrs)
    if quoted:
        return quoted.group(2)
    braced = re.search(rf"\b{name}\s*=\s*\{{", attrs)
    if not braced:
        return None
    expr = _brace(attrs, braced.end() - 1)
    if expr is None:
        return None
    inner = expr.strip()
    parsed = _parse_js(inner)
    if isinstance(parsed, str):
        return parsed
    return " ".join(inner.split()) or None


def _brace(source: str, start: int) -> str | None:
    if start >= len(source) or source[start] != "{":
        return None
    depth = 0
    quote = None
    escaped = False
    for index in range(start, len(source)):
        char = source[index]
        if quote:
            if escaped:
                escaped = False
                continue
            if char == "\\":
                escaped = True
                continue
            if char == quote:
                quote = None
            continue
        if char in {'"', "'", "`"}:
            quote = char
            continue
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start + 1 : index]
    return None


def _markdown_table(attrs: str) -> str | None:
    headers = _js_strings(_attr_expr(attrs, "headers"))
    rows = _js_rows(_attr_expr(attrs, "rows"))
    if not headers:
        return None
    return _render_table(headers, rows or [])


def _markdown_chart(attrs: str) -> str | None:
    categories = _js_strings(_attr_expr(attrs, "categories"))
    series = _js_series(_attr_expr(attrs, "series"))
    if not categories or not series:
        return None
    rows = [[name, *data] for name, data in series]
    return _render_table([""] + categories, rows)


def _render_table(headers: list[str], rows: list[list[str]]) -> str:
    width = len(headers)

    def line(cells: list[str]) -> str:
        padded = [cell.replace("|", "\\|") for cell in cells]
        padded.extend("" for _ in range(width - len(padded)))
        return "| " + " | ".join(padded[:width]) + " |"

    out = [line(headers), "| " + " | ".join(["---"] * width) + " |"]
    out.extend(line(row) for row in rows)
    return "\n".join(out)


def _attr_expr(attrs: str, name: str) -> str | None:
    match = re.search(rf"\b{name}\s*=\s*\{{", attrs)
    if not match:
        return None
    return _brace(attrs, match.end() - 1)


def _js_strings(expr: str | None) -> list[str] | None:
    parsed = _parse_js(expr) if expr else None
    if not isinstance(parsed, list):
        return None
    return [item for item in parsed if isinstance(item, str)]


def _js_rows(expr: str | None) -> list[list[str]] | None:
    parsed = _parse_js(expr) if expr else None
    if not isinstance(parsed, list):
        return None
    rows = []
    for item in parsed:
        if isinstance(item, list):
            rows.append([cell for cell in item if isinstance(cell, str)])
    return rows


def _js_series(expr: str | None) -> list[tuple[str, list[str]]] | None:
    parsed = _parse_js(expr) if expr else None
    if not isinstance(parsed, list):
        return None
    series = []
    for item in parsed:
        if not isinstance(item, dict):
            continue
        name = item.get("name")
        data = item.get("data")
        series.append((name if isinstance(name, str) else "", [str(value) for value in data] if isinstance(data, list) else []))
    return series


def _parse_js(expr: str | None):
    if expr is None:
        return None
    parser = _Parser(expr)
    try:
        return parser.parse_value()
    except ValueError:
        return None


class _Parser:
    def __init__(self, source: str):
        self.source = source
        self.index = 0

    def parse_value(self):
        self._skip()
        char = self._peek()
        if char in {'"', "'"}:
            return self._string()
        if char == "[":
            return self._array()
        if char == "{":
            return self._object()
        if char is not None and (char == "-" or char.isdigit()):
            return self._number()
        self._ident()
        return None

    def _array(self) -> list:
        self._bump()
        items = []
        while True:
            self._skip()
            if self._peek() == "]":
                self._bump()
                return items
            if self._peek() is None:
                raise ValueError("array")
            items.append(self.parse_value())
            self._skip()
            if self._peek() == ",":
                self._bump()

    def _object(self) -> dict:
        self._bump()
        fields = {}
        while True:
            self._skip()
            if self._peek() == "}":
                self._bump()
                return fields
            if self._peek() is None:
                raise ValueError("object")
            key = self._string() if self._peek() in {'"', "'"} else self._ident()
            self._skip()
            if self._peek() == ":":
                self._bump()
            fields[key] = self.parse_value()
            self._skip()
            if self._peek() == ",":
                self._bump()

    def _string(self) -> str:
        mark = self._bump()
        out = []
        while True:
            char = self._bump()
            if char is None:
                raise ValueError("string")
            if char == "\\":
                escaped = self._bump()
                out.append({"n": "\n", "t": "\t"}.get(escaped or "", escaped or ""))
                continue
            if char == mark:
                return "".join(out)
            out.append(char)

    def _number(self) -> str:
        start = self.index
        if self._peek() == "-":
            self._bump()
        while self._peek() is not None and (self._peek().isdigit() or self._peek() == "."):
            self._bump()
        return self.source[start : self.index]

    def _ident(self) -> str:
        start = self.index
        while self._peek() is not None and (self._peek().isalnum() or self._peek() == "_"):
            self._bump()
        return self.source[start : self.index]

    def _skip(self) -> None:
        while True:
            while self._peek() is not None and self._peek().isspace():
                self._bump()
            if self.source.startswith("//", self.index):
                self.index += 2
                while self._peek() not in {None, "\n"}:
                    self._bump()
                continue
            return

    def _peek(self) -> str | None:
        if self.index >= len(self.source):
            return None
        return self.source[self.index]

    def _bump(self) -> str | None:
        char = self._peek()
        if char is None:
            return None
        self.index += 1
        return char
