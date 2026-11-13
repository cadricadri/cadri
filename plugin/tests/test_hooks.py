"""Each harness's hooks end to end: what the agent sees, what Cadri records, and crash safety."""

from __future__ import annotations

import io
import json
import os
import time
import unittest
from pathlib import Path
from contextlib import redirect_stdout
from unittest import mock

from support import BooksTestCase

from cadri import auth, capture
from cadri.hooks import adapters, core, run
from cadri.hooks.adapters.codex import patch_paths
from cadri.memory import digests, edits, sync
from cadri.state import Session


class HookTestCase(BooksTestCase):
    seed_team = {
        "MEMORY.md": "# Team memory\n\nDeploys happen on Tuesdays.\n",
        "components/MEMORY.md": "# Components\n\n- [billing](billing.md) — rate limits and retries\n",
        "components/billing.md": "---\ndescription: Billing service rate limits and retries\n---\n# Billing\n\nRetries back off.\n",
    }

    def setUp(self):
        super().setUp()
        os.environ["CADRI_HOOK_FOREGROUND"] = "1"
        self.uploads = []
        patchers = [
            mock.patch.object(sync, "server_has_new", return_value=False),
            mock.patch.object(capture, "upload", side_effect=self.uploads.append),
        ]
        for patcher in patchers:
            patcher.start()
            self.addCleanup(patcher.stop)

    def hook(self, harness, event, payload):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(run.run(event, harness, json.dumps(payload)), 0)
        return out.getvalue()

    def json_hook(self, harness, event, payload):
        text = self.hook(harness, event, payload)
        return json.loads(text) if text.strip() else {}


class ClaudeTests(HookTestCase):
    def setUp(self):
        super().setUp()
        os.environ["CLAUDE_PLUGIN_ROOT"] = str(self.tmp / "plugin")

    def test_without_the_plugin_names_are_not_namespaced(self):
        del os.environ["CLAUDE_PLUGIN_ROOT"]
        context = self.start()["hookSpecificOutput"]["additionalContext"]
        self.assertIn("`cadri-memory-keeper`", context)
        self.assertNotIn("cadri:", context)

    def start(self, **extra):
        return self.json_hook("claude", "session-start", {"session_id": "c1", "source": "startup", "cwd": str(self.tmp), **extra})

    def test_session_start_loads_both_books_and_the_instructions(self):
        env_file = self.tmp / "claude.env"
        with mock.patch.dict(os.environ, {"CLAUDE_ENV_FILE": str(env_file)}):
            data = self.start()
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertEqual(data["hookSpecificOutput"]["hookEventName"], "SessionStart")
        self.assertIn("Deploys happen on Tuesdays", context)
        self.assertIn("Prefers short answers", context)
        self.assertIn("components/billing.md — Billing service rate limits and retries", context)
        self.assertIn("`cadri:cadri-memory-keeper`", context)
        self.assertIn("Session id: `c1`", context)
        self.assertNotIn("{{", context)
        self.assertLessEqual(len(context), 9500)
        self.assertEqual(env_file.read_text(), "export CADRI_SESSION_ID=c1\nexport CADRI_HARNESS=claude\n")

    def test_expired_sign_in_shows_a_link_and_polls_in_the_background(self):
        entry = {"device_code": "d", "user_code": "ABCD", "url": "https://cadri.example/activate", "expires_at": time.time() + 600}
        with mock.patch.object(auth, "check", return_value=auth.EXPIRED), mock.patch.object(
            auth, "begin_background_login", return_value=entry
        ), mock.patch.object(auth, "finish_background_login", return_value=True) as finish:
            data = self.start()
        self.assertIn("https://cadri.example/activate", data["systemMessage"])
        self.assertIn("ABCD", data["systemMessage"])
        self.assertIn("local copy", data["hookSpecificOutput"]["additionalContext"])
        finish.assert_called_once()

    def test_a_server_without_sign_in_counts_as_signed_in(self):
        with mock.patch.object(auth, "check", return_value=auth.MISSING), mock.patch.object(
            auth, "begin_background_login", side_effect=auth.SignInUnsupported("no sign-in")
        ):
            data = self.start()
        self.assertNotIn("systemMessage", data)
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertNotIn("not signed in", context)
        self.assertIn("Deploys happen on Tuesdays", context)

    def test_prompt_lists_relevant_memory_and_the_search_reminder(self):
        self.start()
        data = self.json_hook("claude", "prompt", {"session_id": "c1", "prompt": "Why do billing retries hit the rate limits?"})
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertIn(str(self.team.path / "components/billing.md"), context)
        self.assertIn("batch the reads", context)

    def test_unrelated_prompt_adds_nothing(self):
        self.start()
        self.assertEqual(self.hook("claude", "prompt", {"session_id": "c1", "prompt": "hello"}), "")

    def test_a_turn_becomes_a_digest_and_a_correction_asks_for_the_keeper(self):
        self.start()
        self.hook("claude", "prompt", {"session_id": "c1", "prompt": "When do we deploy?"})
        memory_file = self.team.path / "MEMORY.md"
        self.hook("claude", "edit", {"session_id": "c1", "tool_name": "Edit", "tool_input": {"file_path": str(memory_file)}})
        self.hook("claude", "edit", {"session_id": "c1", "tool_name": "Write", "tool_input": {"file_path": str(self.tmp / "app.py")}})
        self.assertEqual(edits.latest(self.team)["MEMORY.md"].session, "c1")
        self.hook("claude", "stop", {"session_id": "c1", "last_assistant_message": "On Tuesdays.", "transcript_path": "/nonexistent"})
        [(_, digest)] = digests.pending()
        self.assertEqual(digest["prompt"], "When do we deploy?")
        self.assertEqual(digest["reply"], "On Tuesdays.")
        self.assertEqual(digest["memory_edits"], ["team:MEMORY.md"])
        self.assertEqual(digest["edit_count"], 2)
        [upload] = self.uploads
        self.assertEqual((upload["thread_id"], upload["content"]), ("c1", "On Tuesdays."))

        data = self.json_hook("claude", "prompt", {"session_id": "c1", "prompt": "No, that's wrong: we deploy on Thursdays now."})
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertIn("launch the `cadri:cadri-memory-keeper` subagent in the background", context)
        claim_file = context.split("Reflect on ", 1)[1].split(" for session", 1)[0]
        text = Path(claim_file).read_text(encoding="utf-8")
        self.assertIn("we deploy on Thursdays now", text)
        self.assertEqual(digests.pending(), [])

    def test_any_next_message_wakes_the_keeper_not_only_corrections(self):
        # The keeper (maintain.md), not keyword rules, decides what to keep.
        self.start()
        self.hook("claude", "prompt", {"session_id": "c1", "prompt": "How do retries work?"})
        self.hook("claude", "stop", {"session_id": "c1", "last_assistant_message": "They back off."})
        data = self.json_hook("claude", "prompt", {"session_id": "c1", "prompt": "Thanks — and what about timeouts?"})
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertIn("launch the `cadri:cadri-memory-keeper` subagent", context)
        self.assertIn("ready for the keeper to review", context)

    def test_remember_language_is_only_a_main_agent_hint(self):
        self.assertEqual(digests.signals('"Remember this: we use vitest."'), {"remember"})
        self.assertEqual(digests.signals("Wrong again: Orchard uses vitest."), set())

    def test_stop_twice_in_one_turn_saves_one_digest(self):
        self.start()
        self.hook("claude", "prompt", {"session_id": "c1", "prompt": "Explain the billing service"})
        for _ in range(2):
            self.hook("claude", "stop", {"session_id": "c1", "last_assistant_message": "It bills."})
        self.assertEqual(len(digests.pending()), 1)

    def test_a_subagent_never_triggers_reflection(self):
        self.start()
        self.hook("claude", "prompt", {"session_id": "c1", "prompt": "x"})
        self.hook("claude", "stop", {"session_id": "c1", "agent_type": "cadri:cadri-memory-keeper", "last_assistant_message": "Saved."})
        self.assertEqual(digests.pending(), [])

    def test_explicit_remember_asks_the_main_agent_to_write_it(self):
        self.start()
        data = self.json_hook("claude", "prompt", {"session_id": "c1", "prompt": "Remember that I like tabs."})
        self.assertIn("cadri memory push --human", data["hookSpecificOutput"]["additionalContext"])

    def test_cursor_running_claude_hooks_gets_nothing(self):
        self.assertEqual(self.hook("claude", "session-start", {"session_id": "x", "cursor_version": "2.0"}), "")
        self.assertFalse(Session("claude", "x").path.exists())

    def test_legacy_command_names_the_harness_only(self):
        out = io.StringIO()
        with redirect_stdout(out):
            run.run(None, "claude", json.dumps({"session_id": "c1", "hook_event_name": "Stop", "transcript_path": "/x"}))
        self.assertEqual(out.getvalue(), "")


class CodexTests(HookTestCase):
    def test_session_start_and_prompt_use_additional_context(self):
        data = self.json_hook("codex", "session-start", {"session_id": "x1", "source": "startup"})
        context = data["hookSpecificOutput"]["additionalContext"]
        self.assertIn("`cadri-memory-keeper`", context)
        self.assertNotIn("cadri:cadri-memory-keeper", context)

    def test_stop_always_prints_json(self):
        self.assertEqual(self.hook("codex", "stop", {"session_id": "x1", "last_assistant_message": "done"}).strip(), "{}")

    def test_apply_patch_edits_are_recorded(self):
        patch = (
            "*** Begin Patch\n*** Update File: components/billing.md\n@@\n-a\n+b\n"
            "*** Add File: notes/new.md\n+x\n*** End Patch\n"
        )
        self.assertEqual(patch_paths(patch), ["components/billing.md", "notes/new.md"])
        self.hook("codex", "edit", {"session_id": "x1", "cwd": str(self.team.path), "tool_name": "apply_patch", "tool_input": {"command": patch}})
        self.assertEqual(sorted(edits.latest(self.team)), ["components/billing.md", "notes/new.md"])


class CursorTests(HookTestCase):
    base = {"conversation_id": "k1", "generation_id": "g1", "cursor_version": "2.1"}

    def test_session_start_returns_context_and_env(self):
        data = self.json_hook("cursor", "session-start", self.base)
        self.assertIn("Deploys happen on Tuesdays", data["additional_context"])
        self.assertEqual(data["env"], {"CADRI_SESSION_ID": "k1", "CADRI_HARNESS": "cursor"})

    def test_prompt_context_arrives_with_the_first_tool_call(self):
        self.json_hook("cursor", "session-start", self.base)
        prompt = self.json_hook("cursor", "prompt", {**self.base, "prompt": "billing retries and rate limits"})
        self.assertEqual(prompt, {"continue": True})
        first = self.json_hook("cursor", "tool-after", {**self.base, "tool_name": "Read"})
        self.assertIn("Deploys happen on Tuesdays", first["additional_context"])
        self.assertIn("components/billing.md", first["additional_context"])
        self.assertEqual(self.json_hook("cursor", "tool-after", {**self.base, "tool_name": "Read"}), {})

    def test_the_reply_from_after_agent_response_reaches_the_digest(self):
        self.json_hook("cursor", "prompt", {**self.base, "prompt": "explain billing"})
        self.json_hook("cursor", "response", {**self.base, "text": "Billing retries back off."})
        self.assertEqual(self.json_hook("cursor", "stop", {**self.base, "status": "completed"}), {})
        [(_, digest)] = digests.pending()
        self.assertEqual(digest["reply"], "Billing retries back off.")
        self.assertEqual(digest["harness"], "cursor")


class OpenCodeTests(HookTestCase):
    def test_the_next_prompt_asks_the_main_agent_for_the_keeper(self):
        self.json_hook("opencode", "prompt", {"session_id": "o1", "prompt": "how do retries work"})
        self.json_hook("opencode", "stop", {"session_id": "o1", "last_message": "They back off."})
        data = self.json_hook("opencode", "prompt", {"session_id": "o1", "prompt": "Thanks, that helps."})
        self.assertIn("launch the `cadri-memory-keeper` subagent", data["context"])
        self.assertIn("Reflect on ", data["context"])


class CrashSafetyTests(HookTestCase):
    def test_every_harness_gets_its_empty_answer_when_cadri_fails(self):
        expected = {
            ("claude", "session-start"): "",
            ("claude", "stop"): "",
            ("codex", "stop"): "{}",
            ("codex", "prompt"): "",
            ("cursor", "prompt"): '{"continue": true}',
            ("cursor", "stop"): "{}",
            ("opencode", "prompt"): "{}",
        }
        with mock.patch.object(core, "handle", side_effect=RuntimeError("boom")):
            for (harness, event), answer in expected.items():
                with self.subTest(harness=harness, event=event):
                    self.assertEqual(self.hook(harness, event, {"session_id": "s"}).strip(), answer)
        log = (self.tmp / "home" / "hooks.log").read_text()
        self.assertIn("RuntimeError: boom", log)

    def test_invalid_json_and_unknown_harness_are_harmless(self):
        out = io.StringIO()
        with redirect_stdout(out):
            self.assertEqual(run.run("stop", "codex", "not json"), 0)
            self.assertEqual(run.run("stop", "nope", "{}"), 0)
        self.assertEqual(out.getvalue().strip(), "{}")

    def test_a_failing_background_job_is_logged_not_raised(self):
        with mock.patch.object(capture, "capture", side_effect=OSError("disk full")):
            self.hook("claude", "pre-compact", {"session_id": "s", "transcript_path": "/x"})
        self.assertIn("disk full", (self.tmp / "home" / "hooks.log").read_text())

    def test_no_books_yet(self):
        from cadri.memory import books as bk

        bk.identity_path("http://127.0.0.1:8080").unlink()
        with mock.patch.object(bk, "identity", side_effect=bk.MemoryFailure("offline")):
            data = self.json_hook("claude", "session-start", {"session_id": "n1"})
        self.assertIn("not set up", data["hookSpecificOutput"]["additionalContext"])
        self.assertEqual(self.hook("claude", "prompt", {"session_id": "n1", "prompt": "billing"}), "")


class AdapterTests(unittest.TestCase):
    def test_every_harness_has_an_adapter(self):
        self.assertEqual(sorted(adapters.ADAPTERS), ["claude", "codex", "cursor", "opencode"])
        self.assertIs(adapters.get("claude_code"), adapters.get("claude"))


if __name__ == "__main__":
    unittest.main()
