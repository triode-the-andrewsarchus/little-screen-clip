#!/usr/bin/env python3
"""Verify rendered diagram list strips and enlarged hover on the private desktop."""
import hashlib
from pathlib import Path
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from text_strip_acceptance import hover_observation


def fixtures(root):
    (root / "a-timing.wavejson").write_text("""{
      head: {text: 'HiddenPreviewHeading', tick: 0},
      signal: [
        {name:'CLOCK', wave:'p.......'},
        {name:'DATA', wave:'x.345x..', data:['Read','Write','Done']},
        {name:'READY', wave:'0.1..0..'}
      ]
    }""")
    (root / "b-flow.mermaid").write_text("flowchart LR\n Start --> Inspect --> Done\n")
    (root / "c-invalid.wavejson").write_text("{signal: [")


def exercise(session, root):
    before = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir() if path.is_file()}
    session.observe("01-rendered-rows", ["a-timing.wavejson", "b-flow.mermaid", "c-invalid.wavejson"], timeout_seconds=45)
    text = (session.output / "01-rendered-rows.txt").read_text()
    assert "HiddenPreviewHeading" not in text
    _, first_y = session.word("01-rendered-rows", "a-timing.wavejson")
    _, second_y = session.word("01-rendered-rows", "b-flow.mermaid")
    assert second_y - first_y < 160, "rendered diagrams must not increase normal row height"
    with Image.open(session.output / "01-rendered-rows.png").convert("RGB") as image:
        pixels = [image.getpixel((x, y)) for x in range(430, 1150)
                  for y in range(first_y + 22, first_y + 76)]
        assert sum(min(pixel) > 245 for pixel in pixels) > len(pixels) * .6, "the timing row must use rendered paper instead of a syntax minimap"
        assert sum(max(pixel) < 100 for pixel in pixels) > 150, "rendered timing waveforms must contain visible signal geometry"
    hover_observation(session, "02-waveform-popout", 700, first_y + 42, ["WaveDrom", "CLOCK", "DATA", "READY"])
    hover_observation(session, "03-graph-popout", 700, second_y + 42, ["Mermaid", "Start", "Inspect", "Done"])
    _, third_y = session.word("01-rendered-rows", "c-invalid.wavejson")
    hover_observation(session, "04-invalid-fallback", 700, third_y + 42, ["Rendered preview unavailable"])
    after = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir() if path.is_file()}
    assert before == after, "passive rendering must not rewrite diagram source"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise, "rendered diagram strips and popouts"))
