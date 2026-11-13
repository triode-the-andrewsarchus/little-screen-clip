#!/usr/bin/env python3
"""Exercise native harness strips, source preview and Markdown embedding."""
import hashlib
from pathlib import Path
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from text_strip_acceptance import hover_observation


SOURCE = (Path(__file__).resolve().parents[1] / "docs/examples/diagrams/harness.wireviz").read_text()


def fixtures(root):
    (root / "a-harness.wireviz").write_text(SOURCE)
    (root / "b-harness.yaml").write_text(SOURCE)
    (root / "c-ordinary.yaml").write_text("title: Ordinary configuration\nconnections: database\ndraft: false\n")
    (root / "d-harness.md").write_text("# Harness overview\n\n```wireviz\n" + SOURCE + "```\n")


def has_rendered_headers(path):
    with Image.open(path).convert("RGB") as image:
        pixels = iter(image.tobytes())
        return sum(30 <= r <= 39 and 50 <= g <= 61 and 70 <= b <= 83
                   for r, g, b in zip(pixels, pixels, pixels)) > 800


def rendered_observation(session, name, expected):
    path = session.output / f"{name}.png"
    def ready():
        session.observe(name, expected, timeout_seconds=45)
        return has_rendered_headers(path)
    wait_for(ready, "rendered harness tables rather than matching source text")


def exercise(session, root):
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}
    session.observe("00-folder", ["a-harness.wireviz", "b-harness.yaml"], timeout_seconds=45)
    session.click(647, 160)  # Collapse Shelf so all four normal rows are visible.
    session.observe("01-folder", ["a-harness.wireviz", "b-harness.yaml", "c-ordinary.yaml", "d-harness.md"], timeout_seconds=45)
    _, first_y = session.word("01-folder", "a-harness.wireviz")
    hover_observation(session, "02-harness-strip", 700, first_y + 42,
                      ["WireViz", "RETURN", "POWER", "DATA", "Ground", "Supply", "Signal"],
                      forbidden=["Rendered preview unavailable"])
    assert has_rendered_headers(session.output / "02-harness-strip.png")
    _, second_y = session.word("01-folder", "b-harness.yaml")
    hover_observation(session, "03-generic-yaml", 700, second_y + 42,
                      ["WireViz", "RETURN", "POWER", "Ground", "Supply", "Signal"],
                      forbidden=["Rendered preview unavailable"])
    assert has_rendered_headers(session.output / "03-generic-yaml.png")
    x, y = session.word("01-folder", "a-harness.wireviz")
    session.click(x + 20, y)
    session.key("Return")
    rendered_observation(session, "04-source-preview", ["WireViz", "RETURN", "POWER", "DATA", "Ground", "Supply", "Signal"])
    session.click(1120, 71)  # Use the visible Back to folder action from source editing.
    session.observe("05-folder-return", ["d-harness.md"])
    x, y = session.word("05-folder-return", "d-harness.md")
    session.click(x + 20, y)
    session.key("Return")
    rendered_observation(session, "06-markdown-preview", ["Harness overview", "RETURN", "POWER", "DATA", "Ground", "Supply", "Signal"])
    after = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}
    assert before == after, "passive rendering must not rewrite harness source"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "native WireViz harness previews"))
