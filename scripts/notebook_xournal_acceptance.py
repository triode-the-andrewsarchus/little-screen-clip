#!/usr/bin/env python3
"""Exercise real Xournal documents through list strips and read-only page studios."""
import hashlib
from pathlib import Path
import shutil
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file
from notebook_preview_acceptance import page_columns, observe_page
from text_strip_acceptance import hover_observation


def fixtures(root):
    source = Path(__file__).resolve().parent.parent / "docs/examples/notebooks/xournal"
    for original, name in [
        ("load/pages.xopp", "a-pages.xopp"),
        ("load/strokes.xopp", "b-strokes.xopp"),
        ("legacy-stroke.xoj", "c-legacy.xoj"),
        ("packaged/pdf-background.xopp", "d-package.xopp"),
        ("load/latex-fileversion-5.xopp", "e-equations.xopp"),
        ("packaged/image.xopp", "f-image.xopp"),
        ("load/relativePaths.xopp", "g-missing.xopp"),
    ]:
        shutil.copyfile(source / original, root / name)
    shutil.copyfile(source / "load/pages.xopp.bg_1.png", root / "a-pages.xopp.bg_1.png")


def colored_page(path):
    with Image.open(path).convert("RGB") as image:
        pixels = image.crop((380, 242, 1165, 712)).tobytes()
        return sum(max(pixels[index:index + 3]) - min(pixels[index:index + 3]) > 70
                   for index in range(0, len(pixels), 3)) > 20000


def page_geometry(path):
    with Image.open(path).convert("RGB") as image:
        canvas = image.crop((380, 242, 1165, 712))
        paper = [(x, y) for y in range(canvas.height) for x in range(canvas.width)
                 if min(canvas.getpixel((x, y))) >= 254]
        if len(paper) < 30000:
            return False
        bounds = (min(x for x, _ in paper), min(y for _, y in paper),
                  max(x for x, _ in paper), max(y for _, y in paper))
        pixels = canvas.crop(bounds).tobytes()
        return sum(min(pixels[index:index + 3]) < 230
                   for index in range(0, len(pixels), 3)) > 200


def exercise(session, root):
    original = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    session.observe("00-folder", ["a-pages.xopp", "b-strokes.xopp"])
    session.click(645, 160)  # Collapse Shelf to expose the page strips.
    session.observe("01-rows", ["a-pages.xopp", "b-strokes.xopp", "c-legacy.xoj"])
    _, y = session.word("01-rows", "a-pages.xopp")
    columns = []
    def rendered():
        nonlocal columns
        path = session.output / "02-page-strip.png"
        session.capture_text("02-page-strip", path, [])
        columns = page_columns(path, y)
        if not columns:
            return False
        first = sum(columns[0]) // 2
        with Image.open(path).convert("RGB") as image:
            for index in range(10):
                center = first + index * 56
                pixels = image.crop((center - 12, y + 23, center + 12, y + 61)).tobytes()
                paper = sum(min(pixels[offset:offset + 3]) > 200 for offset in range(0, len(pixels), 3))
                if paper < len(pixels) // 5:
                    return False
            last = first + 10 * 56
            pixels = image.crop((last - 12, y + 23, last + 12, y + 61)).tobytes()
            return sum(max(pixels[offset:offset + 3]) - min(pixels[offset:offset + 3]) > 70
                       for offset in range(0, len(pixels), 3)) > 400
    wait_for(rendered, "rendered Xournal page strips", timeout_seconds=45)
    # The last page is a colored bitmap, so it intentionally is not detected as
    # white paper. Cell spacing is shared by every page; hover verifies the hit.
    x = sum(columns[0]) // 2 + 10 * 56
    hover_observation(session, "03-last-page-hover", x, y + 43, ["Page 11"])
    session.click(x, y + 43, delay=.05)
    session.click(x, y + 43, delay=.05)
    observe_page(session, "04-page-eleven", ["Page 11 of 11"], rendered=colored_page)
    text = (session.output / "04-page-eleven.txt").read_text()
    assert not any(action in text for action in ["Okular", "Rotate", "Delete", "OCR"])
    session.click(850, 420)
    session.key("r")
    session.key("Delete")
    session.observe("05-read-only", ["Page 11 of 11"])
    session.click(1120, 72)
    for filename, label, expected, predicate in [
        ("b-strokes.xopp", "strokes", ["1 pages", "Page 1 of 1"], page_geometry),
        ("c-legacy.xoj", "legacy", ["1 pages", "Page 1 of 1"], page_geometry),
        ("d-package.xopp", "packaged-pdf", ["2 pages", "Page 1 of 2"], page_geometry),
        ("e-equations.xopp", "latex", ["1 pages", "Page 1 of 1"], page_geometry),
        ("f-image.xopp", "packaged-image", ["1 pages", "Page 1 of 1"], colored_page),
    ]:
        open_file(session, root, filename, f"{label}-row")
        observe_page(session, label, expected, rendered=predicate)
        session.click(1120, 72)
    open_file(session, root, "g-missing.xopp", "missing-row")
    session.observe("missing-background", ["background unavailable"], timeout_seconds=45)
    session.click(1120, 72)
    assert {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()} == original


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Xournal gzip/legacy/ZIP strips, late-page handoff, actual ink/images/PDF/LaTeX, missing-background diagnostics and unchanged sources"))
