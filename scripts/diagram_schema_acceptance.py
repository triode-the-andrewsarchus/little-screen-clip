#!/usr/bin/env python3
"""Verify schema artwork in native file strips, source and Markdown views."""
import hashlib
from pathlib import Path
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from text_strip_acceptance import hover_observation

EXAMPLES = Path(__file__).resolve().parents[1] / "docs/examples/diagrams"
FAMILIES = {
    "nomnoml": ("Nomnoml", "pipeline.nomnoml", (144, 238, 144), "[Broken"),
    "dbml": ("DBML", "schema.dbml", (34, 56, 77), "Table broken {"),
    "goat": ("GoAT", "pipeline.goat", (17, 17, 17), "a\tb"),
}


def artwork(path, color, minimum=200):
    with Image.open(path).convert("RGB") as image:
        # The fixture's node fill must occupy real painted area; source words
        # appearing in the syntax editor cannot prove that rendering worked.
        pixels = iter(image.tobytes())
        return sum(all(abs(a - b) < 3 for a, b in zip((r, g, b), color))
                   for r, g, b in zip(pixels, pixels, pixels)) > minimum


def rendered(session, name, expected, color, minimum=200):
    def ready():
        session.observe(name, expected, timeout_seconds=45)
        return artwork(session.output / f"{name}.png", color, minimum)
    wait_for(ready, "native schema node artwork")


def exercise(session, root):
    session.observe("00-families", list(FAMILIES))
    session.click(647, 160)  # Collapse Shelf once in the fixed private viewport.
    for syntax, (title, example, color, _) in FAMILIES.items():
        folder = root / syntax
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}
        session.address(folder)
        prefix = syntax + "-"
        valid, invalid = f"a-pipeline.{syntax}", f"b-invalid.{syntax}"
        rendered(session, prefix + "01-strips", [valid, invalid, "c-overview.md"], color, minimum=500)
        _, y = session.word(prefix + "01-strips", valid)
        hover_observation(session, prefix + "02-popout", 700, y + 42, [title])
        assert artwork(session.output / f"{prefix}02-popout.png", color)
        _, y = session.word(prefix + "01-strips", invalid)
        hover_observation(session, prefix + "03-invalid", 700, y + 42, ["Rendered preview unavailable"])
        x, y = session.word(prefix + "01-strips", valid)
        session.click(x + 20, y)
        session.key("Return")
        rendered(session, prefix + "04-source", ["Diagram", "Preview"], color)
        session.click(1120, 71)
        session.observe(prefix + "05-return", ["c-overview.md"])
        x, y = session.word(prefix + "05-return", "c-overview.md")
        session.click(x + 20, y)
        session.key("Return")
        rendered(session, prefix + "06-markdown", ["Schema overview", "Diagram source"], color)
        session.click(1120, 71)
        session.observe(prefix + "07-folder", [valid, "c-overview.md"])
        after = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in folder.iterdir()}
        assert before == after, "passive rendering must preserve all source files"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    for syntax, (_, example, _, invalid) in FAMILIES.items():
        folder = root / syntax
        folder.mkdir()
        source = (EXAMPLES / example).read_text()
        (folder / f"a-pipeline.{syntax}").write_text(source)
        (folder / f"b-invalid.{syntax}").write_text(invalid)
        (folder / "c-overview.md").write_text("# Schema overview\n\n```" + syntax + "\n" + source + "```\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "native schema diagram previews"))
