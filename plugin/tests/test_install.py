"""Hook installs for each harness, and the installed hook commands run end to end."""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
import sys
import time
import unittest
from pathlib import Path
from unittest import mock

from test_plugin import PluginTestCase

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from shared import cadri_auth, install  # noqa: E402
from shared.discover import codex, opencode  # noqa: E402


class InstallTestCase(PluginTestCase):
    def setUp(self):
        super().setUp()
        patcher = mock.patch.dict(
            os.environ,
            {"HOME": str(self.home), "PATH": "/nonexistent"},
        )
        patcher.start()
        self.addCleanup(patcher.stop)
        for key in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME"):
            os.environ.pop(key, None)

    def read(self, relative: str) -> dict:
        return json.loads((self.home / relative).read_text(encoding="utf-8"))

    def write(self, relative: str, data: object) -> Path:
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data), encoding="utf-8")
        return path

    def run_hook(self, command: str, event: dict, foreground: bool = True) -> subprocess.CompletedProcess:
        env = self.env()
        if not foreground:
            env.pop("CADRI_HOOK_FOREGROUND")
        return subprocess.run(
            command,
            shell=True,
            input=json.dumps(event),
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )


class ClaudeInstallTests(InstallTestCase):
    def test_merges_into_existing_settings_and_reinstalls_cleanly(self):
        user_hook = {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo mine"}]}
        self.write(".claude/settings.json", {"model": "opus", "hooks": {"Stop": [user_hook]}})
        install.install_harness("claude_code", self.home)
        install.install_harness("claude_code", self.home)
        settings = self.read(".claude/settings.json")
        self.assertEqual(settings["model"], "opus")
        self.assertEqual(set(settings["hooks"]), set(install.CLAUDE_EVENTS))
        self.assertEqual(settings["hooks"]["Stop"][0], user_hook)
        for event in install.CLAUDE_EVENTS:
            cadri = [g for g in settings["hooks"][event] if install.is_cadri_command(g["hooks"][0]["command"])]
            self.assertEqual(len(cadri), 1, event)

        install.uninstall_all(self.home)
        self.assertEqual(self.read(".claude/settings.json"), {"model": "opus", "hooks": {"Stop": [user_hook]}})

    def test_installed_command_uploads_the_session(self):
        install.install_harness("claude_code", self.home)
        command = self.read(".claude/settings.json")["hooks"]["Stop"][0]["hooks"][0]["command"]
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(command, event)
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "s1")
        self.assertEqual(upload["body"]["harness"], "claude_code")
        self.assertEqual(upload["body"]["content"], '{"type":"user"}\n')

    def test_hook_without_cadri_url_uses_the_server_init_saved(self):
        install.install_harness("claude_code", self.home)
        cadri_auth.save_url(self.server.url)
        command = self.read(".claude/settings.json")["hooks"]["Stop"][0]["hooks"][0]["command"]
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        env = {"HOME": str(self.home), "PATH": "/usr/bin:/bin", "CADRI_HOOK_FOREGROUND": "1"}
        result = subprocess.run(command, shell=True, input=json.dumps(event), env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.uploads()), 1)

    def test_detached_hook_returns_before_the_upload(self):
        install.install_harness("claude_code", self.home)
        command = self.read(".claude/settings.json")["hooks"]["Stop"][0]["hooks"][0]["command"]
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(command, event, foreground=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        deadline = time.monotonic() + 15
        while not self.uploads() and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertEqual(len(self.uploads()), 1)

    def test_invalid_settings_are_left_alone(self):
        path = self.home / ".claude" / "settings.json"
        path.parent.mkdir(parents=True)
        path.write_text("{not json", encoding="utf-8")
        with self.assertRaises(install.InstallError):
            install.install_harness("claude_code", self.home)
        self.assertEqual(path.read_text(encoding="utf-8"), "{not json")

    def test_skills_land_where_each_harness_reads_them(self):
        install.install_skills(self.home, ["claude_code", "cursor"])
        for root in (".claude/skills", ".agents/skills"):
            for name in ("cadri-install", "cadri-memory", "cadri-memory-seed"):
                self.assertTrue((self.home / root / name / "SKILL.md").is_file(), f"{root}/{name}")
        install.uninstall_all(self.home)
        self.assertFalse((self.home / ".agents/skills/cadri-memory").exists())

    def test_removes_paths_from_earlier_releases(self):
        (self.home / ".claude/plugins/cadri/hooks").mkdir(parents=True)
        self.write(".cursor/cadri-mcp.json", {})
        install.remove_legacy(self.home)
        self.assertFalse((self.home / ".claude/plugins/cadri").exists())
        self.assertFalse((self.home / ".cursor/cadri-mcp.json").exists())


class CursorInstallTests(InstallTestCase):
    def test_keeps_other_hooks_and_removes_the_file_it_created(self):
        install.install_harness("cursor", self.home)
        hooks = self.read(".cursor/hooks.json")
        self.assertEqual(hooks["version"], 1)
        self.assertEqual(set(hooks["hooks"]), set(install.CURSOR_EVENTS))
        install.uninstall_all(self.home)
        self.assertFalse((self.home / ".cursor/hooks.json").exists())

        mine = {"command": "./audit.sh", "failClosed": True}
        self.write(".cursor/hooks.json", {"version": 1, "hooks": {"stop": [mine]}})
        install.install_harness("cursor", self.home)
        self.assertEqual(self.read(".cursor/hooks.json")["hooks"]["stop"][0], mine)
        install.uninstall_all(self.home)
        self.assertEqual(self.read(".cursor/hooks.json"), {"version": 1, "hooks": {"stop": [mine]}})

    def test_stop_hook_finds_the_session_by_conversation_id(self):
        install.install_harness("cursor", self.home)
        command = self.read(".cursor/hooks.json")["hooks"]["stop"][0]["command"]
        folder = self.home / ".cursor/projects/Users-me-app/agent-transcripts/c1"
        folder.mkdir(parents=True)
        (folder / "c1.jsonl").write_text('{"role":"user"}\n', encoding="utf-8")
        event = {
            "conversation_id": "c1",
            "hook_event_name": "stop",
            "transcript_path": None,
            "workspace_roots": ["/Users/me/app"],
            "model": "claude-opus",
        }
        result = self.run_hook(command, event)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "c1")
        self.assertEqual(upload["body"]["harness"], "cursor")
        self.assertEqual(upload["body"]["cwd"], "/Users/me/app")
        self.assertEqual(upload["body"]["model"], "claude-opus")

    def test_session_start_without_a_file_uploads_nothing(self):
        install.install_harness("cursor", self.home)
        command = self.read(".cursor/hooks.json")["hooks"]["stop"][0]["command"]
        result = self.run_hook(command, {"conversation_id": "c2", "hook_event_name": "stop"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.uploads(), [])


class CodexInstallTests(InstallTestCase):
    def test_stop_hook_uploads_the_rollout(self):
        install.install_harness("codex", self.home)
        hooks = self.read(".codex/hooks.json")["hooks"]
        self.assertEqual(set(hooks), set(install.CODEX_EVENTS))
        event = {"session_id": "019a-codex", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(hooks["Stop"][0]["hooks"][0]["command"], event)
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "019a-codex")
        self.assertEqual(upload["body"]["harness"], "codex")

    def test_backfill_ids_match_hook_session_ids(self):
        path = Path("rollout-2026-11-05T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl")
        self.assertEqual(codex.thread_id(path), "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")


class OpenCodeInstallTests(InstallTestCase):
    def test_plugin_runs_the_hook_with_the_session_export(self):
        install.install_harness("opencode", self.home)
        plugin = (self.home / ".config/opencode/plugins/cadri.ts").read_text(encoding="utf-8")
        self.assertIn(install.OPENCODE_MARKER, plugin)
        self.assertNotIn("__CADRI_COMMAND__", plugin)
        command = json.loads(plugin.split("const COMMAND: string[] = ", 1)[1].splitlines()[0])
        self.assertEqual(command[-2:], ["hook", "opencode"])

        db = self.home / ".local/share/opencode/opencode.db"
        db.parent.mkdir(parents=True)
        connection = sqlite3.connect(db)
        with connection:
            # OpenCode 1.19's layout, including the session_message table it leaves empty.
            connection.execute("CREATE TABLE session (id TEXT, title TEXT, time_updated INTEGER)")
            connection.execute(
                "CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT)"
            )
            connection.execute("CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)")
            connection.execute(
                "CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT)"
            )
            connection.execute("INSERT INTO session VALUES ('ses_1', 'ping', 1)")
            connection.execute("INSERT INTO message VALUES ('m1', 'ses_1', 1, '{\"role\":\"user\"}')")
            connection.execute("INSERT INTO part VALUES ('p1', 'm1', 'ses_1', 1, '{\"type\":\"text\",\"text\":\"ping\"}')")
        connection.close()
        result = subprocess.run(
            command,
            input=json.dumps({"session_id": "ses_1", "hook_event_name": "session.idle", "cwd": "/w"}),
            env=self.env(),
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["harness"], "opencode")
        content = json.loads(upload["body"]["content"])
        self.assertEqual(content["info"]["title"], "ping")
        self.assertEqual(
            content["messages"],
            [{"id": "m1", "role": "user", "parts": [{"type": "text", "text": "ping"}]}],
        )

    def test_exports_opencode_2_sessions(self):
        db = self.home / "opencode.db"
        connection = sqlite3.connect(db)
        with connection:
            connection.execute("CREATE TABLE session_v2 (id TEXT, title TEXT, time_updated INTEGER)")
            connection.execute(
                "CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT)"
            )
            connection.execute("INSERT INTO session_v2 VALUES ('ses_2', 'pong', 1)")
            connection.execute("INSERT INTO session_message VALUES ('b', 'ses_2', 'assistant', 2, '{\"text\":\"pong\"}')")
            connection.execute("INSERT INTO session_message VALUES ('a', 'ses_2', 'user', 1, '{\"text\":\"ping\"}')")
        connection.close()
        content = json.loads(opencode.export_session_json(db, "ses_2"))
        self.assertEqual(content["info"]["title"], "pong")
        self.assertEqual(content["messages"], [{"text": "ping"}, {"text": "pong"}])

    def test_refuses_to_overwrite_a_plugin_it_did_not_write(self):
        path = self.home / ".config/opencode/plugins/cadri.ts"
        path.parent.mkdir(parents=True)
        path.write_text("export const Mine = 1\n", encoding="utf-8")
        with self.assertRaises(install.InstallError):
            install.install_harness("opencode", self.home)
        self.assertEqual(path.read_text(encoding="utf-8"), "export const Mine = 1\n")


class DetectionTests(InstallTestCase):
    def test_config_dirs_count_without_sessions(self):
        (self.home / ".cursor").mkdir()
        (self.home / ".codex").mkdir()
        self.assertEqual(install.installed_harnesses(self.home)[-2:], ["cursor", "codex"])


if __name__ == "__main__":
    unittest.main()
