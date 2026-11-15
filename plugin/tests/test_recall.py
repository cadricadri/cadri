"""Prompt lookup scans the files and returns matching lines, rare words first."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import support  # noqa: F401  (puts the plugin on sys.path)

from cadri.memory import recall


class Book:
    def __init__(self, path: Path):
        self.path = path
        self.label = "team"


def write(root: Path, relative: str, text: str) -> None:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


class MentionTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "team"
        write(self.root, "architecture/MEMORY.md", "# Architecture\n\nPrior plans live here.\n- [checkout sequence](checkout-sequence.md) — the checkout flow\n")
        write(
            self.root,
            "architecture/checkout-sequence.md",
            "---\ndescription: Sequence diagram for checkout, from cart through payment to the receipt.\n---\n"
            + ("padding\n" * 400)
            + "Payment is authorized before the receipt.\n",
        )
        write(self.root, "features/MEMORY.md", "# Features\n\nOne file per feature and why it exists.\n")
        write(self.root, "features/checkout/why.md", "---\ndescription: Why checkout is synchronous.\n---\n# Checkout\n\nBuyers wait for payment.\n")
        self.book = Book(self.root)

    def test_a_word_past_the_old_opening_window_is_found(self):
        found = recall.mentions([self.book], "Where is payment authorized?")
        hits = [hit for mention in found for hit in mention.shown]
        self.assertTrue(any(hit.path == "architecture/checkout-sequence.md" and "authorized" in hit.text for hit in hits))

    def test_a_missing_word_does_not_pull_in_nearby_files(self):
        found = recall.mentions([self.book], "Plan a partial refunds feature")
        paths = {hit.path for mention in found for hit in mention.shown}
        terms = [mention.term for mention in found]
        self.assertNotIn("architecture/checkout-sequence.md", paths)
        self.assertNotIn("refund", terms)
        self.assertIn("feature", terms)
        self.assertLess(terms.index("plan"), terms.index("feature"))

    def test_an_unknown_prompt_matches_nothing(self):
        self.assertEqual(recall.mentions([self.book], "hello"), [])
        self.assertEqual(recall.render([self.book], "hello"), "")

    def test_a_match_starts_with_the_sentence_the_user_sees(self):
        self.assertEqual(recall.status_line(1), "Cadri remembered 1 thing that may help")
        text = recall.render([self.book], "Why is checkout synchronous?")
        self.assertTrue(text.startswith("Cadri remembered "))
        self.assertIn("features/checkout/why.md", text)
