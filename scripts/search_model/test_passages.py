"""Passage boundaries preserve source bytes and honor actual tokenizer counts."""
import unittest
from test_runtime import load

worker = load("worker")


class ByteTokenizer:
    def clip_search_token_count(self, model, text):
        return len(text) + 2


class Passages(unittest.TestCase):
    def test_multilingual_passages_cover_every_source_byte_with_bounded_overlap(self):
        text = ("A café. किताब पढ़ें。 音楽 🎵\n\n" * 400)
        encoded = text.encode()
        start, covered = 0, 0
        library = ByteTokenizer()
        while start < len(encoded):
            window = encoded[start:start + 8192].decode(errors="ignore")
            result = worker.passage(library, None, {"text": window})
            end, next_start = result["end"], result["next"]
            passage = encoded[start:start + end].decode()
            self.assertLessEqual(library.clip_search_token_count(None, ("title: none | text: " + passage).encode()), 768)
            self.assertLessEqual(start, covered)
            self.assertGreater(start + end, covered)
            overlap = encoded[start + next_start:start + end].decode()
            self.assertLessEqual(library.clip_search_token_count(None, overlap.encode()), 128)
            covered = start + end
            start = covered if covered == len(encoded) else start + next_start
        self.assertEqual(covered, len(encoded))

    def test_short_passage_keeps_entire_source_and_long_paragraphs_hard_split(self):
        for text in ["A short source.", "é" * 2000]:
            result = worker.passage(ByteTokenizer(), None, {"text": text})
            self.assertGreater(result["next"], 0)
            self.assertLessEqual(result["next"], result["end"])
            self.assertLessEqual(result["end"], len(text.encode()))
        self.assertEqual(worker.passage(ByteTokenizer(), None, {"text": "short"})["end"], 5)

    def test_paragraph_or_sentence_boundary_precedes_hard_cut(self):
        text = "a" * 500 + ".\n" + "b" * 400
        result = worker.passage(ByteTokenizer(), None, {"text": text})
        self.assertEqual(result["end"], 502)

    def test_invalid_and_over_budget_inputs_fail_explicitly(self):
        for text in ["", None, "\0", "x" * 8193]:
            with self.assertRaises(ValueError):
                worker.passage(ByteTokenizer(), None, {"text": text})


if __name__ == "__main__":
    unittest.main()
