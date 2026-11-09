#!/usr/bin/env python3
"""Read EPUB chapters and rename their container through private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file


def exercise(session, root):
    original = hashlib.sha256((root / "journey.epub").read_bytes()).digest()
    session.observe("01-chapter-strip", ["journey.epub", "A Small Journey", "2 chapters"])
    _, y = session.word("01-chapter-strip", "journey.epub", minimum_y=240)
    # The two chapter cells fill the ordinary list's information strip.
    session.click(990, y + 45, delay=0.05)
    session.click(990, y + 45, delay=0.05)
    session.observe("02-clicked-arrival", ["Chapter 2 / 2", "Arrival", "harbor"])
    # Button centers verified in the fixed private viewport; OCR can omit button text.
    session.click(535, 200)
    session.observe("03-departure", ["Chapter 1 / 2", "Departure", "blue sea", "Evening"])
    with Image.open(session.output / "03-departure.png") as shot:
        blue = sum(b - r > 100 and b - g > 80 for r, g, b in shot.convert("RGB").getdata())
    assert blue > 15000, "Book illustration is not visible"
    session.click(670, 200)
    session.observe("04-arrival", ["Chapter 2 / 2", "harbor"])
    x, y = session.word("04-arrival", "journey.epub", tree=True, minimum_y=90)
    session.click(x + 10, y)
    session.observe("05-rename", ["Filename"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "renamed.epub"], env=session.env)
    session.key("Return")
    wait_for(lambda: (root / "renamed.epub").exists(), "book rename")
    assert hashlib.sha256((root / "renamed.epub").read_bytes()).digest() == original
    session.observe("06-renamed", ["renamed.epub", "harbor"])
    session.click(1110, 70)
    session.observe("07-folder", ["renamed.epub", "2 chapters", "notes.txt"])
    open_file(session, root, "renamed.epub", "08-reopen")
    session.observe("09-reopened", ["Departure", "blue sea"])
    session.click(1110, 70)
    session.observe("10-returned", ["renamed.epub", "notes.txt"])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    shutil.copyfile(repo / "crates/little-clip-core/tests/fixtures/ebooks/journey.epub", root / "journey.epub")
    (root / "notes.txt").write_text("Neighboring text file.\n")
    return run_application_acceptance(args, output, root, exercise, "chapter strip selection, local illustrations, reading order, rename and reopen")


if __name__ == "__main__":
    raise SystemExit(main())
