#!/usr/bin/env python3
"""Exercise real notebook strips, page handoff and read-only viewing on private Weston/Niri."""
import hashlib
from itertools import groupby
from pathlib import Path
import shutil

from PIL import Image
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)
from file_manager_phase4_acceptance import open_file
from text_strip_acceptance import hover_observation


def fixtures(root):
    sources = Path(__file__).resolve().parent.parent / "docs/examples/notebooks/supernote"
    for source, target in [
        ("demo-a5x-20230015-1to10.note", "a-pages.note"),
        ("layout-n5-20230015-horizontal-1090.note", "b-landscape.note"),
        ("render-n6-20230015-moonchild-user-bg.note", "c-template.note"),
    ]:
        shutil.copyfile(sources / source, root / target)
    (root / "d-ordinary.note").write_text("Ordinary text with a notebook-looking extension.\n")


def page_columns(path, y):
    """Find white page thumbnails, excluding labels, separators and placeholders."""
    with Image.open(path).convert("RGB") as image:
        columns = []
        for x in range(330, image.width - 30):
            white = sum(min(image.getpixel((x, row))) >= 254
                        for row in range(y + 18, min(y + 70, image.height)))
            if white >= 18:
                columns.append(x)
        groups = [list(group) for _, group in groupby(enumerate(columns), lambda pair: pair[1] - pair[0])]
        return [(group[0][1], group[-1][1]) for group in groups if len(group) >= 8]



def observe_page(session, name, expected, rendered=None):
    session.observe(name, expected, timeout_seconds=45)
    def painted():
        path = session.output / f"{name}.png"
        session.capture_text(name, path, expected)
        if rendered is not None:
            return rendered(path)
        with Image.open(path).convert("RGB") as image:
            canvas = image.crop((380, 242, 1165, 712))
            paper = [(x, y) for y in range(canvas.height) for x in range(canvas.width)
                     if min(canvas.getpixel((x, y))) >= 254]
            if len(paper) < 30000:
                return False
            bounds = (min(x for x, _ in paper), min(y for _, y in paper),
                      max(x for x, _ in paper), max(y for _, y in paper))
            pixels = canvas.crop(bounds).tobytes()
            ink = sum(max(pixels[index:index + 3]) < 130 for index in range(0, len(pixels), 3))
            return ink > 200
    wait_for(painted, f"{name}: visible paper and ink instead of loading placeholders", timeout_seconds=45)
    session.observe(name, expected)


def exercise(session, root):
    original = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    session.observe("00-folder", ["a-pages.note", "b-landscape.note"])
    session.click(645, 160)  # Collapse the Shelf to expose all fixture rows.
    session.observe("01-folder", ["a-pages.note", "b-landscape.note", "c-template.note", "d-ordinary.note"])
    _, y = session.word("01-folder", "a-pages.note")
    columns = []
    def rendered():
        nonlocal columns
        path = session.output / "02-rendered-strips.png"
        session.capture_text("02-rendered-strips", path, [])
        columns = page_columns(path, y)
        return len(columns) == 10
    wait_for(rendered, "ten real rendered notebook page thumbnails", timeout_seconds=45)
    session.observe("02-rendered-strips", ["a-pages.note"])
    x = sum(columns[-1]) // 2
    hover_observation(session, "03-last-page-hover", x, y + 43, ["Page 10"])
    session.click(x, y + 43, delay=0.05)
    session.click(x, y + 43, delay=0.05)
    observe_page(session, "04-selected-page", ["10 pages", "Page 10 of 10"])
    for shot in ["04-selected-page"]:
        text = (session.output / f"{shot}.txt").read_text()
        for forbidden in ["Okular", "Rotate", "Delete", "OCR", "Contents"]:
            assert forbidden not in text, f"Read-only notebook exposed PDF-only action: {forbidden}"
    session.click(850, 420)
    session.key("r")
    session.key("Delete")
    session.key("f", "ctrl")
    session.key("t")
    session.observe("05-read-only-shortcuts", ["10 pages", "Page 10 of 10"])
    session.click(1120, 72)  # Back to folder in the top menu bar.
    session.observe("06-return", ["a-pages.note", "b-landscape.note"])
    open_file(session, root, "b-landscape.note", "07-landscape-row")
    observe_page(session, "08-landscape", ["1 pages", "Page 1 of 1"])
    session.click(1120, 72)  # Back to folder in the top menu bar.
    open_file(session, root, "c-template.note", "09-template-row")
    observe_page(session, "10-template", ["1 pages", "Page 1 of 1"])
    session.click(1120, 72)  # Back to folder in the top menu bar.
    session.observe("11-folder", ["d-ordinary.note"])
    assert {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()} == original


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "real Supernote list strips, last-page handoff, landscape/template pages, read-only actions and unchanged sources"))
