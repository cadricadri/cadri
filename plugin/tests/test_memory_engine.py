"""End-to-end check of the memory client against a running engine.

Skipped unless CADRI_TEST_ENGINE_URL points at an engine in auth mode none, for example:
  CADRI_DB=/tmp/x/t.db CADRI_MEMORIES=/tmp/x/mem CADRI_BIND=127.0.0.1:18412 cadri-engine
  CADRI_TEST_ENGINE_URL=http://127.0.0.1:18412 python3 -m unittest discover -s plugin/tests -p test_memory_engine.py
The test writes to the team and personal books of that engine.
"""

from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
import urllib.request
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from cadri import cli  # noqa: E402
from cadri.memory import editapi, push, repair  # noqa: E402
from cadri.memory import books as bk  # noqa: E402

ENGINE = os.environ.get("CADRI_TEST_ENGINE_URL")


@unittest.skipUnless(ENGINE, "set CADRI_TEST_ENGINE_URL to run against a local engine")
class MemoryEngineTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.use_home("a")
        stored = mock.patch("cadri.auth.stored_access_token", return_value=None)
        stored.start()
        self.addCleanup(stored.stop)
        self.suffix = os.urandom(4).hex()

    def use_home(self, name: str) -> None:
        env = mock.patch.dict(
            os.environ,
            {"CADRI_HOME": str(Path(self.tmp.name) / name), "CADRI_URL": ENGINE, "CADRI_API_TOKEN": "", "CADRI_SESSION_ID": f"s-{name}"},
        )
        env.start()
        self.addCleanup(env.stop)

    def cli(self, *argv, stdin=""):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err), mock.patch("sys.stdin", io.StringIO(stdin)):
            code = cli.main(["memory", *argv])
        return code, out.getvalue().strip(), err.getvalue()

    def post_edit(self, path, content):
        body = json.dumps({"message": "ui", "changes": [{"op": "write", "path": path, "content": content}]})
        request = urllib.request.Request(
            f"{ENGINE}/api/memories/commits?view=tree",
            data=body.encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)

    def test_edit_push_and_sync_against_a_real_engine(self):
        code, path, err = self.cli("clone")
        self.assertEqual(code, 0, err)
        team = Path(path)
        name = f"it-{self.suffix}.md"
        (team / name).write_text("one\n")
        code, _, err = self.cli("push", "--book", "team", "--harness", "claude", "-m", "First")
        self.assertEqual(code, 0, err)
        self.assertIn("pushed 1 commit", err)

        self.post_edit(f"other-{self.suffix}.md", "two\n")
        code, _, err = self.cli("sync", "--scope", "team")
        self.assertEqual(code, 0, err)
        self.assertEqual((team / f"other-{self.suffix}.md").read_text(), "two\n")

    def test_concurrent_pushes_rebase_and_conflicts_go_through_repair(self):
        self.assertEqual(self.cli("clone")[0], 0)
        book = bk.open_book("team")
        name = f"shared-{self.suffix}.md"
        (book.path / name).write_text("base\n")
        self.assertEqual(self.cli("push", "--book", "team", "-m", "base")[0], 0)

        self.post_edit(f"unrelated-{self.suffix}.md", "elsewhere\n")
        (book.path / f"mine-{self.suffix}.md").write_text("mine\n")
        code, _, err = self.cli("push", "--book", "team", "-m", "mine")
        self.assertEqual(code, 0, err)

        self.post_edit(name, "theirs\n")
        (book.path / name).write_text("ours\n")
        code, _, err = self.cli("push", "--book", "team", "-m", "ours")
        self.assertEqual(code, push.EXIT_CONFLICT, err)
        (repair.worktree(book) / name).write_text("ours and theirs\n")
        code, _, err = self.cli("push", "--continue", "--book", "team")
        self.assertEqual(code, 0, err)
        self.assertEqual((book.path / name).read_text(), "ours and theirs\n")

    def test_personal_book_is_a_separate_clone(self):
        code, path, err = self.cli("path", "--scope", "user")
        self.assertEqual(code, 0, err)
        user_path = Path(path)
        self.assertTrue(user_path.name.startswith("user-"))
        (user_path / f"me-{self.suffix}.md").write_text("x\n")
        self.assertEqual(self.cli("push", "--book", "user", "-m", "mine")[0], 0)
        self.assertFalse((user_path.parent / "team" / f"me-{self.suffix}.md").exists())

    def test_edit_api_write_still_works(self):
        book = bk.open_book("team")
        bk.ensure_clone(book)
        code, commit, err = self.cli("write", f"api-{self.suffix}.md", "-m", "api", "--agent", "it-agent", stdin="api\n")
        self.assertEqual((code, len(commit)), (0, 40), err)
        self.assertEqual(editapi.EXIT_CONFLICT, 3)


if __name__ == "__main__":
    unittest.main()
