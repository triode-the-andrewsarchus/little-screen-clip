#!/usr/bin/env python3
"""Check reMarkable strips and read-only studios on an isolated native desktop."""
import hashlib
from pathlib import Path
import shutil
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file
from notebook_preview_acceptance import page_columns, observe_page
from notebook_xournal_acceptance import page_geometry
from text_strip_acceptance import hover_observation


def fixtures(root):
    source = Path(__file__).resolve().parent.parent / "docs/examples/notebooks/remarkable"
    for original, name in [
        ("archives/Test.rmdoc", "a-notice.rmdoc"),
        ("generated/layered-template.rmn", "b-grid.rmn"),
        ("legacy-real/test_v5_math.rm", "c-legacy.rm"),
        ("rmscene/Color_and_tool_v3.14.4.rm", "d-current.rm"),
        ("generated/redirected-pdf.rmdoc", "e-pdf.rmdoc"),
        ("generated/legacy-paper.rmn", "f-legacy-grid.rmn"),
    ]:
        shutil.copyfile(source / original, root / name)


def paper_color(color):
    def painted(path):
        with Image.open(path).convert("RGB") as image:
            pixels = image.crop((380, 242, 1165, 712)).tobytes()
            return sum(all(abs(pixels[index + channel] - color[channel]) <= 2 for channel in range(3))
                       for index in range(0, len(pixels), 3)) > 20000
    return painted


def exercise(session, root):
    original = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}
    session.observe("00-folder", ["a-notice.rmdoc", "b-grid.rmn"])
    session.click(645, 160)  # Collapse Shelf using the existing private fixture layout.
    session.observe("01-rows", ["a-notice.rmdoc", "b-grid.rmn"])
    _, y = session.word("01-rows", "a-notice.rmdoc")
    columns = []
    def rendered():
        nonlocal columns
        path = session.output / "02-strips.png"
        session.capture_text("02-strips", path, [])
        columns = page_columns(path, y)
        return len(columns) == 2
    wait_for(rendered, "two rendered reMarkable page thumbnails", timeout_seconds=45)
    session.observe("02-notice", ["Paper unavailable"])
    x = sum(columns[-1]) // 2
    hover_observation(session, "03-page-hover", x, y + 43, ["Page 2"])
    session.click(x, y + 43, delay=.05)
    session.click(x, y + 43, delay=.05)
    observe_page(session, "04-page-two", ["2 pages", "Page 2 of 2", "Paper artwork unavailable"], rendered=page_geometry)
    text = (session.output / "04-page-two.txt").read_text()
    assert not any(action in text for action in ["Okular", "Rotate", "Delete", "OCR"])
    session.click(850, 420)
    session.key("r")
    session.key("Delete")
    session.observe("05-read-only", ["Page 2 of 2"])
    session.click(1120, 72)
    for name, label, expected, predicate in [
        ("b-grid.rmn", "template", ["2 pages", "Page 1 of 2"], paper_color((255, 248, 224))),
        ("c-legacy.rm", "legacy", ["1 pages", "Page 1 of 1"], page_geometry),
        ("d-current.rm", "current", ["1 pages", "Page 1 of 1"], page_geometry),
        ("e-pdf.rmdoc", "pdf", ["3 pages", "Page 1 of 3"], paper_color((204, 230, 255))),
        ("f-legacy-grid.rmn", "legacy-template", ["2 pages", "Page 1 of 2"], paper_color((255, 248, 224))),
    ]:
        open_file(session, root, name, f"{label}-row")
        observe_page(session, label, expected, rendered=predicate)
        assert "Paper artwork unavailable" not in (session.output / f"{label}.txt").read_text()
        session.click(1120, 72)
    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()} == original


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "reMarkable real v5/v6, RMDOC/RMN strips, selected-page handoff, paper notices, nested templates, PDF background and read-only sources"))
