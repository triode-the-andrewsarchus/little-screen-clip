#!/usr/bin/env python3
"""Exercise space-aware strips, shared Columns lists and archive navigation."""
from file_manager_columns_acceptance import resize
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
    session.click(600, 160)  # Shelf toggle observed in the browser toolbar.
    session.observe("strips", ["a-small", "Inside folder", "Open"])
    # A folder's entry square occupies the same 64px area as its direct cards.
    _, y = session.word("strips", "a-small", minimum_y=210)
    session.click(385, y + 50)
    session.observe("entered-folder", ["readme.txt", "sun.png"])
    session.key("Left", "alt")
    session.observe("returned", ["a-small", "c-package.zip"])
    _, y = session.word("returned", "c-package.zip", minimum_y=210)
    session.click(385, y + 50)
    session.observe("archive-root", ["Browsing compressed archive", "assets", "c-package.zip"])
    assert not (root / "assets").exists()
    session.key("Up", "alt")
    session.observe("archive-parent", ["a-small", "c-package.zip"])
    _, y = session.word("archive-parent", "c-package.zip", minimum_y=210)
    session.click(455, y + 55)
    session.observe("member-folder", ["payload.json", "Browsing compressed archive"])
    session.key("Up", "alt")
    session.observe("member-parent", ["assets", "Browsing compressed archive"])
    session.key("Up", "alt")
    session.observe("member-return", ["a-small", "c-package.zip"])
    session.click(176, 160)
    session.observe("columns", ["a-small", "c-package.zip"])
    session.address(root / "a-small/nested")
    session.observe("deep-columns", ["a-small", "nested", "deep.txt"])
    resize(session, 760, 700)
    session.observe("narrow-columns", ["a-small", "nested", "deep.txt"])
    resize(session, 1120, 760)
    session.address(root)
    session.observe("columns-root", ["c-package.zip"])
    # Keyboard navigation uses the shared List selection and activation policy.
    session.key("Home")
    session.key("c")
    session.key("Return")
    session.observe("archive-columns", ["Browsing compressed archive", "assets"])
    session.key("Up", "alt")
    session.observe("columns-archive-parent", ["c-package.zip", "a-small"])
    session.key("q", "ctrl")


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Square folder activation, scrollable folder chain, shared rich List column and persistent compressed archive status"))
