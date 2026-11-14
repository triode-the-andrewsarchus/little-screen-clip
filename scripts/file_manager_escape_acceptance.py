#!/usr/bin/env python3
"""Check Escape returns from file-backed viewers on private Weston/Niri."""
from pathlib import Path
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance

CASES = [("calendar", "harbor.ics"), ("toolpath", "frame.gcode"), ("notebook", "sample.ipynb")]


def exercise(session, root):
    session.address(root / "pictures")
    session.observe("pictures", ["a.png", "b.png", "Columns"])
    for view in ["list", "columns"]:
        if view == "columns":
            session.click(176, 160)
        session.key("Down" if view == "list" else "Up")
        session.key("Return")
        session.observe(f"{view}-first-picture", ["Back to folder", "a.png"])
        session.key("Escape")
        session.observe(f"{view}-picture-return", ["a.png", "b.png", "Columns"])
        # No file/background click after Escape: the next arrow and Enter
        # must immediately open the next picture in the existing folder.
        session.key("Down")
        session.key("Return")
        session.observe(f"{view}-next-picture", ["Back to folder", "b.png"])
        # Both filenames also appear in the filmstrip; inspect the actual
        # canvas color so reopening the first image cannot satisfy the check.
        for shot, color in [(f"{view}-first-picture", (37, 111, 156)), (f"{view}-next-picture", (169, 107, 38))]:
            with Image.open(session.output / f"{shot}.png") as image:
                assert image.convert("RGB").getpixel((640, 500)) == color, (view, shot)
        session.key("Escape")
        session.observe(f"{view}-second-return", ["a.png", "b.png", "Columns"])
    session.click(120, 160)
    for folder, name in CASES:
        session.address(root / folder)
        shot = f"{folder}-browser"
        session.observe(shot, [Path(name).stem, "Columns"])
        session.key("Down")
        session.key("Return")
        session.observe(f"{folder}-viewer", ["Back to folder", name])
        session.key("Escape")
        text = session.observe(f"{folder}-returned", [Path(name).stem, "Columns", "Folder previews"])
        assert "Back to folder" not in text, "Escape left the file viewer open"


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    pictures = root / "pictures"
    pictures.mkdir()
    for name, color in [("a.png", "#256f9c"), ("b.png", "#a96b26")]:
        Image.new("RGB", (160, 100), color).save(pictures / name)
    sources = [repo / "crates/little-clip-core/tests/fixtures/calendar/harbor.ics",
               repo / "crates/little-clip-core/tests/fixtures/gcode/frame.gcode",
               repo / "crates/clip-file-manager/tests/fixtures/notebooks/sample.ipynb"]
    for (folder, name), source in zip(CASES, sources):
        (root / folder).mkdir()
        shutil.copyfile(source, root / folder / name)
    return run_application_acceptance(args, output, root, exercise,
        "Immediate arrow/Enter navigation after image-editor Escape in List and Columns; ordinary viewer Escape returns")


if __name__ == "__main__":
    raise SystemExit(main())
