"""Bundles and installs for each harness, and the installed hook commands run end to end."""

from __future__ import annotations

import json
import os
import shutil
import sqlite3
import subprocess
import sys
import time
import tomllib
import unittest
from pathlib import Path
from unittest import mock

from test_plugin import PluginTestCase

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from cadri import auth as cadri_auth  # noqa: E402
from cadri import cli  # noqa: E402
from cadri.discover import codex, cursor, opencode  # noqa: E402
from cadri.install import bundles, config  # noqa: E402

# Older hook entries named only the harness; install still recognizes and replaces them.
LEGACY_COMMAND = "/usr/bin/python3 /opt/cadri/plugin/bin/cadri_cli.py hook {harness}"
NODE = shutil.which("node")


class InstallTestCase(PluginTestCase):
    def setUp(self):
        super().setUp()
        patcher = mock.patch.dict(os.environ, {"HOME": str(self.home), "PATH": "/nonexistent"})
        patcher.start()
        self.addCleanup(patcher.stop)
        for key in ("CLAUDE_CONFIG_DIR", "CODEX_HOME", "XDG_CONFIG_HOME", "CADRI_HOME"):
            os.environ.pop(key, None)

    def read(self, relative: str) -> dict:
        return json.loads((self.home / relative).read_text(encoding="utf-8"))

    def write(self, relative: str, data: object) -> Path:
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(data if isinstance(data, str) else json.dumps(data), encoding="utf-8")
        return path

    def run_hook(self, command: str, event: dict, foreground: bool = True) -> subprocess.CompletedProcess:
        env = self.env(PATH="/usr/bin:/bin")
        if not foreground:
            env.pop("CADRI_HOOK_FOREGROUND")
        return subprocess.run(command, shell=True, input=json.dumps(event), env=env, capture_output=True, text=True, timeout=30)

    def install(self, *harnesses: str) -> list[str]:
        return bundles.install(list(harnesses), self.home)

    def fake_cli(self, name: str) -> Path:
        """An executable `name` on PATH that records its arguments."""
        bin_dir = self.home / "fakebin"
        bin_dir.mkdir(exist_ok=True)
        log = self.home / f"{name}.log"
        script = bin_dir / name
        script.write_text(f'#!/bin/sh\necho "$@" >> "{log}"\n', encoding="utf-8")
        script.chmod(0o755)
        os.environ["PATH"] = str(bin_dir)
        return log


def hook_commands(groups: dict) -> list[dict]:
    return [hook for entries in groups.values() for group in entries for hook in group.get("hooks", [group])]


class BundleTests(InstallTestCase):
    def test_release_bundles_call_the_shim_with_explicit_timeouts(self):
        dist = self.home / "dist"
        bundles.main([str(dist)])
        claude = json.loads((dist / "claude/cadri/hooks/hooks.json").read_text())["hooks"]
        codex_hooks = json.loads((dist / "codex/hooks.json").read_text())["hooks"]
        cursor_hooks = json.loads((dist / "cursor/hooks/hooks.json").read_text())["hooks"]
        for name, groups in (("claude", claude), ("codex", codex_hooks), ("cursor", cursor_hooks)):
            self.assertNotIn("SessionEnd", groups)
            self.assertNotIn("sessionEnd", groups)
            for hook in hook_commands(groups):
                self.assertTrue(hook["command"].startswith('"$HOME/.cadri/bin/cadri" hook '), hook)
                self.assertTrue(hook["command"].endswith(f"--harness {name}"), hook)
                self.assertIsInstance(hook["timeout"], int)
        self.assertEqual(claude["PostToolUse"][0]["matcher"], "Write|Edit|MultiEdit|NotebookEdit")
        self.assertTrue(claude["PostToolUse"][0]["hooks"][0]["async"])
        self.assertEqual(codex_hooks["SessionStart"][0]["hooks"][0]["additionalContextLimit"], 4000)
        self.assertEqual(set(cursor_hooks), {h.event for h in bundles.CURSOR_HOOKS})

        for skills in (dist / "claude/cadri/skills", dist / "cursor/skills", dist / "codex/skills"):
            self.assertEqual(sorted(p.name for p in skills.iterdir()), sorted(bundles.SHIPPED_SKILLS))
        marketplace = json.loads((dist / "claude/.claude-plugin/marketplace.json").read_text())
        self.assertEqual(marketplace["plugins"][0]["source"], "./cadri")
        manifest = json.loads((dist / "cursor/.cursor-plugin/plugin.json").read_text())
        for key in ("rules", "skills", "agents", "hooks"):
            self.assertTrue((dist / "cursor" / manifest[key]).exists(), key)

    def test_keeper_agents_use_each_harness_format(self):
        dist = self.home / "dist"
        bundles.main([str(dist)])
        claude = (dist / "claude/cadri/agents/cadri-memory-keeper.md").read_text()
        self.assertTrue(claude.startswith("---\nname: cadri-memory-keeper\n"))
        self.assertIn("background: true", claude)
        self.assertIn("`cadri:cadri-memory-repair`", claude)
        self.assertIn("cadri memory push", claude)
        cursor_agent = (dist / "cursor/agents/cadri-memory-keeper.md").read_text()
        self.assertIn("is_background: true", cursor_agent)
        role = tomllib.loads((dist / "codex/agents/cadri-memory-keeper.toml").read_text())
        self.assertEqual(role["name"], "cadri-memory-keeper")
        self.assertIn("cadri memory push", role["developer_instructions"])
        opencode_agent = (dist / "opencode/agents/cadri-memory-keeper.md").read_text()
        self.assertTrue(opencode_agent.startswith("---\n# cadri-managed"))
        self.assertIn('resource: "~/.cadri/memory/**"', opencode_agent)
        plugin = (dist / "opencode/cadri.ts").read_text()
        self.assertIn('(["$HOME/.cadri/bin/cadri"] as string[])', plugin)
        self.assertIn('expand("$HOME/.cadri/plugins/opencode/skills")', plugin)

    def test_shim_runs_this_checkout(self):
        shim = bundles.write_shim()
        self.assertEqual(shim, self.home / ".cadri/bin/cadri")
        self.assertTrue(os.access(shim, os.X_OK))
        result = subprocess.run([str(shim), "--help"], capture_output=True, text=True, env=self.env(), timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("memory", result.stdout)


class ClaudeInstallTests(InstallTestCase):
    def command(self, event: str) -> str:
        return self.read(".claude/settings.json")["hooks"][event][-1]["hooks"][0]["command"]

    def test_without_the_cli_hooks_go_into_settings_and_uninstall_restores_them(self):
        user_hook = {"matcher": "Bash", "hooks": [{"type": "command", "command": "echo mine"}]}
        original = {"model": "opus", "hooks": {"Stop": [user_hook]}, "permissions": {"allow": ["Bash(ls:*)"]}}
        self.write(".claude/settings.json", original)
        self.install("claude_code")
        [line] = self.install("claude_code")
        self.assertIn("hooks in", line)
        settings = self.read(".claude/settings.json")
        self.assertEqual(settings["model"], "opus")
        self.assertEqual(set(settings["hooks"]), {h.event for h in bundles.CLAUDE_HOOKS})
        self.assertEqual(settings["hooks"]["Stop"][0], user_hook)
        for event in settings["hooks"]:
            cadri = [g for g in settings["hooks"][event] if config.is_cadri_command(g["hooks"][0]["command"])]
            self.assertEqual(len(cadri), 1, event)
        memory = "~/.cadri/memory"
        self.assertEqual(settings["permissions"]["additionalDirectories"], [memory])
        allow = settings["permissions"]["allow"]
        self.assertIn(f"Edit({memory}/**)", allow)
        self.assertIn(f"Read({memory}/**)", allow)
        self.assertIn("Read(~/.cadri/state/digests/**)", allow)
        self.assertIn("Bash(cadri memory:*)", allow)
        self.assertNotIn(f"Write({memory}/**)", allow)
        self.assertTrue((self.home / ".claude/skills/cadri-memory/SKILL.md").is_file())
        keeper = (self.home / ".claude/agents/cadri-memory-keeper.md").read_text()
        self.assertIn("`cadri-memory-repair`", keeper)

        bundles.uninstall(self.home)
        self.assertEqual(self.read(".claude/settings.json"), original)
        self.assertFalse((self.home / ".claude/skills/cadri-memory").exists())
        self.assertFalse((self.home / ".claude/agents/cadri-memory-keeper.md").exists())

    def test_with_the_cli_installs_the_plugin_from_a_local_marketplace(self):
        log = self.fake_cli("claude")
        self.write(
            ".claude/settings.json",
            {
                "hooks": {"Stop": [{"hooks": [{"type": "command", "command": LEGACY_COMMAND.format(harness="claude_code")}]}]},
                "permissions": {"allow": ["Write(~/.cadri/memory/**)", "Bash(ls:*)"]},
            },
        )
        (self.home / ".claude/skills/cadri-memory").mkdir(parents=True)
        (self.home / ".claude/skills/cadri-memory/SKILL.md").write_text("stale fallback copy")
        [line] = self.install("claude_code")
        self.assertIn("plugin cadri installed", line)
        marketplace = self.home / ".cadri/plugins/claude"
        self.assertEqual(
            log.read_text().splitlines(),
            [f"plugin marketplace add {marketplace}", "plugin install cadri@cadri --scope user"],
        )
        self.assertTrue((marketplace / "cadri/hooks/hooks.json").is_file())
        settings = self.read(".claude/settings.json")
        self.assertNotIn("hooks", settings)
        self.assertNotIn("Write(~/.cadri/memory/**)", settings["permissions"]["allow"])
        self.assertIn("Bash(ls:*)", settings["permissions"]["allow"])
        self.assertFalse((self.home / ".claude/skills/cadri-memory").exists())

        log.unlink()
        bundles.uninstall(self.home)
        self.assertEqual(log.read_text().splitlines(), ["plugin uninstall cadri@cadri --scope user", "plugin marketplace remove cadri"])
        self.assertFalse(marketplace.exists())
        self.assertEqual(self.read(".claude/settings.json"), {"permissions": {"allow": ["Bash(ls:*)"]}})

    def test_installed_command_uploads_the_session(self):
        self.install("claude_code")
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(self.command("Stop"), event)
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "s1")
        self.assertEqual(upload["body"]["harness"], "claude_code")
        self.assertEqual(upload["body"]["content"], '{"type":"user"}\n')

    def test_hook_without_cadri_url_uses_the_server_init_saved(self):
        self.install("claude_code")
        cadri_auth.save_url(self.server.url)
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        env = {"HOME": str(self.home), "PATH": "/usr/bin:/bin", "CADRI_HOOK_FOREGROUND": "1"}
        result = subprocess.run(self.command("Stop"), shell=True, input=json.dumps(event), env=env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(self.uploads()), 1)

    def test_detached_hook_returns_before_the_upload(self):
        self.install("claude_code")
        event = {"session_id": "s1", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(self.command("Stop"), event, foreground=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        deadline = time.monotonic() + 15
        while not self.uploads() and time.monotonic() < deadline:
            time.sleep(0.1)
        self.assertEqual(len(self.uploads()), 1)

    def test_invalid_settings_are_left_alone(self):
        path = self.write(".claude/settings.json", "{not json")
        [line] = self.install("claude_code")
        self.assertIn("not installed", line)
        self.assertEqual(path.read_text(encoding="utf-8"), "{not json")

class CursorInstallTests(InstallTestCase):
    def plugin(self) -> Path:
        return self.home / ".cursor/plugins/local/cadri"

    def command(self, event: str) -> str:
        return json.loads((self.plugin() / "hooks/hooks.json").read_text())["hooks"][event][0]["command"]

    def test_installs_a_local_plugin_and_removes_old_user_hooks(self):
        mine = {"command": "./audit.sh", "failClosed": True}
        old = {"command": LEGACY_COMMAND.format(harness="cursor"), "timeout": 30}
        self.write(".cursor/hooks.json", {"version": 1, "hooks": {"stop": [mine, old], "sessionEnd": [old]}})
        self.install("cursor")
        self.assertEqual(self.read(".cursor/hooks.json"), {"version": 1, "hooks": {"stop": [mine]}})
        rule = (self.plugin() / "rules/cadri.mdc").read_text()
        self.assertIn("alwaysApply: true", rule)
        self.assertIn("cadri memory context", rule)
        self.assertTrue((self.plugin() / "skills/cadri-memory/SKILL.md").is_file())
        self.assertIn(str(self.home / ".cadri/bin/cadri"), self.command("stop"))

        bundles.uninstall(self.home)
        self.assertFalse(self.plugin().exists())
        self.assertEqual(self.read(".cursor/hooks.json"), {"version": 1, "hooks": {"stop": [mine]}})

    def test_stop_hook_finds_the_session_by_conversation_id(self):
        self.install("cursor")
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
        result = self.run_hook(self.command("stop"), event)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "c1")
        self.assertEqual(upload["body"]["harness"], "cursor")
        self.assertNotIn("cwd", upload["body"])
        self.assertEqual(upload["body"]["model"], "claude-opus")

    def test_stop_hook_reads_the_model_from_composer_data(self):
        self.install("cursor")
        folder = self.home / ".cursor/projects/Users-me-app/agent-transcripts/c3"
        folder.mkdir(parents=True)
        (folder / "c3.jsonl").write_text('{"role":"user"}\n', encoding="utf-8")
        self.write_composer("c3", model_name="grok-4.7-high", model_id="grok-4.7")
        result = self.run_hook(self.command("stop"), {"conversation_id": "c3", "hook_event_name": "stop"})
        self.assertEqual(result.returncode, 0, result.stderr)
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["model"], "grok-4.7-high")

    def test_stop_without_a_transcript_uploads_nothing(self):
        self.install("cursor")
        result = self.run_hook(self.command("stop"), {"conversation_id": "c2", "hook_event_name": "stop"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.uploads(), [])

    def test_backfill_sends_the_cursor_model(self):
        thread = "c4"
        folder = self.home / ".cursor/projects/app/agent-transcripts" / thread
        folder.mkdir(parents=True)
        (folder / f"{thread}.jsonl").write_text('{"role":"user","message":{"content":[]}}\n', encoding="utf-8")
        self.write_composer(thread, model_name="", model_id="claude-opus-5-5")
        with mock.patch.object(cli, "post_session", return_value=(201, {})) as post:
            summary = cli.backfill("http://127.0.0.1:9", 0, 10, 10_000_000)
        self.assertEqual(summary["uploaded"], 1)
        self.assertEqual(summary["by_harness"], {"cursor": 1})
        payload = post.call_args.args[1]
        self.assertEqual(payload["harness"], "cursor")
        self.assertEqual(payload["thread_id"], thread)
        self.assertEqual(payload["model"], "claude-opus-5-5")

    def test_model_name_ignores_a_missing_or_broken_record(self):
        self.assertIsNone(cursor.model_name(self.home, "missing"))
        self.write_composer("bad", model_name="not-json")
        connection = sqlite3.connect(cursor.state_db(self.home))
        connection.execute("UPDATE cursorDiskKV SET value = ? WHERE key = ?", ("{not json", "composerData:bad"))
        connection.commit()
        connection.close()
        self.assertIsNone(cursor.model_name(self.home, "bad"))

    def write_composer(self, thread_id: str, model_name: str, model_id: str | None = None) -> None:
        path = self.home / "Library/Application Support/Cursor/User/globalStorage/state.vscdb"
        path.parent.mkdir(parents=True, exist_ok=True)
        connection = sqlite3.connect(path)
        connection.execute("CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value BLOB)")
        model_config = {"modelName": model_name}
        if model_id is not None:
            model_config["selectedModels"] = [{"modelId": model_id}]
        connection.execute(
            "INSERT OR REPLACE INTO cursorDiskKV (key, value) VALUES (?, ?)",
            (f"composerData:{thread_id}", json.dumps({"modelConfig": model_config})),
        )
        connection.commit()
        connection.close()


class CodexInstallTests(InstallTestCase):
    def test_installs_hooks_role_skills_and_writable_root(self):
        self.write(".codex/config.toml", 'model = "gpt-5.6"\n')
        self.install("codex")
        hooks = self.read(".codex/hooks.json")["hooks"]
        self.assertEqual(set(hooks), {h.event for h in bundles.CODEX_HOOKS})
        self.assertEqual(hooks["PostToolUse"][0]["matcher"], "apply_patch")
        role = tomllib.loads((self.home / ".codex/agents/cadri-memory-keeper.toml").read_text())
        self.assertEqual(role["name"], "cadri-memory-keeper")
        settings = tomllib.loads((self.home / ".codex/config.toml").read_text())
        self.assertEqual(settings["model"], "gpt-5.6")
        self.assertEqual(settings["sandbox_workspace_write"]["writable_roots"], [str(self.home / ".cadri/memory")])
        self.assertTrue((self.home / ".agents/skills/cadri-memory/SKILL.md").is_file())

        bundles.uninstall(self.home)
        self.assertEqual((self.home / ".codex/config.toml").read_text(), 'model = "gpt-5.6"\n')
        self.assertEqual(self.read(".codex/hooks.json"), {})
        self.assertFalse((self.home / ".codex/agents/cadri-memory-keeper.toml").exists())
        self.assertFalse((self.home / ".agents/skills/cadri-memory").exists())

    def test_an_existing_sandbox_table_is_left_for_the_user(self):
        original = '[sandbox_workspace_write]\nnetwork_access = true\n'
        self.write(".codex/config.toml", original)
        [line] = self.install("codex")
        self.assertIn("add ", line)
        self.assertIn("writable_roots", line)
        self.assertEqual((self.home / ".codex/config.toml").read_text(), original)

    def test_stop_hook_prints_json_and_uploads_the_rollout(self):
        self.install("codex")
        hooks = self.read(".codex/hooks.json")["hooks"]
        event = {"session_id": "019a-codex", "transcript_path": str(self.transcript), "hook_event_name": "Stop"}
        result = self.run_hook(hooks["Stop"][0]["hooks"][0]["command"], event)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), {})
        [upload] = self.uploads()
        self.assertEqual(upload["body"]["thread_id"], "019a-codex")
        self.assertEqual(upload["body"]["harness"], "codex")

    def test_backfill_ids_match_hook_session_ids(self):
        path = Path("rollout-2026-11-05T10-00-00-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b.jsonl")
        self.assertEqual(codex.thread_id(path), "0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b")


class OpenCodeInstallTests(InstallTestCase):
    def plugin(self) -> Path:
        return self.home / ".config/opencode/plugins/cadri.ts"

    def command(self) -> list[str]:
        line = self.plugin().read_text().split("const COMMAND: string[] = (", 1)[1].splitlines()[0]
        return json.loads(line.split(" as string[]", 1)[0])

    def test_stop_uploads_the_session_export(self):
        self.install("opencode")
        self.assertEqual(self.command(), [str(self.home / ".cadri/bin/cadri")])
        self.assertTrue((self.home / ".config/opencode/agents/cadri-memory-keeper.md").is_file())
        db = self.home / ".local/share/opencode/opencode.db"
        db.parent.mkdir(parents=True)
        connection = sqlite3.connect(db)
        with connection:
            # OpenCode 1.19's layout, including the session_message table it leaves empty.
            connection.execute("CREATE TABLE session (id TEXT, title TEXT, time_updated INTEGER)")
            connection.execute("CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT)")
            connection.execute("CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)")
            connection.execute("CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT)")
            connection.execute("INSERT INTO session VALUES ('ses_1', 'ping', 1)")
            connection.execute("INSERT INTO message VALUES ('m1', 'ses_1', 1, '{\"role\":\"user\"}')")
            connection.execute("INSERT INTO part VALUES ('p1', 'm1', 'ses_1', 1, '{\"type\":\"text\",\"text\":\"ping\"}')")
        connection.close()
        result = subprocess.run(
            [*self.command(), "hook", "stop", "--harness", "opencode"],
            input=json.dumps({"session_id": "ses_1"}),
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
        self.assertEqual(content["messages"], [{"id": "m1", "role": "user", "parts": [{"type": "text", "text": "ping"}]}])

    def test_exports_opencode_2_sessions(self):
        db = self.home / "opencode.db"
        connection = sqlite3.connect(db)
        with connection:
            connection.execute("CREATE TABLE session_v2 (id TEXT, title TEXT, time_updated INTEGER)")
            connection.execute("CREATE TABLE session_message (id TEXT, session_id TEXT, type TEXT, seq INTEGER, data TEXT)")
            connection.execute("INSERT INTO session_v2 VALUES ('ses_2', 'pong', 1)")
            connection.execute("INSERT INTO session_message VALUES ('b', 'ses_2', 'assistant', 2, '{\"text\":\"pong\"}')")
            connection.execute("INSERT INTO session_message VALUES ('a', 'ses_2', 'user', 1, '{\"text\":\"ping\"}')")
        connection.close()
        content = json.loads(opencode.export_session_json(db, "ses_2"))
        self.assertEqual(content["info"]["title"], "pong")
        self.assertEqual(content["messages"], [{"text": "ping"}, {"text": "pong"}])

    def test_lets_agents_use_memory_and_uninstall_restores_the_config(self):
        original = {"model": "opencode/x", "permission": {"external_directory": "ask", "edit": "ask"}}
        self.write(".config/opencode/opencode.json", original)
        self.install("opencode")
        permission = self.read(".config/opencode/opencode.json")["permission"]
        memory = f"{self.home / '.cadri/memory'}/*"
        reflections = f"{self.home / '.cadri/state/digests'}/*"
        self.assertEqual(permission["external_directory"], {"*": "ask", memory: "allow", reflections: "allow"})
        self.assertEqual(permission["bash"], {"cadri memory *": "allow"})
        self.assertEqual(permission["edit"], "ask")
        bundles.uninstall(self.home)
        self.assertEqual(self.read(".config/opencode/opencode.json"), {**original, "$schema": "https://opencode.ai/config.json"})

    def test_a_plain_jsonc_config_is_edited_in_place(self):
        self.write(".config/opencode/opencode.jsonc", '{\n  "$schema": "https://opencode.ai/config.json"\n}\n')
        self.install("opencode")
        self.assertFalse((self.home / ".config/opencode/opencode.json").exists())
        permission = self.read(".config/opencode/opencode.jsonc")["permission"]
        self.assertEqual(permission["bash"], {"cadri memory *": "allow"})
        bundles.uninstall(self.home)
        self.assertEqual(self.read(".config/opencode/opencode.jsonc"), {"$schema": "https://opencode.ai/config.json"})

    def test_a_jsonc_config_with_comments_gets_instructions_instead_of_edits(self):
        path = self.write(".config/opencode/opencode.jsonc", "{ // mine\n}\n")
        [line] = self.install("opencode")
        self.assertIn("opencode.jsonc", line)
        self.assertIn("cadri memory *", line)
        self.assertEqual(path.read_text(), "{ // mine\n}\n")

    def test_refuses_to_overwrite_a_plugin_it_did_not_write(self):
        path = self.write(".config/opencode/plugins/cadri.ts", "export const Mine = 1\n")
        [line] = self.install("opencode")
        self.assertIn("not installed", line)
        self.assertEqual(path.read_text(encoding="utf-8"), "export const Mine = 1\n")

    def built_plugin(self) -> tuple[Path, Path]:
        """The OpenCode plugin wired to a fake `cadri` that logs each call; returns (plugin, log)."""
        log = self.home / "calls.jsonl"
        fake = self.home / "fake-cadri"
        fake.write_text(
            f"#!{sys.executable}\n"
            "import json, sys\n"
            "event = sys.argv[sys.argv.index('hook') + 1]\n"
            "payload = json.loads(sys.stdin.read() or '{}')\n"
            f"open({str(log)!r}, 'a').write(json.dumps({{'event': event, 'payload': payload}}) + '\\n')\n"
            "if event in ('session-start', 'prompt'):\n"
            "    print(json.dumps({'context': 'CTX-' + event}))\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        plugin = self.home / "cadri.ts"
        plugin.write_text(bundles.opencode_plugin(str(fake), str(self.home / "skills")))
        return plugin, log

    def calls(self, log: Path, count: int) -> list[dict]:
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and (not log.exists() or len(log.read_text().splitlines()) < count):
            time.sleep(0.1)
        return [json.loads(line) for line in log.read_text().splitlines()]

    @unittest.skipUnless(NODE, "needs node 23 or later")
    def test_the_v1_plugin_drives_every_hook(self):
        plugin, log = self.built_plugin()
        driver = self.home / "driver.mjs"
        driver.write_text(
            "const { default: plugin } = await import(process.argv[2])\n"
            "const client = { session: {\n"
            "  messages: async () => ({ data: [{ info: { role: 'assistant' }, parts: [{ type: 'text', text: 'done' }] }] }),\n"
            "  get: async ({ path }) => ({ data: { id: path.id, parentID: path.id === 'k1' ? 's1' : undefined } }),\n"
            "} }\n"
            "const hooks = await plugin.server({ directory: '/work', client })\n"
            "await hooks['chat.message']({ sessionID: 's1', agent: 'build' }, { parts: [{ type: 'text', text: 'hello' }] })\n"
            "await hooks['chat.message']({ sessionID: 'k1', agent: 'cadri-memory-keeper' }, { parts: [{ type: 'text', text: 'reflect' }] })\n"
            "const keeperShell = { env: {} }\n"
            "await hooks['shell.env']({ cwd: '/work', sessionID: 'k1' }, keeperShell)\n"
            "console.log(JSON.stringify(keeperShell.env))\n"
            "const first = { system: [] }, second = { system: [] }\n"
            "await hooks['experimental.chat.system.transform']({ sessionID: 's1' }, first)\n"
            "await hooks['experimental.chat.system.transform']({ sessionID: 's1' }, second)\n"
            "await hooks['tool.execute.after']({ tool: 'edit', sessionID: 's1', args: { filePath: '/work/a.md' } }, {})\n"
            "await hooks.event({ event: { type: 'session.idle', properties: { sessionID: 's1' } } })\n"
            "const shell = { env: {} }\n"
            "await hooks['shell.env']({ cwd: '/work', sessionID: 's1' }, shell)\n"
            "console.log(JSON.stringify(shell.env))\n"
            "console.log(JSON.stringify([first.system, second.system]))\n"
        )
        result = subprocess.run([NODE, str(driver), str(plugin)], capture_output=True, text=True, timeout=60, env=self.env())
        self.assertEqual(result.returncode, 0, result.stderr)
        both = ["CTX-session-start", "CTX-prompt"]
        lines = result.stdout.strip().splitlines()
        self.assertEqual(json.loads(lines[-1]), [both, both])
        self.assertEqual(json.loads(lines[-2]), {"CADRI_SESSION_ID": "s1", "CADRI_HARNESS": "opencode"})
        keeper_env = {"CADRI_SESSION_ID": "k1", "CADRI_HARNESS": "opencode", "CADRI_AGENT": "cadri-memory-keeper"}
        self.assertEqual(json.loads(lines[-3]), keeper_env)
        calls = self.calls(log, 6)
        main = [call for call in calls if call["payload"].get("session_id") == "s1"]
        self.assertEqual([call["event"] for call in main[:2]], ["session-start", "prompt"])
        self.assertEqual(main[1]["payload"]["prompt"], "hello")
        self.assertEqual(sorted(call["event"] for call in main), ["edit", "prompt", "session-start", "stop"])
        stop = next(call for call in main if call["event"] == "stop")
        self.assertEqual(stop["payload"]["last_message"], "done")
        self.assertEqual({call["payload"].get("agent") for call in main}, {None})
        keeper = [call["payload"].get("agent") for call in calls if call["payload"].get("session_id") == "k1"]
        self.assertEqual(keeper, ["cadri-memory-keeper", "cadri-memory-keeper"])

    @unittest.skipUnless(NODE, "needs node 23 or later")
    def test_the_v2_plugin_drives_every_hook(self):
        node = NODE
        log = self.home / "calls.jsonl"
        fake = self.home / "fake-cadri"
        fake.write_text(
            f"#!{sys.executable}\n"
            "import json, sys\n"
            "event = sys.argv[sys.argv.index('hook') + 1]\n"
            "payload = json.loads(sys.stdin.read() or '{}')\n"
            f"open({str(log)!r}, 'a').write(json.dumps({{'event': event, 'payload': payload}}) + '\\n')\n"
            "if event in ('session-start', 'prompt'):\n"
            "    print(json.dumps({'context': 'CTX-' + event}))\n",
            encoding="utf-8",
        )
        fake.chmod(0o755)
        skills = self.home / "skills"
        (skills / "cadri-memory").mkdir(parents=True)
        (skills / "cadri-memory/SKILL.md").write_text("---\nname: cadri-memory\ndescription: Use memory\n---\nBody\n")
        plugin = self.home / "cadri.ts"
        plugin.write_text(bundles.opencode_plugin(str(fake), str(skills)))
        driver = self.home / "driver.mjs"
        driver.write_text(
            "const { default: plugin } = await import(process.argv[2])\n"
            "const hooks = {}, puts = [], skills = []\n"
            "const handle = { dispose: async () => {} }\n"
            "const ctx = {\n"
            "  location: { directory: '/work' },\n"
            "  skill: { transform: async (fn) => { fn({ get: () => undefined, add: (s) => skills.push(s.id) }); return handle } },\n"
            "  session: {\n"
            "    hook: async (name, fn) => { hooks[name] = fn; return handle },\n"
            "    instructions: { entry: { put: async (e) => puts.push(e), remove: async () => {} } },\n"
            "    context: async () => [{ role: 'assistant', parts: [{ type: 'text', text: 'done' }] }],\n"
            "  },\n"
            "  tool: { hook: async (name, fn) => { hooks['tool.' + name] = fn; return handle } },\n"
            "  event: { subscribe: async function* () { yield { type: 'session.execution.succeeded', data: { sessionID: 's1' } } } },\n"
            "}\n"
            "const dispose = await plugin.setup(ctx)\n"
            "const first = { sessionID: 's1', agent: 'build', system: [] }\n"
            "const second = { sessionID: 's1', agent: 'build', system: [] }\n"
            "hooks.context(first); hooks.context(second)\n"
            "await hooks.prompt({ sessionID: 's1', prompt: { text: 'hello' } })\n"
            "hooks['tool.execute.after']({ tool: 'edit', input: { filePath: '/work/a.md' }, sessionID: 's1' })\n"
            "await new Promise((r) => setTimeout(r, 50))\n"
            "await dispose()\n"
            "console.log(JSON.stringify({ skills, system: [first.system, second.system], puts }))\n"
        )
        result = subprocess.run([node, str(driver), str(plugin)], capture_output=True, text=True, timeout=60, env=self.env())
        self.assertEqual(result.returncode, 0, result.stderr)
        out = json.loads(result.stdout.strip().splitlines()[-1])
        self.assertEqual(out["skills"], ["cadri-memory"])
        seed = [{"type": "text", "text": "CTX-session-start"}]
        self.assertEqual(out["system"], [seed, seed])
        self.assertEqual(out["puts"], [{"sessionID": "s1", "key": "cadri", "value": "CTX-prompt"}])
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline and (not log.exists() or len(log.read_text().splitlines()) < 4):
            time.sleep(0.1)
        calls = [json.loads(line) for line in log.read_text().splitlines()]
        events = sorted(call["event"] for call in calls)
        self.assertEqual(events, ["edit", "prompt", "session-start", "stop"])
        stop = next(call for call in calls if call["event"] == "stop")
        self.assertEqual(stop["payload"]["last_message"], "done")
        edit = next(call for call in calls if call["event"] == "edit")
        self.assertEqual(edit["payload"]["paths"], ["/work/a.md"])


class UpgradeTests(InstallTestCase):
    def test_install_replaces_previous_hook_entries_and_skill_copies(self):
        self.write(".claude/settings.json", {"hooks": {"SessionEnd": [{"hooks": [{"type": "command", "command": LEGACY_COMMAND.format(harness="claude_code")}]}]}})
        self.write(".codex/hooks.json", {"hooks": {"Stop": [{"hooks": [{"type": "command", "command": LEGACY_COMMAND.format(harness="codex")}]}]}})
        self.write(".config/opencode/plugins/cadri.ts", "// cadri-managed-opencode-plugin\nexport const Old = 1\n")
        self.write(".agents/skills/cadri-memory/SKILL.md", "stale")
        self.install("claude_code", "codex", "cursor", "opencode")
        claude = self.read(".claude/settings.json")["hooks"]
        self.assertNotIn("SessionEnd", claude)
        codex_hooks = self.read(".codex/hooks.json")["hooks"]
        self.assertEqual(len(codex_hooks["Stop"]), 1)
        self.assertNotIn("cadri_cli.py", json.dumps(codex_hooks))
        self.assertNotIn("Old", self.plugin_text())
        skills = sorted(p.name for p in (self.home / ".agents/skills").iterdir())
        self.assertEqual(skills, sorted(bundles.SHIPPED_SKILLS))
        self.assertNotEqual((self.home / ".agents/skills/cadri-memory/SKILL.md").read_text(), "stale")

    def plugin_text(self) -> str:
        return (self.home / ".config/opencode/plugins/cadri.ts").read_text()


class SeedWindowTests(unittest.TestCase):
    def test_the_smaller_set_is_the_day_window_when_it_is_shorter(self):
        now = 1_700_000_000.0
        day = 24 * 60 * 60
        recent = [self.ref(f"r{i}", now - day) for i in range(40)]
        older = [self.ref(f"o{i}", now - 90 * day) for i in range(80)]
        chosen, in_window, newest = cli.select_seed_sessions(older + recent, now, 60, 100)
        self.assertEqual((in_window, newest, len(chosen)), (40, 100, 40))
        self.assertTrue(all(item.thread_id.startswith("r") for item in chosen))

    def test_the_smaller_set_is_the_recency_cap_when_the_window_is_larger(self):
        now = 1_700_000_000.0
        many = [self.ref(f"n{i}", now - i * 3600) for i in range(150)]
        chosen, in_window, newest = cli.select_seed_sessions(list(reversed(many)), now, 60, 100)
        self.assertEqual((in_window, newest, len(chosen)), (150, 100, 100))
        self.assertEqual({item.thread_id for item in chosen}, {f"n{i}" for i in range(100)})

    def test_an_override_uses_the_numbers_it_is_given(self):
        now = 1_700_000_000.0
        day = 24 * 60 * 60
        refs = [self.ref(f"d{i}", now - i * day) for i in range(30)]
        chosen, in_window, newest = cli.select_seed_sessions(refs, now, 10, 10)
        self.assertEqual((in_window, newest, len(chosen)), (11, 10, 10))
        self.assertEqual(cli.resolve_discover_bounds(None, None, True), (60, 100))
        self.assertEqual(cli.resolve_discover_bounds(10, 10, True), (10, 10))
        self.assertEqual(cli.resolve_discover_bounds(None, None, False), (30.0, 50))

    def test_init_names_the_join_skill_when_the_team_book_has_lessons(self):
        self.assertIn("cadri-memory-join", cli.next_memory_step(True))
        self.assertIn("shared with your teammates", cli.next_memory_step(True))
        self.assertIn("cadri-memory-seed", cli.next_memory_step(False))
        self.assertNotIn("cadri-memory-join", cli.next_memory_step(False))
        self.assertIn("team book", cli.memory_time_warning(True))
        self.assertIn("Seeding usually takes", cli.memory_time_warning(False))

    def ref(self, thread_id: str, updated_at: float):
        from cadri.discover.types import SessionRef

        return SessionRef("cursor", thread_id, Path(thread_id), updated_at)


class DetectionTests(InstallTestCase):
    def test_config_dirs_count_without_sessions(self):
        (self.home / ".cursor").mkdir()
        (self.home / ".codex").mkdir()
        self.assertEqual(config.installed_harnesses(self.home)[-2:], ["cursor", "codex"])


if __name__ == "__main__":
    unittest.main()
