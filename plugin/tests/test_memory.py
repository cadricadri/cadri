"""The memory books with real git: push attribution, concurrency, repair, and sync."""

from __future__ import annotations

import io
import json
import os
import subprocess
import tempfile
import time
import unittest
import urllib.error
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest import mock

from support import BASE, TOKEN, BooksTestCase, run_git

from cadri import cli
from cadri.memory import books as bk
from cadri.memory import edits, editapi, push, repair, sync
from cadri.memory.layout import TEAM_LAYOUT, has_lessons
from cadri.state import locked


def caller(session="s1", agent=None, human=None, since=None, harness="claude"):
    return push.caller_from(session, harness, agent, human, since)


class Logs(list):
    def __call__(self, message):
        self.append(message)

    def text(self):
        return "\n".join(self)


class BookTests(BooksTestCase):
    def test_paths_for_team_and_personal_books(self):
        root = Path(os.environ["CADRI_HOME"]) / "memory" / "127.0.0.1-8080" / "org1"
        self.assertEqual(self.team.path, root / "team")
        self.assertEqual(self.user.path, root / "user-u1")
        self.assertEqual(self.team.lock_path, root / "team.lock")
        self.assertEqual(repair.worktree(self.team), root / "team.repair")
        self.assertIn("app.cadri.io", bk.Book("https://app.cadri.io", "o", "u", "team").path.parts)

    def test_local_books_come_from_the_cached_identity(self):
        self.assertEqual([book.path for book in bk.local_books()], [self.team.path, self.user.path])
        bk.identity_path(BASE).unlink()
        self.assertEqual(bk.local_books(), [])

    def test_git_never_runs_without_command_line_tools(self):
        with mock.patch.object(bk, "git_ready", return_value=False), mock.patch("subprocess.run") as run:
            with self.assertRaisesRegex(bk.MemoryFailure, "xcode-select --install"):
                bk.git(self.team, "status")
        run.assert_not_called()

    def test_the_token_goes_only_to_network_commands_and_never_to_disk(self):
        with mock.patch("subprocess.run", wraps=subprocess.run) as run:
            bk.git(self.team, "status")
            bk.git(self.team, "fetch", "--quiet", "origin", "main", network=True)
        local, network = (call.args[0] for call in run.call_args_list)
        self.assertNotIn(f"http.extraHeader=Authorization: Bearer {TOKEN}", local)
        self.assertIn(f"http.extraHeader=Authorization: Bearer {TOKEN}", network)
        self.write(self.team, "notes.md", "hello\n")
        self.assertEqual(push.push_book(self.team, caller(), "Add notes", log=Logs()), 0)
        for path in Path(os.environ["CADRI_HOME"]).rglob("*"):
            if path.is_file():
                self.assertNotIn(TOKEN, path.read_text(errors="ignore"), path)


class PushTests(BooksTestCase):
    def test_commits_only_the_callers_files_with_trailers(self):
        self.write(self.team, "mine.md", "mine\n")
        self.write(self.team, "theirs.md", "theirs\n")
        edits.record(self.team, "mine.md", "s1", "main", "claude")
        edits.record(self.team, "theirs.md", "s2", "main", "claude")
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "Save mine", log=logs), 0, logs.text())
        self.assertEqual(self.remote_file("team", "mine.md"), "mine")
        with self.assertRaises(subprocess.CalledProcessError):
            self.remote_file("team", "theirs.md")
        message = self.remote_log("team")
        self.assertIn("Save mine", message)
        self.assertIn("Cadri-Author-Kind: robot", message)
        self.assertIn("Cadri-Author-Label: Claude Code agent", message)
        self.assertIn("Cadri-Session-Id: s1", message)
        self.assertIn("theirs.md", push.changed_paths(self.team))
        self.assertEqual(self.head(self.team), self.head(self.team, "origin/main"))

    def test_takes_files_another_session_left_quiet_as_their_own_commit(self):
        self.write(self.team, "mine.md", "mine\n")
        self.write(self.team, "stale.md", "stale\n", age=600)
        edits.record(self.team, "stale.md", "s2", "main", "codex")
        record = edits.record_path(self.team)
        rows = [json.loads(line) for line in record.read_text().splitlines()]
        rows[-1]["at"] -= 600
        record.write_text("".join(json.dumps(row) + "\n" for row in rows))
        self.assertEqual(push.push_book(self.team, caller(), "Save mine", log=Logs()), 0)
        newest, older = self.remote_log("team", 2).split("\x00")[:2]
        self.assertIn("Cadri-Session-Id: s1", older)
        self.assertIn("Cadri-Session-Id: s2", newest)
        self.assertIn("Left uncommitted by session s2", newest)
        self.assertIn("Codex agent", newest)

    def test_human_pushes_carry_the_users_id(self):
        self.write(self.user, "style.md", "No emoji.\n")
        human = push.Author("human", "u1", "me@example.com")
        self.assertEqual(push.push_book(self.user, caller(human=human), "Remember style", log=Logs()), 0)
        message = self.remote_log("user")
        self.assertIn("Cadri-Author-Kind: human", message)
        self.assertIn("Cadri-Author-Id: u1", message)

    def test_team_books_reject_a_new_top_level_directory(self):
        self.write(self.team, "scratch/note.md", "nope\n")
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "Bad area", log=logs), 1)
        self.assertIn("top-level directories", logs.text())
        (self.team.path / "scratch/note.md").unlink()
        (self.team.path / "scratch").rmdir()
        self.write(self.team, "features/flags.md", "---\nname: flags\ndescription: Feature flags\n---\nLaunchDarkly.\n")
        self.assertEqual(push.push_book(self.team, caller(), "Add feature note", log=Logs()), 0)
        self.assertEqual(self.remote_file("team", "features/flags.md").splitlines()[-1], "LaunchDarkly.")

    def test_a_keeper_owns_its_edits_whatever_session_its_harness_reported(self):
        self.write(self.team, "a.md", "a\n")
        since = time.time() - 5
        edits.record(self.team, "a.md", "child-thread", "cadri-memory-keeper", "codex")
        keeper = caller(session="parent", agent="cadri:cadri-memory-keeper", since=since, harness="codex")
        self.assertEqual(push.push_book(self.team, keeper, "Keep a", log=Logs()), 0)
        message = self.remote_log("team")
        self.assertIn("Cadri-Session-Id: parent", message)
        self.assertIn("Codex memory keeper", message)

    def test_rebases_elsewhere_when_the_server_moved_and_keeps_unrelated_edits(self):
        self.push_from_elsewhere("team", {"other.md": "other\n"})
        self.write(self.team, "mine.md", "mine\n")
        edits.record(self.team, "mine.md", "s1", "main", "claude")
        self.write(self.team, "busy.md", "half written\n")
        edits.record(self.team, "busy.md", "s2", "main", "claude")
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "Save mine", log=logs), 0, logs.text())
        self.assertEqual(self.remote_file("team", "other.md"), "other")
        self.assertEqual(self.remote_file("team", "mine.md"), "mine")
        self.assertEqual(self.head(self.team), self.head(self.team, "origin/main"))
        self.assertEqual((self.team.path / "busy.md").read_text(), "half written\n")
        self.assertEqual((self.team.path / "other.md").read_text(), "other\n")
        self.assertFalse(self.team.sibling(".integrate").exists())

    def test_a_conflict_is_resolved_in_the_repair_worktree_then_published(self):
        self.push_from_elsewhere("team", {"MEMORY.md": "# Team memory\n\nDeploys happen on Thursdays.\n"})
        self.write(self.team, "MEMORY.md", "# Team memory\n\nDeploys happen on Wednesdays.\n")
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "Change deploy day", log=logs), push.EXIT_CONFLICT)
        self.assertIn("cadri memory push --continue --book team", logs.text())
        self.assertTrue(repair.active(self.team))
        self.assertEqual(repair.marker(self.team)["conflicts"], ["MEMORY.md"])
        worktree = repair.worktree(self.team)
        self.assertIn("<<<<<<<", (worktree / "MEMORY.md").read_text())
        self.assertNotIn("<<<<<<<", (self.team.path / "MEMORY.md").read_text())
        self.assertEqual(sync.quick(self.team, time.monotonic()).state, sync.REPAIR)

        self.assertEqual(push.resume(self.team, log=logs), push.EXIT_CONFLICT)
        (worktree / "MEMORY.md").write_text("# Team memory\n\nDeploys happen on Wednesdays and Thursdays.\n")
        self.assertEqual(push.resume(self.team, log=logs), 0, logs.text())
        self.assertIn("Wednesdays and Thursdays", self.remote_file("team", "MEMORY.md"))
        self.assertFalse(repair.active(self.team))
        self.assertFalse(worktree.exists())
        self.assertEqual(self.head(self.team), self.head(self.team, "origin/main"))
        self.assertIn("Wednesdays and Thursdays", (self.team.path / "MEMORY.md").read_text())

    def test_abort_keeps_the_commit_for_later(self):
        self.push_from_elsewhere("team", {"MEMORY.md": "theirs\n"})
        self.write(self.team, "MEMORY.md", "mine\n")
        self.assertEqual(push.push_book(self.team, caller(), "m", log=Logs()), push.EXIT_CONFLICT)
        self.assertEqual(push.abort(self.team, log=Logs()), 0)
        self.assertFalse(repair.active(self.team))
        self.assertEqual(push.ahead_count(self.team), 1)

    def test_offline_push_commits_and_the_stop_sweep_publishes_later(self):
        self.write(self.team, "later.md", "later\n")
        logs = Logs()
        with mock.patch.object(bk.Book, "remote", property(lambda book: "/nonexistent/team.git")):
            run_git(self.team.path, "remote", "set-url", "origin", "/nonexistent/team.git")
            self.assertEqual(push.push_book(self.team, caller(), "Later", log=logs), 0)
        self.assertIn("Cadri pushes it after this turn", logs.text())
        self.assertEqual(push.ahead_count(self.team), 1)
        run_git(self.team.path, "remote", "set-url", "origin", str(self.remotes / "team.git"))
        self.assertEqual(push.sweep(self.team, "claude", log=logs), 0, logs.text())
        self.assertEqual(self.remote_file("team", "later.md"), "later")

    def test_a_rejected_push_leaves_the_edits_uncommitted(self):
        hook = self.remotes / "team.git" / "hooks" / "pre-receive"
        hook.write_text("#!/bin/sh\necho 'missing Cadri-Author-Kind trailer' >&2\nexit 1\n")
        hook.chmod(0o755)
        self.write(self.team, "x.md", "x\n")
        before = self.head(self.team)
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "x", log=logs), 1)
        self.assertIn("rejected", logs.text())
        self.assertEqual(self.head(self.team), before)
        self.assertIn("x.md", push.changed_paths(self.team))

    def test_oversized_or_deep_files_are_refused_before_committing(self):
        self.write(self.team, "big.md", "x" * (push.MAX_FILE_CHARS + 1))
        self.write(self.team, "a/b/c/deep.md", "deep\n")
        logs = Logs()
        self.assertEqual(push.push_book(self.team, caller(), "big", log=logs), 1)
        self.assertIn("big.md", logs.text())
        self.assertIn("a/b/c/deep.md", logs.text())
        self.assertEqual(push.ahead_count(self.team), 0)

    def test_sweep_commits_quiet_files_and_leaves_fresh_ones(self):
        self.write(self.team, "quiet.md", "quiet\n", age=600)
        self.write(self.team, "fresh.md", "fresh\n")
        self.assertEqual(push.sweep(self.team, "cursor", log=Logs()), 0)
        self.assertEqual(self.remote_file("team", "quiet.md"), "quiet")
        self.assertEqual(push.changed_paths(self.team), ["fresh.md"])

    def test_push_waits_for_the_book_lock(self):
        self.write(self.team, "x.md", "x\n")
        logs = Logs()
        with mock.patch.object(push, "LOCK_SECONDS", 0.2), locked(self.team.lock_path):
            self.assertEqual(push.push_book(self.team, caller(), "x", log=logs), 1)
        self.assertIn("busy", logs.text())


class SyncTests(BooksTestCase):
    def quick(self, book, new):
        with mock.patch.object(sync, "server_has_new", return_value=new):
            return sync.quick(book, time.monotonic() + 5)

    def test_up_to_date(self):
        status = self.quick(self.team, False)
        self.assertEqual(status.state, sync.CLEAN)
        self.assertFalse(status.needs_background)
        self.assertIsNotNone(bk.last_synced(self.team))

    def test_fast_forwards_when_the_server_moved(self):
        self.push_from_elsewhere("team", {"new.md": "new\n"})
        self.assertEqual(self.quick(self.team, True).state, sync.CLEAN)
        self.assertEqual((self.team.path / "new.md").read_text(), "new\n")

    def test_offline_uses_the_local_copy(self):
        status = self.quick(self.team, None)
        self.assertEqual(status.state, sync.OFFLINE)
        self.assertTrue(status.needs_background)

    def test_dirty_edits_block_the_update_without_touching_them(self):
        self.push_from_elsewhere("team", {"MEMORY.md": "theirs\n"})
        self.write(self.team, "MEMORY.md", "mine, unsaved\n")
        self.assertEqual(self.quick(self.team, True).state, sync.DIRTY_BLOCKED)
        self.assertEqual((self.team.path / "MEMORY.md").read_text(), "mine, unsaved\n")

    def test_local_commits_and_server_changes_are_merged_in_the_background(self):
        self.write(self.team, "mine.md", "mine\n")
        run_git(self.team.path, "add", "mine.md")
        run_git(self.team.path, "commit", "--quiet", "-m", "local")
        self.push_from_elsewhere("team", {"theirs.md": "theirs\n"})
        status = self.quick(self.team, True)
        self.assertEqual(status.state, sync.DIVERGED)
        self.assertEqual(sync.full(self.team, log=Logs()), 0)
        self.assertEqual(self.remote_file("team", "mine.md"), "mine")
        self.assertEqual(self.head(self.team), self.head(self.team, "origin/main"))

    def test_a_missing_clone_is_cloned_in_the_background(self):
        import shutil

        shutil.rmtree(self.user.path)
        self.assertEqual(sync.quick(self.user, time.monotonic() + 5).state, sync.MISSING)
        self.assertEqual(sync.full(self.user, log=Logs()), 0)
        self.assertTrue((self.user.path / "MEMORY.md").exists())

    def test_a_broken_clone_is_moved_aside_and_recloned(self):
        (self.team.path / ".git" / "HEAD").write_text("garbage\n")
        self.assertEqual(self.quick(self.team, False).state, sync.CORRUPT)
        self.assertEqual(sync.full(self.team, log=Logs()), 0)
        self.assertTrue((self.team.path / "MEMORY.md").exists())
        self.assertTrue(list(self.team.org_dir.glob("team.corrupt-*")))

    def test_a_busy_book_is_skipped_quickly(self):
        with locked(self.team.lock_path):
            started = time.monotonic()
            self.assertEqual(sync.quick(self.team, time.monotonic() + 5).state, sync.BUSY)
        self.assertLess(time.monotonic() - started, 2)


class Response:
    def __init__(self, status=200, body=None):
        self.status = status
        self.body = json.dumps(body or {}).encode()
        self.headers = {}

    def read(self):
        return self.body

    def __enter__(self):
        return self

    def __exit__(self, *_):
        return False


class FakeServer:
    def __init__(self, routes):
        self.routes = routes
        self.requests = []

    def __call__(self, request, timeout=None):
        url = request.full_url
        self.requests.append({"url": url, "method": request.get_method(), "body": json.loads(request.data) if request.data else None})
        status, body = self.routes[url.split(BASE)[1].split("?")[0]]
        if status >= 300:
            raise urllib.error.HTTPError(url, status, "x", {}, io.BytesIO(json.dumps(body).encode()))
        return Response(status, body)


class CliTests(BooksTestCase):
    def cli(self, *argv, stdin="", server=None):
        out, err = io.StringIO(), io.StringIO()
        server = server or FakeServer({"/api/memories/tip": (304, {})})
        with redirect_stdout(out), redirect_stderr(err), mock.patch("sys.stdin", io.StringIO(stdin)), mock.patch(
            "urllib.request.urlopen", server
        ):
            code = cli.main(list(argv))
        return code, out.getvalue(), err.getvalue()

    def test_memory_push_both_books_and_finish_the_digest(self):
        from cadri.memory import digests

        digests.save("claude", "s1", "how do we deploy?", "On Tuesdays.", 0, [])
        claim = digests.gate("s9", now=time.time() + 3 * 3600) or self.fail("expected an orphan claim")
        self.write(self.team, "operations.md", "Deploys need a freeze notice.\n")
        code, _, err = self.cli(
            "memory", "push", "--agent", "cadri-memory-keeper", "--session", "s1", "--harness", "claude",
            "--digest", str(claim.file), "-m", "Record the deploy freeze",
        )
        self.assertEqual(code, 0, err)
        self.assertIn("pushed 1 commit to the team book", err)
        self.assertFalse(claim.directory.exists())
        self.assertIn("Claude Code memory keeper", self.remote_log("team"))

    def test_memory_push_human_uses_the_cached_identity(self):
        self.write(self.user, "prefs.md", "Tabs.\n")
        code, _, err = self.cli("memory", "push", "--book", "user", "--human", "-m", "Remember tabs")
        self.assertEqual(code, 0, err)
        self.assertIn("Cadri-Author-Label: me@example.com", self.remote_log("user"))

    def test_memory_status_and_context(self):
        self.write(self.team, "loose.md", "loose\n")
        code, out, _ = self.cli("memory", "status")
        self.assertEqual(code, 0)
        self.assertIn("uncommitted files: loose.md", out)
        with mock.patch.dict(os.environ, {"CLAUDE_PLUGIN_ROOT": "/plugin"}):
            code, out, _ = self.cli("memory", "context", "--harness", "claude")
        self.assertEqual(code, 0)
        self.assertIn("Deploys happen on Tuesdays", out)
        self.assertIn("cadri:cadri-memory-keeper", out)

    def test_write_through_the_edit_api_sends_base_agent_and_session(self):
        head = self.head(self.team)
        server = FakeServer({"/api/memories/commits": (200, {"commit": "c" * 40}), "/api/memories/tip": (304, {})})
        with mock.patch.dict(os.environ, {"CADRI_SESSION_ID": "from-env"}):
            code, out, err = self.cli("memory", "write", "notes.md", "-m", "Add notes", "--agent", "bot-1", stdin="hello\n", server=server)
        self.assertEqual(code, 0, err)
        post = next(r for r in server.requests if r["method"] == "POST")
        self.assertEqual(post["url"], f"{BASE}/api/memories/commits?view=tree")
        self.assertEqual(
            post["body"],
            {
                "message": "Add notes",
                "changes": [{"op": "write", "path": "notes.md", "content": "hello\n"}],
                "agent": "bot-1",
                "base": head,
                "session_id": "from-env",
            },
        )

    def test_write_exits_3_on_a_conflict_and_lists_the_paths(self):
        server = FakeServer({"/api/memories/commits": (409, {"error": "conflict", "paths": ["deploy.md"]})})
        code, _, err = self.cli("memory", "write", "deploy.md", "-m", "m", stdin="x", server=server)
        self.assertEqual(code, editapi.EXIT_CONFLICT)
        self.assertIn("deploy.md", err)


class ScaffoldTests(BooksTestCase):
    seed_team = {}

    def test_scaffold_writes_team_indexes_only_when_the_book_is_empty(self):
        server = FakeServer({"/api/memories/commits": (200, {"commit": "c" * 40}), "/api/memories/tip": (304, {})})
        with mock.patch("urllib.request.urlopen", server):
            self.assertFalse(editapi.scaffold_team(self.user))
            self.assertTrue(editapi.scaffold_team(self.team))
        post = next(request for request in server.requests if request["method"] == "POST")
        self.assertEqual([change["path"] for change in post["body"]["changes"]], [path for path, _ in TEAM_LAYOUT])
        self.assertEqual(post["body"]["agent"], "cadri-init")
        self.assertNotIn("base", post["body"])


class LessonTests(unittest.TestCase):
    def test_indexes_are_not_lessons_and_git_metadata_is_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / ".git").mkdir()
            (root / ".git" / "notes.md").write_text("not a lesson")
            (root / "MEMORY.md").write_text("# Team")
            (root / "architecture").mkdir()
            (root / "architecture" / "MEMORY.md").write_text("# Architecture")
            (root / "README.md").write_text("legacy index")
            self.assertFalse(has_lessons(root))
            self.assertFalse(has_lessons(root / "missing"))
            (root / "architecture" / "decisions.md").write_text("---\nname: Decisions\n---\n")
            self.assertTrue(has_lessons(root))


if __name__ == "__main__":
    unittest.main()
