"""Canvas capture: the file is the document, the transcript says which session owns it."""

from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

from support import PLUGIN

sys.path.insert(0, str(PLUGIN))

from cadri import canvases  # noqa: E402

SAMPLE = """
<Stack>
  <H1>Hosted Postgres</H1>
  <Text>Keep the page in the database.</Text>
  <Callout tone="info" title="Decision">RDS in the VPC.</Callout>
  <Stat value="2.1 MB" label="Stored view" />
  <H2>Team size</H2>
  <Table
    headers={["Engineers", "Stored"]}
    rows={[
      ["25", "322 MB"],
      ["1000", "13 GB"],
    ]}
  />
  <BarChart
    categories={["5", "25"]}
    series={[{ name: "Stored (MB)", data: [64, 322] }]}
  />
</Stack>
"""

DRAFT = "<H1>Draft</H1><Text>First version of the note.</Text>"
PATH = "/Users/me/.cursor/projects/app/canvases/hosted-postgres.canvas.tsx"


def write_line(contents: str) -> str:
    return json.dumps(
        {
            "role": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "name": "Write",
                        "input": {"path": PATH, "contents": contents},
                    }
                ]
            },
        }
    )


class CanvasTests(unittest.TestCase):
    def test_a_canvas_becomes_markdown(self):
        projected = canvases.project(SAMPLE)
        self.assertIsNotNone(projected)
        assert projected is not None
        self.assertEqual(projected["title"], "Hosted Postgres")
        self.assertTrue(projected["body"].startswith("# Hosted Postgres"))
        self.assertIn("Keep the page in the database.", projected["body"])
        self.assertIn("> **Decision**", projected["body"])
        self.assertIn("> RDS in the VPC.", projected["body"])
        self.assertIn("**2.1 MB** — Stored view", projected["body"])
        self.assertIn("## Team size", projected["body"])
        self.assertIn("| Engineers | Stored |", projected["body"])
        self.assertIn("| 25 | 322 MB |", projected["body"])
        self.assertIn("| 1000 | 13 GB |", projected["body"])
        self.assertIn("| Stored (MB) | 64 | 322 |", projected["body"])

    def test_a_later_write_replaces_the_draft(self):
        blob = write_line(DRAFT) + "\n" + write_line(SAMPLE)
        [canvas] = canvases.extract(blob)
        self.assertEqual(canvas["slug"], "hosted-postgres")
        self.assertEqual(canvas["title"], "Hosted Postgres")
        self.assertNotIn("First version", canvas["body"])

    def test_an_ordinary_write_is_not_a_canvas(self):
        blob = json.dumps(
            {
                "role": "assistant",
                "message": {
                    "content": [
                        {
                            "type": "tool_use",
                            "name": "Write",
                            "input": {"path": "/Users/me/cadri/engine/src/main.rs", "contents": SAMPLE},
                        }
                    ]
                },
            }
        )
        self.assertEqual(canvases.extract(blob), [])

    def test_the_file_replaces_the_draft_and_the_upload_carries_it(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            folder = home / ".cursor" / "projects" / "app" / "canvases"
            folder.mkdir(parents=True)
            (folder / "hosted-postgres.canvas.tsx").write_text(SAMPLE, encoding="utf-8")
            draft = canvases.extract(write_line(DRAFT))
            [canvas] = canvases.overlay(draft, home)
            self.assertEqual(canvas["source"], "file")
            self.assertEqual(canvas["title"], "Hosted Postgres")
            uploaded = canvases.refresh(write_line(DRAFT) + "\n", home)
            self.assertIn("Hosted Postgres", uploaded)
            self.assertNotIn("First version", uploaded)

    def test_a_strreplace_still_picks_up_the_file(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory)
            folder = home / ".cursor" / "projects" / "app" / "canvases"
            folder.mkdir(parents=True)
            (folder / "hosted-postgres.canvas.tsx").write_text(SAMPLE, encoding="utf-8")
            blob = json.dumps(
                {
                    "role": "assistant",
                    "message": {
                        "content": [
                            {
                                "type": "tool_use",
                                "name": "StrReplace",
                                "input": {
                                    "path": PATH,
                                    "old_string": "Draft",
                                    "new_string": "Hosted Postgres",
                                },
                            }
                        ]
                    },
                }
            )
            [canvas] = canvases.overlay(canvases.extract(blob), home)
            self.assertEqual(canvas["title"], "Hosted Postgres")
            uploaded = canvases.refresh(blob, home)
            self.assertIn('"name": "Write"', uploaded)
            self.assertIn("Keep the page in the database.", uploaded)
