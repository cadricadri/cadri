"""Unit tests for the shared memory clone client, with HTTP and git mocked."""

from __future__ import annotations

import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import urllib.error
from pathlib import Path
from unittest import mock

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))
sys.path.insert(0, str(PLUGIN / "bin"))

import cadri_cli  # noqa: E402
from shared import memory  # noqa: E402

BASE = "http://127.0.0.1:8080"
HEAD = "a" * 40
NEW = "b" * 40
TOKEN = "secret-token-123"


class Response:
    def __init__(self, status=200, body=None, headers=None):
        self.status = status
        self.body = json.dumps(body or {}).encode()
        self.headers = headers or {}

    def read(self):
        return self.body

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


class FakeServer:
    """Stands in for urllib.request.urlopen; records requests, answers by path."""

    def __init__(self, routes):
        self.routes = routes
        self.requests = []

    def __call__(self, request, timeout=None):
        url = request.full_url
        self.requests.append(
            {
                "url": url,
                "method": request.get_method(),
                "headers": {k.lower(): v for k, v in request.header_items()},
                "body": json.loads(request.data) if request.data else None,
            }
        )
        status, body = self.routes[url.split(BASE)[1].split("?")[0]]
        if status >= 300:
            raise urllib.error.HTTPError(url, status, "x", {}, io.BytesIO(json.dumps(body).encode()))
        return Response(status, body)


class GitRecorder:
    def __init__(self, head=HEAD, fail=()):
        self.calls = []
        self.head = head
        self.fail = fail

    def __call__(self, command, **kwargs):
        self.calls.append(command)
        code = 1 if any(f in command for f in self.fail) else 0
        out = ""
        if "rev-parse" in command:
            code, out = (0, self.head + "\n") if self.head else (1, "")
        return subprocess.CompletedProcess(command, code, out, "")


class MemoryClientTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        env = mock.patch.dict(os.environ, {"CADRI_HOME": self.tmp.name, "CADRI_API_TOKEN": TOKEN, "CADRI_URL": BASE}, clear=False)
        env.start()
        self.addCleanup(env.stop)
        self.team = memory.Book(BASE, "org1", "u1", "team")
        self.user = memory.Book(BASE, "org1", "u1", "user")

    def make_clone(self, book):
        (book.path / ".git").mkdir(parents=True)

    def run_cli(self, argv, server, git):
        with mock.patch("urllib.request.urlopen", server), mock.patch("subprocess.run", git):
            return cadri_cli.main(argv)

    def test_paths_for_team_and_user_books(self):
        root = Path(self.tmp.name) / "memory" / "127.0.0.1-8080" / "org1"
        self.assertEqual(self.team.path, root / "team")
        self.assertEqual(self.user.path, root / "user-u1")
        self.assertEqual(self.team.lock_path, root / "team.lock")
        self.assertEqual(self.team.watch_lock_path, root / "team.watch.lock")
        self.assertEqual(self.team.remote, f"{BASE}/git/team.git")
        self.assertEqual(self.user.remote, f"{BASE}/git/user.git")
        self.assertEqual(memory.heartbeat_path(), Path(self.tmp.name) / "memory" / "heartbeat")

    def test_default_host_directory_has_no_port_noise(self):
        book = memory.Book("https://app.cadri.io", "o", "u", "team")
        self.assertIn("app.cadri.io", book.path.parts)

    def test_every_git_call_carries_the_auth_header_and_no_token_hits_disk(self):
        server = FakeServer({"/api/me": (200, {"org_id": "org1", "user_id": "u1"}), "/api/memories/tip": (200, {})})
        git = GitRecorder()
        self.assertEqual(self.run_cli(["memory", "clone"], server, git), 0)
        self.assertTrue(git.calls)
        for command in git.calls:
            self.assertEqual(command[:3], ["git", "-c", f"http.extraHeader=Authorization: Bearer {TOKEN}"])
        clone = next(c for c in git.calls if "clone" in c)
        self.assertNotIn(TOKEN, " ".join(clone[3:]))
        self.make_clone(self.team)
        self.assertEqual(self.run_cli(["memory", "sync"], server, git), 0)
        for path in Path(self.tmp.name).rglob("*"):
            if path.is_file():
                self.assertNotIn(TOKEN, path.read_text(errors="ignore"))

    def test_write_sends_base_and_agent_and_prints_the_commit(self):
        self.make_clone(self.team)
        server = FakeServer(
            {
                "/api/me": (200, {"org_id": "org1", "user_id": "u1"}),
                "/api/memories/commits": (200, {"commit": NEW, "tip": NEW}),
                "/api/memories/tip": (200, {"tip": NEW}),
            }
        )
        out = io.StringIO()
        with mock.patch("sys.stdin", io.StringIO("hello\n")), mock.patch("sys.stdout", out):
            code = self.run_cli(["memory", "write", "notes.md", "-m", "Add notes", "--agent", "bot-1"], server, GitRecorder())
        self.assertEqual(code, 0)
        post = next(r for r in server.requests if r["method"] == "POST")
        self.assertEqual(post["url"], f"{BASE}/api/memories/commits?view=tree")
        self.assertEqual(
            post["body"],
            {
                "message": "Add notes",
                "changes": [{"op": "write", "path": "notes.md", "content": "hello\n"}],
                "base": HEAD,
                "agent": "bot-1",
            },
        )
        self.assertEqual(out.getvalue().strip(), NEW)

    def test_write_to_the_personal_book_uses_the_user_scope_and_omits_base_when_empty(self):
        self.make_clone(self.user)
        server = FakeServer(
            {
                "/api/me": (200, {"org_id": "org1", "user_id": "u1"}),
                "/api/memories/commits": (200, {"commit": NEW}),
                "/api/memories/tip": (200, {}),
            }
        )
        with mock.patch("sys.stdin", io.StringIO("x")), mock.patch.dict(os.environ, {"CADRI_AGENT_ID": "env-agent"}):
            self.run_cli(["memory", "write", "a.md", "-m", "m", "--scope", "user"], server, GitRecorder(head=None))
        post = next(r for r in server.requests if r["method"] == "POST")
        self.assertEqual(post["url"], f"{BASE}/api/memories/commits?view=tree&scope=user")
        self.assertNotIn("base", post["body"])
        self.assertEqual(post["body"]["agent"], "env-agent")

    def test_write_exits_3_on_a_conflict_and_lists_the_paths(self):
        self.make_clone(self.team)
        server = FakeServer(
            {
                "/api/me": (200, {"org_id": "org1", "user_id": "u1"}),
                "/api/memories/commits": (409, {"error": "conflict", "paths": ["deploy.md"]}),
            }
        )
        err = io.StringIO()
        with mock.patch("sys.stdin", io.StringIO("x")), mock.patch("sys.stderr", err):
            code = self.run_cli(["memory", "write", "deploy.md", "-m", "m"], server, GitRecorder())
        self.assertEqual(code, 3)
        self.assertIn("deploy.md", err.getvalue())

    def test_sync_stops_early_when_the_tip_is_unchanged(self):
        self.make_clone(self.team)
        server = FakeServer({"/api/me": (200, {"org_id": "org1", "user_id": "u1"}), "/api/memories/tip": (304, {})})
        git = GitRecorder()
        self.assertEqual(self.run_cli(["memory", "sync"], server, git), 0)
        tip = next(r for r in server.requests if r["url"].endswith("/api/memories/tip"))
        self.assertEqual(tip["headers"]["if-none-match"], f'"{HEAD}"')
        self.assertFalse(any("fetch" in c for c in git.calls))
        self.assertTrue(self.team.synced_path.exists())

    def test_sync_fetches_and_fast_forwards_when_the_tip_moved(self):
        self.make_clone(self.team)
        server = FakeServer({"/api/me": (200, {"org_id": "org1", "user_id": "u1"}), "/api/memories/tip": (200, {"tip": NEW})})
        git = GitRecorder()
        self.assertEqual(self.run_cli(["memory", "sync", "--quiet"], server, git), 0)
        self.assertTrue(any("fetch" in c for c in git.calls))
        self.assertTrue(any("--ff-only" in c for c in git.calls))

    def test_sync_aborts_the_rebase_and_exits_1_when_local_commits_conflict(self):
        self.make_clone(self.team)
        server = FakeServer({"/api/me": (200, {"org_id": "org1", "user_id": "u1"}), "/api/memories/tip": (200, {})})
        git = GitRecorder(fail=("--ff-only",))
        real = git.__call__

        def failing(command, **kwargs):
            if "rebase" in command and "--abort" not in command:
                git.calls.append(command)
                return subprocess.CompletedProcess(command, 1, "", "conflict")
            return real(command, **kwargs)

        err = io.StringIO()
        with mock.patch("sys.stderr", err):
            code = self.run_cli(["memory", "sync"], server, failing)
        self.assertEqual(code, 1)
        self.assertTrue(any("--abort" in c for c in git.calls))
        self.assertIn("conflict", err.getvalue())

    def test_sync_does_nothing_when_another_sync_holds_the_lock(self):
        self.make_clone(self.team)
        held = memory.SyncLock(self.team.lock_path)
        self.addCleanup(held.release)
        server = FakeServer({})
        with mock.patch("urllib.request.urlopen", server), mock.patch("subprocess.run", GitRecorder()):
            self.assertEqual(memory.sync(self.team), 0)
        self.assertEqual(server.requests, [])

    def test_watch_exits_when_the_heartbeat_is_stale_and_when_already_running(self):
        server = FakeServer({"/api/me": (200, {"org_id": "org1", "user_id": "u1"})})
        memory.touch_heartbeat()
        old = 1_000_000_000
        os.utime(memory.heartbeat_path(), (old, old))
        with mock.patch("urllib.request.urlopen", server):
            # touching at start makes it fresh; make it stale again on the first sleep
            calls = []

            def fake_sleep(_):
                calls.append(1)
                os.utime(memory.heartbeat_path(), (old, old))

            with mock.patch("shared.memory.sync", return_value=0):
                self.assertEqual(memory.watch(BASE, sleep=fake_sleep), 0)
            self.assertEqual(len(calls), 1)
            held = memory.SyncLock(self.team.watch_lock_path)
            self.addCleanup(held.release)
            with mock.patch("shared.memory.sync", side_effect=AssertionError("must not sync")):
                self.assertEqual(memory.watch(BASE, sleep=fake_sleep), 0)

    def test_detach_starts_a_new_session_and_returns_at_once(self):
        with mock.patch("subprocess.Popen") as popen:
            memory.detach_watcher(Path("/x/cadri_cli.py"))
        kwargs = popen.call_args.kwargs
        self.assertTrue(kwargs["start_new_session"])
        self.assertEqual(popen.call_args.args[0][-2:], ["memory", "watch"])


if __name__ == "__main__":
    unittest.main()
