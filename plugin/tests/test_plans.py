"""Plans extracted from each harness's session blob."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

from support import PLUGIN

sys.path.insert(0, str(PLUGIN))

from cadri import plans  # noqa: E402


PLAN = "# Checkout\n\nCharge only after the cart locks, so a price change cannot land mid-payment.\n"


class PlanTests(unittest.TestCase):
    def test_cursor_keeps_the_revised_plan(self):
        first = {"name": "Checkout", "overview": "Draft.", "plan": PLAN + "Draft only.\n"}
        second = {"name": "Checkout", "overview": "Revised.", "plan": PLAN}
        blob = "\n".join(
            json.dumps(
                {
                    "role": "assistant",
                    "message": {"content": [{"type": "tool_use", "name": "CreatePlan", "input": item}]},
                }
            )
            for item in (first, second)
        )
        [plan] = plans.extract(blob)
        self.assertEqual(plan["title"], "Checkout")
        self.assertEqual(plan["overview"], "Revised.")
        self.assertNotIn("Draft only", plan["body"])
        self.assertEqual(plan["source"], "CreatePlan")

    def test_claude_plan_file_and_codex_patch_and_opencode_tool(self):
        claude = json.dumps(
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Write",
                            "input": {
                                "file_path": "/Users/me/.claude/plans/checkout.md",
                                "content": "---\nname: Checkout\noverview: Charge after the cart locks.\n---\n" + PLAN,
                            },
                        },
                        {"type": "tool_use", "name": "ExitPlanMode", "input": {}},
                    ]
                },
            }
        )
        [plan] = plans.extract(claude)
        self.assertEqual(plan["title"], "Checkout")
        self.assertEqual(plan["overview"], "Charge after the cart locks.")
        self.assertNotIn("name: Checkout", plan["body"])
        self.assertEqual(plan["source"], "Write")

        patch = "*** Begin Patch\n*** Add File: docs/checkout.plan.md\n+# Checkout\n+\n+Charge only after the cart locks, so a price change cannot land mid-payment.\n*** End Patch\n"
        codex = "\n".join(
            [
                json.dumps(
                    {
                        "type": "response_item",
                        "payload": {
                            "type": "function_call",
                            "name": "update_plan",
                            "arguments": json.dumps({"plan": [{"step": "read tests", "status": "pending"}]}),
                        },
                    }
                ),
                json.dumps(
                    {
                        "type": "response_item",
                        "payload": {"type": "custom_tool_call", "name": "apply_patch", "input": patch},
                    }
                ),
            ]
        )
        [plan] = plans.extract(codex)
        self.assertIn("cart locks", plan["body"])
        self.assertEqual(plan["source"], "apply_patch")

        opencode = json.dumps(
            {
                "info": {"id": "ses_1"},
                "messages": [
                    {
                        "info": {"role": "assistant"},
                        "parts": [
                            {
                                "type": "tool",
                                "tool": "submit_plan",
                                "state": {"input": {"name": "Rename", "plan": "# Rename\n\nThe button says Save, because the old label described the implementation and confused people.\n"}},
                            }
                        ],
                    }
                ],
            }
        )
        [plan] = plans.extract(opencode)
        self.assertEqual(plan["title"], "Rename")
        self.assertEqual(plan["source"], "submit_plan")

    def test_an_ordinary_write_is_not_a_plan(self):
        blob = json.dumps(
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {"type": "tool_use", "name": "Write", "input": {"file_path": "/work/src/app.py", "content": PLAN}}
                    ]
                },
            }
        )
        self.assertEqual(plans.extract(blob), [])

    def test_opencode_plan_files_live_in_the_singular_plan_directory(self):
        body = "# Rename\n\nThe button says Save, because the old label described the implementation and confused people.\n"
        hit = json.dumps(
            {
                "type": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Write",
                            "input": {"file_path": "/Users/me/.opencode/plan/rename.md", "content": body},
                        }
                    ]
                },
            }
        )
        miss = hit.replace("/.opencode/plan/", "/.opencode/plans/")
        [plan] = plans.extract(hit)
        self.assertEqual(plan["title"], "Rename")
        self.assertEqual(plans.extract(miss), [])

    def test_the_plan_file_replaces_the_draft(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            folder = home / ".cursor" / "plans"
            folder.mkdir(parents=True)
            (folder / "checkout_abc.plan.md").write_text(
                "---\nname: Checkout\noverview: The accepted version.\n---\n"
                + PLAN
                + "The person added this line before accepting.\n",
                encoding="utf-8",
            )
            draft = plans.extract(
                json.dumps(
                    {
                        "role": "assistant",
                        "message": {
                            "content": [
                                {"type": "tool_use", "name": "CreatePlan", "input": {"name": "Checkout", "plan": PLAN}}
                            ]
                        },
                    }
                )
            )
            [plan] = plans.overlay(draft, home)
        self.assertEqual(plan["source"], "file")
        self.assertEqual(plan["overview"], "The accepted version.")
        self.assertIn("before accepting", plan["body"])

    def test_a_seen_plan_is_not_returned_again(self):
        found = [{"title": "Checkout", "body": PLAN, "source": "CreatePlan"}]
        fresh, seen = plans.select_new(found, [])
        self.assertEqual(len(fresh), 1)
        again, seen_again = plans.select_new(found, seen)
        self.assertEqual(again, [])
        self.assertEqual(seen_again, seen)
