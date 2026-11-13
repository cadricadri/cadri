"""Memory books backed by local bare repositories, so tests run real git without a server.

The bare repos stand in for <server>/git/<scope>.git. The HTTP tip check is patched
per test; the server's own rules (fast-forward only, author trailers) are covered by
test_memory_engine.py against a real engine.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from cadri.memory import books as bk  # noqa: E402
from cadri.state import write_json  # noqa: E402

BASE = "http://127.0.0.1:8080"
TOKEN = "secret-token-123"


def run_git(cwd: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-c", "user.name=Other", "-c", "user.email=other@example.com", *args],
        cwd=cwd,
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


class BooksTestCase(unittest.TestCase):
    """A signed-in identity with team and personal books cloned from bare remotes."""

    seed_team = {"MEMORY.md": "# Team memory\n\nDeploys happen on Tuesdays.\n"}
    seed_user = {"MEMORY.md": "# Personal memory\n\nPrefers short answers.\n"}

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name).resolve()
        env = {key: value for key, value in os.environ.items() if not key.startswith(("CADRI_", "CLAUDE_"))}
        env.update({"CADRI_HOME": str(self.tmp / "home"), "CADRI_URL": BASE, "CADRI_API_TOKEN": TOKEN})
        patcher = mock.patch.dict(os.environ, env, clear=True)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.remotes = self.tmp / "remotes"
        for scope, files in (("team", self.seed_team), ("user", self.seed_user)):
            bare = self.remotes / f"{scope}.git"
            bare.mkdir(parents=True)
            run_git(bare, "init", "--quiet", "--bare", "--initial-branch=main")
            if files:
                self.push_from_elsewhere(scope, files, "Seed")
        remotes = self.remotes
        remote = mock.patch.object(bk.Book, "remote", property(lambda book: str(remotes / f"{book.scope}.git")))
        remote.start()
        self.addCleanup(remote.stop)
        write_json(bk.identity_path(BASE), {"org_id": "org1", "user_id": "u1", "email": "me@example.com", "checked_at": time.time()})
        self.team = bk.Book(BASE, "org1", "u1", "team")
        self.user = bk.Book(BASE, "org1", "u1", "user")
        for book in (self.team, self.user):
            bk.ensure_clone(book)

    def elsewhere(self, scope: str) -> Path:
        path = self.tmp / f"elsewhere-{scope}"
        if not (path / ".git").exists():
            subprocess.run(["git", "clone", "--quiet", str(self.remotes / f"{scope}.git"), str(path)], check=True, capture_output=True)
        else:
            run_git(path, "pull", "--quiet", "--ff-only")
        return path

    def push_from_elsewhere(self, scope: str, files: dict[str, str], message: str = "Elsewhere") -> None:
        """Another machine commits `files` and pushes them to main."""
        path = self.elsewhere(scope)
        for relative, content in files.items():
            target = path / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8")
        run_git(path, "add", "-A")
        run_git(path, "commit", "--quiet", "-m", message)
        run_git(path, "push", "--quiet", "origin", "HEAD:main")

    def remote_file(self, scope: str, relative: str) -> str:
        return run_git(self.remotes / f"{scope}.git", "show", f"main:{relative}")

    def remote_log(self, scope: str, count: int = 1) -> str:
        return run_git(self.remotes / f"{scope}.git", "log", f"-{count}", "--format=%B%x00", "main")

    def write(self, book: bk.Book, relative: str, content: str, age: float = 0) -> Path:
        path = book.path / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        if age:
            then = time.time() - age
            os.utime(path, (then, then))
        return path

    def head(self, book: bk.Book, ref: str = "HEAD") -> str:
        return run_git(book.path, "rev-parse", ref)
