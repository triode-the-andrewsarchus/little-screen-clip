#!/usr/bin/env python3
"""Exercise Image Studio's browser collection on private Weston/Niri sockets."""
from pathlib import Path
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
import ui_acceptance_capture as private
import gzip
import subprocess
import re


def click_word(session, name, word, **kwargs):
    position = session.word(name, word, required=False, **kwargs)
    if position is None:
        # Sparse OCR sometimes drops the filename separator or reads it as a comma.
        position = session.word(name, word.replace(".", ","), required=False, **kwargs)
    if position is None:
        position = session.word(name, word.replace(".", ""), **kwargs)
    x, y = position
    session.click(x + 8, y)


def click_control(session, name, word):
    # Observed coordinates in the runner's fixed 1280x800 private viewport.
    session.click({"Back": 975, "Export": 1110}[word], 70)


def flip_photo(session, name):
    session.key("f")
    session.observe(name, ["Mirror"])
    session.click(160, 588)


def observe_badge(session, name, expected):
    text = session.observe(name, expected)
    crop = session.output / f"{name}-badge.png"
    Image.open(session.output / f"{name}.png").crop((100, 102, 161, 118)).resize((610, 160)).save(crop)
    text = private.run_checked(["tesseract", str(crop), "stdout", "--psm", "7"], env=session.env)
    assert "unsaved" in text.lower(), text
    return (session.output / f"{name}.txt").read_text()


def include_strip_ocr(session):
    # Full-window sparse OCR can omit the compact collection control row.
    original = session.capture_text
    def capture(name, path, expected):
        text = original(name, path, expected)
        crop = session.output / f"{name}-collection-controls.png"
        Image.open(path).crop((90, 50, 1180, 90)).resize((4360, 200)).save(crop)
        text += private.run_checked(["tesseract", str(crop), "stdout", "--psm", "6"], env=session.env)
        header = session.output / f"{name}-image-header.png"
        Image.open(path).crop((90, 200, 1180, 260)).resize((4360, 240)).save(header)
        text += private.run_checked(["tesseract", str(header), "stdout", "--psm", "6"], env=session.env)
        footer = session.output / f"{name}-image-footer.png"
        Image.open(path).crop((90, 720, 1180, 775)).resize((4360, 220)).save(footer)
        text += private.run_checked(["tesseract", str(footer), "stdout", "--psm", "6"], env=session.env)
        return re.sub(r"\b(FIRST|SECOND|RENAMED)[,.]?(JFIF|SVGZ|JXL)\b", r"\1.\2", text)
    session.capture_text = capture


def exercise(session, root, extension="png"):
    extension = extension.upper() if extension in {"jfif", "svgz", "jxl"} else extension
    first, second, renamed = (f"{stem}.{extension}" for stem in ["FIRST", "SECOND", "RENAMED"])
    originals = {first: (root / first).read_bytes(), second: (root / second).read_bytes()}
    include_strip_ocr(session)
    session.observe("01-browser", [first, second])
    with Image.open(session.output / "01-browser.png") as shot:
        pixels = list(shot.convert("RGB").getdata())
        assert sum(r - g > 60 and r - b > 60 for r, g, b in pixels) > 100, "Red photo preview missing"
        assert sum(b - r > 60 and b - g > 40 for r, g, b in pixels) > 100, "Blue photo preview missing"
    click_word(session, "01-browser", first)
    session.key("Return")
    session.observe("02-studio", ["Back to folder", "Reset", "1:1", "1 / 2"])
    session.key("5")
    # A committed flip creates a visible change and an undo step.
    flip_photo(session, "03-transform")
    observe_badge(session, "04-edited", ["Export"])
    session.key("Right")
    observe_badge(session, "05-next", ["2 / 2", second])
    flip_photo(session, "05-transform")
    session.key("Left")
    observe_badge(session, "06-restored", ["1 / 2", first])
    session.click(220, 230)
    session.observe("07-rename", ["Filename"])
    # The rename dialog focuses and selects the current filename.
    session.key("a", "ctrl")
    private.run_checked(["wtype", renamed], env=session.env)
    # Capture the field; the exact typed name is verified by the resulting file.
    session.observe("07-typed", ["Filename", "Rename file"])
    # Choose the dialog's Rename, below the strip.
    session.key("Return")
    wait_for(lambda: (root / renamed).exists(), "rename completed")
    rename_text = observe_badge(session, "08-renamed", [renamed])
    assert "photo renamed" not in rename_text.lower()
    assert not (root / first).exists()
    assert (root / renamed).exists()
    click_control(session, "08-renamed", "Export")
    session.observe("09-export", ["Output folder", "Export PNG copies"])
    session.click(540, 525)
    wait_for(lambda: (root / "Edited" / "RENAMED_edit.png").exists() and (root / "Edited" / "SECOND_edit.png").exists(), "both exports completed")
    text = session.observe("10-exported", ["Export all edits", renamed])
    for redundant in ["Photo renamed", "Exported 2", "Open externally", "IMAGE STUDIO"]:
        assert redundant.lower() not in text.lower(), redundant
    assert (root / "Edited" / "RENAMED_edit.png").exists()
    assert (root / "Edited" / "SECOND_edit.png").exists()
    # Export preserves both the source image and its editor history.
    with Image.open(root / "Edited" / "RENAMED_edit.png") as edited:
        if extension in {"SVGZ", "JXL"}:
            assert edited.size == (640, 400)
            assert edited.getpixel((0, 0))[:3] == (180, 60, 40)
        else:
            assert Image.open(root / renamed).getpixel((0, 0)) != edited.getpixel((0, 0))
    session.observe("11-before-return", ["Back to folder"])
    click_control(session, "11-before-return", "Back")
    session.observe("12-browser-return", [renamed, second])
    click_word(session, "12-browser-return", renamed)
    session.key("Return")
    session.observe("13-reopened", ["1 / 2", renamed])
    assert (root / renamed).read_bytes() == originals[first]
    assert (root / second).read_bytes() == originals[second]
    # Ratings survive returning to the browser and reopening the collection.
    ratings = list(Path(session.env["XDG_DATA_HOME"]).glob("little-screen-clip/photo-ratings-v1/*.json"))
    assert any(path.read_text() == "5" for path in ratings)


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager",
        lambda parser: parser.add_argument("--format", choices=["png", "jfif", "svgz", "jxl"], default="png"))
    output, root = media_fixture_directory(args)
    image = Image.new("RGB", (640, 400), (180, 60, 40))
    for x in range(320):
        for y in range(400):
            image.putpixel((x, y), (30, 120, 210))
    extension = args.format.upper() if args.format in {"jfif", "svgz", "jxl"} else args.format
    if args.format == "svgz":
        first = '<svg xmlns="http://www.w3.org/2000/svg" width="640" height="400"><rect width="320" height="400" fill="#1e78d2"/><rect x="320" width="320" height="400" fill="#b43c28"/></svg>'
        second = '<svg xmlns="http://www.w3.org/2000/svg" width="400" height="500"><rect width="400" height="500" fill="#5aaa46"/></svg>'
        for name, markup in [("FIRST",first),("SECOND",second)]:
            (root / f"{name}.{extension}").write_bytes(gzip.compress(markup.encode(), mtime=0))
    elif args.format == "jxl":
        for index, (name, pixels) in enumerate([("FIRST", image), ("SECOND", Image.new("RGB", (400, 500), (90, 170, 70)))]):
            source = root / f"{name}.png"
            pixels.save(source)
            subprocess.run(["cjxl", str(source), str(root / f"{name}.{extension}"), "-d", "0", "-e", "1", f"--container={index}", "--num_threads=1"], check=True, timeout=30)
            source.unlink()
    else:
        image.save(root / f"FIRST.{extension}", format="JPEG" if args.format == "jfif" else "PNG")
        Image.new("RGB", (400, 500), (90, 170, 70)).save(root / f"SECOND.{extension}", format="JPEG" if args.format == "jfif" else "PNG")
    return run_application_acceptance(args, output, root, lambda session, root: exercise(session, root, args.format), "Image Studio photo strip, ratings, retained edits, rename and export")


if __name__ == "__main__":
    raise SystemExit(main())
