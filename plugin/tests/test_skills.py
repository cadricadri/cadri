"""Shipped skills stay installable instructions: every file is shipped, links resolve, and cadri commands exist."""

from __future__ import annotations

import re
import unittest
from pathlib import Path

import sys

PLUGIN = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(PLUGIN))

from cadri.cli import build_parser  # noqa: E402
from cadri.install.bundles import SHIPPED_SKILLS  # noqa: E402

SKILLS = PLUGIN / "skills"
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
# Book paths such as `architecture/MEMORY.md` are not files in the skill tree.
PATH_REF = re.compile(r"`((?:references/|cadri-[\w-]+/)[\w./-]+\.md)`")
COMMAND = re.compile(r"(?<![\w./-])cadri(?:\s+memory)?\s+([a-z][a-z-]*)")


def commands() -> set[str]:
    parser = build_parser()
    top = set(parser._subparsers._group_actions[0].choices)
    memory = parser._subparsers._group_actions[0].choices["memory"]
    nested = set(memory._subparsers._group_actions[0].choices)
    return {f"cadri {name}" for name in top} | {f"cadri memory {name}" for name in nested}


def frontmatter(text: str) -> dict[str, str]:
    if not text.startswith("---\n"):
        raise AssertionError("skill is missing frontmatter")
    body = text.split("\n---\n", 1)[0][4:]
    found = {}
    for line in body.splitlines():
        key, _, value = line.partition(":")
        if key and value:
            found[key.strip()] = value.strip()
    return found


def referenced_markdown(text: str) -> list[str]:
    found = [match for match in LINK.findall(text) if not match.startswith(("http://", "https://", "#"))]
    found.extend(PATH_REF.findall(text))
    return found


class SkillContractTests(unittest.TestCase):
    def test_every_skill_on_disk_is_shipped(self):
        on_disk = sorted(path.name for path in SKILLS.iterdir() if (path / "SKILL.md").is_file())
        self.assertEqual(on_disk, sorted(SHIPPED_SKILLS))

    def test_frontmatter_names_the_directory_and_links_resolve(self):
        for name in SHIPPED_SKILLS:
            with self.subTest(skill=name):
                root = SKILLS / name
                text = (root / "SKILL.md").read_text(encoding="utf-8")
                meta = frontmatter(text)
                self.assertEqual(meta.get("name"), name)
                self.assertGreater(len(meta.get("description", "")), 20)
                for relative in referenced_markdown(text):
                    target = (SKILLS / relative) if relative.split("/", 1)[0] in SHIPPED_SKILLS else (root / relative)
                    self.assertTrue(target.is_file(), f"{name} links to missing {relative}")

    def test_skill_commands_are_real_cli_commands(self):
        known = commands()
        for path in SKILLS.rglob("*.md"):
            for match in COMMAND.finditer(path.read_text(encoding="utf-8")):
                verb = match.group(0)
                with self.subTest(file=path.name, command=verb):
                    self.assertIn(verb, known)


if __name__ == "__main__":
    unittest.main()
