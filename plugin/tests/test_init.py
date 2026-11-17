"""`cadri init` as one command: install the agents it found, upload the seed window, clone both books, name the next skill."""

from __future__ import annotations

import io
import os
import shutil
from contextlib import redirect_stderr, redirect_stdout
from unittest import mock

from test_plugin import FakeCadri
from support import BooksTestCase

from cadri import cli
from cadri.memory import books as bk


class InitHarness:
    def setUp(self):
        super().setUp()
        self.server = FakeCadri()
        self.addCleanup(self.server.close)
        self.server.gets["/api/me"] = (200, {"org_id": "org1", "user_id": "u1", "email": "me@example.com"})
        self.server.respond("/api/sessions", (201, {"session_id": "thread-1"}))
        self.user_home = self.tmp / "user"
        self.user_home.mkdir()
        (self.user_home / ".claude").mkdir()
        (self.user_home / ".cursor").mkdir()
        git = shutil.which("git")
        assert git is not None
        bindir = self.tmp / "git-only"
        bindir.mkdir()
        os.symlink(git, bindir / "git")
        os.environ["HOME"] = str(self.user_home)
        os.environ["PATH"] = str(bindir)
        os.environ["CADRI_URL"] = self.server.url
        for key in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME"):
            os.environ.pop(key, None)
        self.plant_session()

    def plant_session(self, body: str = '{"type":"user","message":{"content":"hello from the session"}}\n') -> None:
        path = self.user_home / ".claude" / "projects" / "-tmp-repo" / "thread-1.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body, encoding="utf-8")

    def cadri(self, *argv: str) -> tuple[int, str, str]:
        out, err = io.StringIO(), io.StringIO()
        with redirect_stdout(out), redirect_stderr(err):
            code = cli.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def init(self, *extra: str) -> tuple[int, str, str]:
        return self.cadri("init", "--local", "--url", self.server.url, *extra)


class InitTests(InitHarness, BooksTestCase):
    def test_a_team_without_an_organization_stops_init(self):
        self.server.respond("/api/sessions", (403, {"error": "no_organization"}))
        code, _out, err = self.init()
        self.assertEqual(code, 1)
        self.assertIn("create your team", err)
        self.assertIn("cadri init", err)

    def test_show_prints_one_local_session(self):
        code, out, err = self.cadri("show", "thread-1")
        self.assertEqual(code, 0, err)
        self.assertIn("hello from the session", out)
        code, _out, err = self.cadri("show", "missing-thread")
        self.assertEqual(code, 1)
        self.assertIn("no local session missing-thread", err)
        self.assertIn("cadri discover", err)

    def test_uninstall_removes_the_hooks_init_wrote(self):
        self.assertEqual(self.init()[0], 0)
        code, out, err = self.cadri("uninstall")
        self.assertEqual(code, 0, err)
        self.assertIn("Claude Code plugin", out)
        self.assertIn("Cursor plugin", out)
        self.assertIn("Memory books stay", out)
        settings = self.user_home / ".claude" / "settings.json"
        text = settings.read_text(encoding="utf-8") if settings.is_file() else ""
        self.assertNotIn("cadri", text)
        self.assertFalse((self.user_home / ".cursor" / "plugins" / "local" / "cadri").exists())

    def test_init_without_git_does_not_name_a_skill_yet(self):
        bk.git_ready.cache_clear()
        self.addCleanup(bk.git_ready.cache_clear)
        with mock.patch("cadri.memory.books.shutil.which", return_value=None):
            code, out, err = self.init()
        self.assertEqual(code, 0, err)
        self.assertIn("git is not installed", out)
        self.assertIn("Next: clone both books", out)
        self.assertIn("cadri memory clone", out)
        self.assertNotIn("Next: follow the cadri-memory-seed", out)
        self.assertIn("uploaded 1 sessions", out)

    def test_init_installs_uploads_clones_and_names_the_seed_skill(self):
        code, out, err = self.init()
        self.assertEqual(err, "", err)
        self.assertEqual(code, 0, out)
        self.assertIn("Agents on this machine: Claude Code, Cursor", out)
        self.assertIn("Claude Code: hooks in", out)
        self.assertIn("Cursor: plugin at", out)
        self.assertIn("Uploading 1 sessions", out)
        self.assertIn("uploaded 1 sessions", out)
        self.assertIn("Claude Code 1", out)
        self.assertIn("Memory: cloned", out)
        self.assertIn("Books stay empty until seeding", out)
        self.assertIn("Next: follow the cadri-memory-seed skill.", out)
        self.assertNotIn("cadri-memory-join", out)

        settings = (self.user_home / ".claude" / "settings.json").read_text(encoding="utf-8")
        self.assertIn("hook ", settings)
        self.assertIn("--harness claude", settings)
        hooks = self.user_home / ".cursor" / "plugins" / "local" / "cadri" / "hooks" / "hooks.json"
        self.assertTrue(hooks.is_file(), hooks)
        skills = self.user_home / ".claude" / "skills"
        self.assertEqual(sorted(path.name for path in skills.iterdir()), sorted(cli_shipped_skills()))

        [upload] = self.server.requests_to("/api/sessions")
        self.assertEqual(upload["body"]["thread_id"], "thread-1")
        self.assertEqual(upload["body"]["hook_event"], "backfill")
        self.assertEqual(upload["body"]["harness"], "claude_code")
        self.assertIn("hello from the session", upload["body"]["content"])
        cloned = bk.Book(self.server.url, "org1", "u1", "team")
        self.assertTrue((cloned.path / "MEMORY.md").is_file())


class InitJoinsAnExistingBook(InitHarness, BooksTestCase):
    seed_team = {
        "MEMORY.md": "# Team memory\n\nDeploys happen on Tuesdays.\n",
        "components/billing.md": "---\nname: Billing\ndescription: Rates\n---\n# Billing\n\nRetries.\n",
    }

    def test_init_names_the_join_skill_when_the_team_book_has_a_lesson(self):
        code, out, err = self.init()
        self.assertEqual(code, 0, err)
        self.assertIn("The team book already has memories", out)
        self.assertIn("Your personal book starts empty.", out)
        self.assertIn("Next: follow the cadri-memory-join skill.", out)
        self.assertNotIn("cadri-memory-seed", out)


def cli_shipped_skills() -> tuple[str, ...]:
    from cadri.install.bundles import SHIPPED_SKILLS

    return SHIPPED_SKILLS
