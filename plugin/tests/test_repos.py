"""Repo discovery for the seed window: encoded project paths, git roots, and the choice."""

from __future__ import annotations

import argparse
import io
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from cadri import cli  # noqa: E402
from cadri.discover.repos import (  # noqa: E402
    RepoCount,
    choose_repo,
    decode_project_path,
    encoded_project_name,
    format_repo_choice,
    git_toplevel,
    roots_for_sessions,
)
from cadri.discover.types import SessionRef  # noqa: E402
from support import run_git  # noqa: E402


def ref(harness: str, thread_id: str, path: Path, updated_at: float = 1_800_000_000.0) -> SessionRef:
    return SessionRef(harness, thread_id, path, updated_at)


class DecodeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)
        (self.root / "Users" / "me" / "cadri" / "cadri").mkdir(parents=True)
        (self.root / "Users" / "me" / "my-app").mkdir()

    def test_slashes_round_trip_when_that_path_exists(self):
        found = decode_project_path("Users-me-cadri-cadri", root=self.root)
        self.assertEqual(found, self.root / "Users" / "me" / "cadri" / "cadri")

    def test_a_leading_dash_is_the_claude_root_slash(self):
        found = decode_project_path("-Users-me-cadri-cadri", root=self.root)
        self.assertEqual(found, self.root / "Users" / "me" / "cadri" / "cadri")

    def test_a_hyphen_inside_a_name_stays_when_the_split_path_is_missing(self):
        found = decode_project_path("Users-me-my-app", root=self.root)
        self.assertEqual(found, self.root / "Users" / "me" / "my-app")

    def test_a_missing_path_decodes_to_nothing(self):
        self.assertIsNone(decode_project_path("Users-me-missing", root=self.root))

    def test_project_names_come_from_the_session_path(self):
        cursor = ref(
            "cursor",
            "t1",
            Path("/home/x/.cursor/projects/Users-me-app/agent-transcripts/t1/t1.jsonl"),
        )
        claude = ref("claude_code", "t2", Path("/home/x/.claude/projects/-Users-me-app/t2.jsonl"))
        codex = ref("codex", "t3", Path("/home/x/.codex/sessions/2026/01/01/rollout.jsonl"))
        self.assertEqual(encoded_project_name(cursor), "Users-me-app")
        self.assertEqual(encoded_project_name(claude), "-Users-me-app")
        self.assertIsNone(encoded_project_name(codex))


class ChoiceTests(unittest.TestCase):
    def test_the_current_directory_wins_when_it_sits_inside_a_root(self):
        counts = [RepoCount("/work/app", 4), RepoCount("/work/other", 20)]
        choice = choose_repo(counts, Path("/work/app/plugin"))
        self.assertEqual((choice.brief, choice.reason), ("/work/app", "cwd"))
        text = format_repo_choice(choice)
        self.assertIn("Brief /work/app. This agent is running inside it.", text)
        self.assertIn("Leave the others", text)

    def test_a_clear_majority_wins_outside_every_root(self):
        counts = [RepoCount("/work/app", 40), RepoCount("/work/other", 8)]
        choice = choose_repo(counts, Path("/tmp"))
        self.assertEqual((choice.brief, choice.reason, choice.ask), ("/work/app", "majority", ()))
        self.assertIn("most sessions", format_repo_choice(choice))

    def test_close_counts_ask_and_a_stray_stays_out_of_the_question(self):
        counts = [RepoCount("/work/app", 22), RepoCount("/work/other", 18), RepoCount("/work/stray", 1)]
        choice = choose_repo(counts, Path("/tmp"))
        self.assertEqual(choice.reason, "ask")
        self.assertEqual(choice.brief, None)
        self.assertEqual([item.path for item in choice.ask], ["/work/app", "/work/other"])
        self.assertEqual(len(choice.seen), 3)
        self.assertIn("Ask which repo to brief before you read.", format_repo_choice(choice))

    def test_no_roots_writes_no_briefing(self):
        choice = choose_repo([], Path("/tmp"))
        self.assertEqual(choice.reason, "none")
        self.assertIn("Write no repository briefing.", format_repo_choice(choice))


class RootTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.addCleanup(self.tmp.cleanup)

    def test_sessions_count_toward_the_git_root(self):
        app = self.root / "Users" / "me" / "app"
        other = self.root / "Users" / "me" / "other"
        loose = self.root / "Users" / "me" / "notes"
        app.mkdir(parents=True)
        (app / "plugin").mkdir()
        other.mkdir()
        loose.mkdir()
        run_git(app, "init")
        run_git(other, "init")
        home = self.root / "home"
        cursor_app = home / ".cursor/projects/Users-me-app/agent-transcripts/a/a.jsonl"
        cursor_other = home / ".cursor/projects/Users-me-other/agent-transcripts/b/b.jsonl"
        claude_loose = home / ".claude/projects/-Users-me-notes/c.jsonl"
        for path in (cursor_app, cursor_other, claude_loose):
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}\n")
        codex = home / ".codex/sessions/day/rollout.jsonl"
        codex.parent.mkdir(parents=True)
        codex.write_text(json.dumps({"type": "session_meta", "payload": {"cwd": str(app / "plugin")}}) + "\n")
        database = home / "opencode.db"
        connection = sqlite3.connect(database)
        connection.execute("CREATE TABLE session (id TEXT, directory TEXT)")
        connection.execute("INSERT INTO session VALUES (?, ?)", ("oc1", str(other)))
        connection.commit()
        connection.close()
        refs = [
            ref("cursor", "a", cursor_app),
            ref("cursor", "b", cursor_other),
            ref("claude_code", "c", claude_loose),
            ref("codex", "d", codex),
            ref("opencode", "oc1", database),
        ]
        counts = roots_for_sessions(refs, decode_root=self.root)
        self.assertEqual(
            [(item.path, item.sessions) for item in counts],
            [(str(app.resolve()), 2), (str(other.resolve()), 2)],
        )
        self.assertEqual(git_toplevel(loose), None)

    def test_seed_json_names_the_repo(self):
        counts = [RepoCount("/work/app", 4)]
        args = argparse.Namespace(json=True, seed=True, days=None, limit=None)
        stdout = io.StringIO()
        stderr = io.StringIO()
        with (
            mock.patch("cadri.cli.discover_all", return_value=[ref("cursor", "t1", Path("t1"))]),
            mock.patch("cadri.cli.roots_for_sessions", return_value=counts),
            mock.patch("sys.stdout", stdout),
            mock.patch("sys.stderr", stderr),
        ):
            self.assertEqual(cli.cmd_discover(args), 0)
        payload = json.loads(stdout.getvalue())
        self.assertEqual(payload["sessions"][0]["thread_id"], "t1")
        self.assertEqual(payload["repos"]["brief"], "/work/app")
        self.assertEqual(payload["repos"]["reason"], "majority")
        self.assertIn("seed window: 1 sessions", stderr.getvalue())
        self.assertIn("Brief /work/app.", stderr.getvalue())

    def test_discover_without_seed_stays_a_list(self):
        args = argparse.Namespace(json=True, seed=False, days=None, limit=None)
        stdout = io.StringIO()
        with (
            mock.patch("cadri.cli.discover_all", return_value=[ref("cursor", "t1", Path("t1"))]),
            mock.patch("sys.stdout", stdout),
        ):
            self.assertEqual(cli.cmd_discover(args), 0)
        self.assertEqual(json.loads(stdout.getvalue())[0]["thread_id"], "t1")
