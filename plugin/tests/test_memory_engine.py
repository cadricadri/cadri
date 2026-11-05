"""End-to-end check of the memory client against a running engine.

Skipped unless CADRI_TEST_ENGINE_URL points at an engine in auth mode none, for example:
  CADRI_DB=/tmp/x/t.db CADRI_MEMORIES=/tmp/x/mem CADRI_BIND=127.0.0.1:18412 cadri-engine
  CADRI_TEST_ENGINE_URL=http://127.0.0.1:18412 python3 -m unittest plugin/tests/test_memory_engine.py
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
sys.path.insert(0, str(PLUGIN / "bin"))

import cadri_cli  # noqa: E402
from shared import memory  # noqa: E402

ENGINE = os.environ.get("CADRI_TEST_ENGINE_URL")


@unittest.skipUnless(ENGINE, "set CADRI_TEST_ENGINE_URL to run against a local engine")
class MemoryEngineTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        env = mock.patch.dict(
            os.environ, {"CADRI_HOME": self.tmp.name, "CADRI_URL": ENGINE, "CADRI_API_TOKEN": ""}
        )
        env.start()
        self.addCleanup(env.stop)
        stored = mock.patch("shared.cadri_auth.stored_access_token", return_value=None)
        stored.start()
        self.addCleanup(stored.stop)
        self.suffix = os.urandom(4).hex()

    def cli(self, *argv, stdin=""):
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err), mock.patch("sys.stdin", io.StringIO(stdin)):
            code = cadri_cli.main(["memory", *argv])
        return code, out.getvalue().strip(), err.getvalue()

    def post_edit(self, path, content, **extra):
        body = json.dumps({"message": "ui", "changes": [{"op": "write", "path": path, "content": content}], **extra})
        request = urllib.request.Request(
            f"{ENGINE}/api/memories/commits?view=tree",
            data=body.encode(),
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        with urllib.request.urlopen(request, timeout=15) as response:
            return json.load(response)

    def test_clone_write_sync_and_conflict_against_a_real_engine(self):
        code, path, _ = self.cli("clone")
        self.assertEqual(code, 0)
        team = Path(path)
        self.assertTrue((team / ".git").is_dir())

        name = f"it-{self.suffix}.md"
        code, commit, err = self.cli("write", name, "-m", "first", "--agent", "it-agent", stdin="one\n")
        self.assertEqual((code, len(commit)), (0, 40), err)
        self.assertEqual((team / name).read_text(), "one\n")

        self.post_edit(f"other-{self.suffix}.md", "two\n")
        code, _, err = self.cli("sync")
        self.assertEqual(code, 0, err)
        self.assertEqual((team / f"other-{self.suffix}.md").read_text(), "two\n")
        self.assertEqual(self.cli("sync", "--quiet")[0], 0)

        self.post_edit(name, "changed elsewhere\n")
        code, _, err = self.cli("write", name, "-m", "stale", stdin="mine\n")
        self.assertEqual(code, memory.EXIT_CONFLICT)
        self.assertIn(name, err)

        code, _, _ = self.cli("sync")
        self.assertEqual(code, 0)
        code, _, err = self.cli("write", name, "-m", "retry", stdin="mine\n")
        self.assertEqual(code, 0, err)
        self.assertEqual((team / name).read_text(), "mine\n")

    def test_personal_book_is_a_separate_clone(self):
        code, path, err = self.cli("write", f"me-{self.suffix}.md", "-m", "mine", "--scope", "user", stdin="x\n")
        self.assertEqual(code, 0, err)
        user_path = Path(self.cli("path", "--scope", "user")[1])
        self.assertTrue(user_path.name.startswith("user-"))
        self.assertTrue((user_path / f"me-{self.suffix}.md").exists())
        self.assertFalse((user_path.parent / "team" / f"me-{self.suffix}.md").exists())


if __name__ == "__main__":
    unittest.main()
