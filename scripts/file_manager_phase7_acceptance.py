#!/usr/bin/env python3
"""Review contained-file previews on an isolated native compositor."""
from pathlib import Path
import io
import tarfile
import zipfile
from PIL import Image, ImageDraw
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    small = root / "a-small"
    small.mkdir()
    (small / "readme.txt").write_text("Contained document opened successfully.\n", encoding="utf-8")
    nested = small / "nested"
    nested.mkdir()
    (nested / "deep.txt").write_text("Nested folder child.\n", encoding="utf-8")
    image = Image.new("RGB", (180, 120), "#163f52")
    draw = ImageDraw.Draw(image)
    draw.ellipse((35, 15, 145, 105), fill="#edb84b")
    image.save(small / "sun.png")
    large = root / "b-large"
    large.mkdir()
    for i in range(40):
        (large / f"item-{i:02}.txt").write_text("bounded inventory\n", encoding="utf-8")
    with zipfile.ZipFile(root / "c-package.zip", "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("assets/payload.json", '{"archive":true}')
    with tarfile.open(root / "d-package.tar", "w") as archive:
        data = b"read-only member inventory\n"
        member = tarfile.TarInfo("docs/manual.txt")
        member.size = len(data)
        archive.addfile(member, io.BytesIO(data))


def exercise(session, root):
    session.observe("initial", ["a-small", "b-large"])
    session.click(645, 160)  # Collapse Shelf to expose all four fixture rows.
    session.observe("00-contained-files", ["a-small", "Inside folder", "readme.txt", "sun.png", "Archive contents", "payload.json", "manual"])
    session.click(755, 160)  # Folder previews menu in the observed toolbar.
    session.observe("preview-settings", ["Auto-expand small folders and archives"])
    x, y = session.word("preview-settings", "Auto-expand")
    session.click(x + 5, y)
    session.key("Escape")
    session.observe("automatic-off", ["a-small", "b-large", "c-package.zip"])
    off = (session.output / "automatic-off.txt").read_text()
    assert "readme.txt" not in off and "payload.json" not in off
    _, y = session.word("automatic-off", "a-small")
    session.click(360, y)
    session.observe("manual-override", ["readme.txt", "sun.png"])
    session.click(755, 160)
    session.observe("preview-settings-again", ["Auto-expand small folders and archives"])
    x, y = session.word("preview-settings-again", "Auto-expand")
    session.click(x + 5, y)
    session.key("Escape")
    session.observe("automatic-on", ["readme.txt", "payload.json", "manual"])
    # The large-folder preview automatically stays collapsed, while small ones
    # expose contained files. No members are extracted to the fixture directory.
    assert not (root / "assets").exists()
    assert not (root / "docs").exists()
    _, folder_y = session.word("00-contained-files", "a-small")
    session.click(360, folder_y)
    session.observe("collapsed", ["a-small", "Archive contents"])
    assert "readme.txt" not in (session.output / "collapsed.txt").read_text()
    session.click(360, folder_y)
    session.observe("expanded", ["nested", "readme.txt", "sun.png"])
    x, y = session.word("expanded", "nested")
    session.click(x + 2, y, delay=.05)
    session.click(x + 2, y, delay=.05)
    session.observe("nested-folder", ["deep.txt"])
    session.key("Left", "alt")
    session.observe("returned", ["readme.txt", "a-small"])
    x, y = session.word("returned", "readme.txt")
    session.click(x + 2, y, delay=.05)
    session.click(x + 2, y, delay=.05)
    session.observe("01-open-child", ["Contained document opened successfully", "Back to folder"])
    session.key("q", "ctrl")


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Initial small-folder and archive inventories, actual contained-file activation, no implicit extraction"))
