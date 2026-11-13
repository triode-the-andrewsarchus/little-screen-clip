#!/usr/bin/env python3
"""Verify upstream UML artwork in native file strips, source and Markdown views."""
import hashlib
from pathlib import Path
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from text_strip_acceptance import hover_observation

SOURCE = (Path(__file__).resolve().parents[1] / "docs/examples/diagrams/pipeline.nomnoml").read_text()


def artwork(path, minimum=200):
    with Image.open(path).convert("RGB") as image:
        # The fixture's node fill must occupy real painted area; source words
        # appearing in the syntax editor cannot prove that rendering worked.
        pixels = iter(image.tobytes())
        return sum(abs(r - 144) < 3 and abs(g - 238) < 3 and abs(b - 144) < 3
                   for r, g, b in zip(pixels, pixels, pixels)) > minimum


def rendered(session, name, expected):
    def ready():
        session.observe(name, expected, timeout_seconds=45)
        return artwork(session.output / f"{name}.png")
    wait_for(ready, "native UML node artwork")


def exercise(session, root):
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}
    session.observe("01-folder", ["a-pipeline.nomnoml", "b-invalid.nomnoml", "c-overview.md"], timeout_seconds=45)
    session.click(647, 160)  # Collapse Shelf in the fixed private viewport.
    session.observe("02-strips", ["a-pipeline.nomnoml", "b-invalid.nomnoml", "c-overview.md"])
    assert artwork(session.output / "02-strips.png", minimum=2000)
    _, y = session.word("02-strips", "a-pipeline.nomnoml")
    hover_observation(session, "03-popout", 700, y + 42, ["Nomnoml"])
    assert artwork(session.output / "03-popout.png")
    _, y = session.word("02-strips", "b-invalid.nomnoml")
    hover_observation(session, "04-invalid", 700, y + 42, ["Rendered preview unavailable"])
    x, y = session.word("02-strips", "a-pipeline.nomnoml")
    session.click(x + 20, y)
    session.key("Return")
    rendered(session, "05-source", ["Diagram", "Source", "Worker", "Preview"])
    session.click(1120, 71)
    session.observe("06-return", ["c-overview.md"])
    x, y = session.word("06-return", "c-overview.md")
    session.click(x + 20, y)
    session.key("Return")
    rendered(session, "07-markdown", ["UML overview", "Diagram source"])
    after = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}
    assert before == after, "passive rendering must preserve all source files"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-pipeline.nomnoml").write_text(SOURCE)
    (root / "b-invalid.nomnoml").write_text("[Broken")
    (root / "c-overview.md").write_text("# UML overview\n\n```nomnoml\n" + SOURCE + "```\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "native Nomnoml previews"))
