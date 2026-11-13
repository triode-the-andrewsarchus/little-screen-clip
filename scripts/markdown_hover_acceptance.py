#!/usr/bin/env python3
"""Verify rendered Markdown hover follows the selected source in private Files."""
import hashlib
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from text_strip_acceptance import hover_observation


def fixtures(root):
    source = "# Beginning\n\nWelcome to the **first section**.\n\n"
    source += "## Middle section\n\nOrdinary **styled** paragraphs.\n\n- An item\n\n" * 30
    source += "## Final destination\n\nThis is the **last section** with `inline code`.\n"
    (root / "a-document.md").write_text(source)
    (root / "b-diagram.md").write_text("# Diagram\n\n```mermaid\nflowchart LR\n Start --> Inspect --> Done\n```\n")
    (root / "c-unfinished.md").write_text("```mermaid\nflowchart LR\n Start --> Unfinished\n")


def exercise(session, root):
    before = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.glob("*.md")}
    session.observe("01-documents", ["a-document.md", "b-diagram.md", "c-unfinished.md"])
    _, y = session.word("01-documents", "a-document.md")
    # The private runner fixes the client at 1280x800, with the full-width
    # minimap from x=424 to 1168 and no front-matter panel for these fixtures.
    hover_observation(session, "02-beginning", 426, y + 42, ["Markdown", "Beginning", "first section"], forbidden=["Preparing", "unavailable"])
    hover_observation(session, "03-destination", 1156, y + 42, ["Markdown", "Final destination", "last section"], forbidden=["Preparing", "unavailable"])
    _, diagram_y = session.word("01-documents", "b-diagram.md")
    hover_observation(session, "04-complete-diagram", 900, diagram_y + 42, ["Diagram source", "Inspect", "Done"])
    _, broken_y = session.word("01-documents", "c-unfinished.md")
    hover_observation(session, "05-unfinished-source", 850, broken_y + 42, ["Rendered Markdown unavailable", "Incomplete Markdown fence"])
    after = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.glob("*.md")}
    assert before == after, "rendered hover must not modify Markdown source"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "Markdown source-position hover"))
