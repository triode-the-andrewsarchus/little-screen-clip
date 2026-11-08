#!/usr/bin/env python3
"""Explore saved notebook cells through private Weston/Niri, without running a kernel."""
from pathlib import Path
import hashlib
import shutil
import subprocess
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    original = hashlib.sha256((root / "sample.ipynb").read_bytes()).digest()
    session.observe("01-cell-map", ["sample.ipynb", "5 cells", "3 code"])
    # Double-click the second of five semantic cells in its visible file row.
    _, y = session.word("01-cell-map", "sample.ipynb", minimum_y=240)
    session.click(620, y + 45, delay=0.05)
    session.click(620, y + 45, delay=0.05)
    session.observe("02-selected-code", ["Code cell 2", "Saved execution [2]", "values", "Saved outputs"])
    # Copy the selected source before screenshots can replace the private clipboard.
    click_word(session, "02-selected-code", "Copy", minimum_y=280)
    copied = subprocess.run(["wl-paste", "--no-newline"], env=session.env, capture_output=True, check=True).stdout
    assert copied == b"values = [1, 4, 9]\nprint(values)\n", copied
    session.scroll_to_visible("02-saved-chart", ["Saved chart"], step=160)
    with Image.open(session.output / "02-saved-chart.png") as shot:
        blue = sum(b - r > 80 and b - g > 60 for r, g, b in shot.convert("RGB").getdata())
    assert blue > 1000, "Saved PNG chart was not visibly rendered"
    click_word(session, "02-selected-code", "Next", minimum_y=200, maximum_y=310)
    session.observe("03-saved-error", ["Code cell 3", "Saved error", "ValueError", "example"])
    # Choose the first cell directly through the shared overview map.
    session.click(180, 220)
    session.observe("04-markdown", ["Markdown cell 1", "Notebook sample", "Rendered", "Source"])
    click_word(session, "04-markdown", "Source", last=True, minimum_y=300)
    session.observe("05-markdown-source", ["# Notebook sample", "saved experiment"])
    click_word(session, "05-markdown-source", "Notebook", last=True, minimum_y=450)
    session.observe("05-notebook-metadata", ["Notebook fixture", "python", "language_info"])
    x, y = session.word("05-notebook-metadata", "sample.ipynb", tree=True, minimum_y=90)
    session.click(x + 10, y)
    session.observe("06-rename", ["Filename"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "renamed.ipynb"], env=session.env)
    session.key("Return")
    wait_for(lambda: (root / "renamed.ipynb").exists(), "notebook rename")
    assert hashlib.sha256((root / "renamed.ipynb").read_bytes()).digest() == original
    session.observe("07-renamed", ["renamed.ipynb", "Markdown cell 1"])
    session.click(1110, 70)
    session.observe("08-returned", ["renamed.ipynb", "5 cells"])
    open_file(session, root, "renamed.ipynb", "09-reopen")
    session.observe("10-reopened", ["Markdown cell 1", "Notebook sample"])
    session.click(1110, 70)
    session.observe("11-folder", ["renamed.ipynb", "notes.txt"])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    shutil.copyfile(repo / "crates/clip-workbench/tests/fixtures/notebooks/sample.ipynb", root / "sample.ipynb")
    (root / "notes.txt").write_text("An ordinary neighboring text file.\n")
    return run_application_acceptance(args, output, root, exercise, "semantic notebook strip, clicked cell, source copy, saved error, Markdown, rename and reopen")


if __name__ == "__main__":
    raise SystemExit(main())
