#!/usr/bin/env python3
"""Ordinary font/binary/text/image rows beside the protected-interface work."""
from pathlib import Path
import shutil
import subprocess

from PIL import Image

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)
from file_manager_phase4_acceptance import back, open_file, type_text


def exercise(session, root):
    session.observe("01-browser", ["Files", "4 items"])
    session.click(848, 201)
    session.observe("02-font", ["a-font.ttf", "Fira Sans", "glyphs", "weight 400"])
    session.observe("03-binary", ["b-program.elf", "Elf", "64-bit", "sections"])
    session.scroll_to_visible("04-text", ["c-source.rs", "120 lines"], step=150, attempts=6)
    open_file(session, root, "c-source.rs", "05-open-text")
    session.observe("06-text-viewer", ["c-source.rs", "ORDINARY_SOURCE", "Edit text"])
    back(session, "06-text-viewer")
    session.observe("07-return", ["Folders", "4 items"])
    session.scroll_to_visible("08-image", ["d-image.png", "120", "80"], step=150, attempts=6)
    x, y = session.word("08-image", "d-image.png")
    session.click(x + 10, y)
    captured = b"Saved captured clipboard content.\nsecond line\n"
    subprocess.run(["wl-copy", "--type", "text/plain"], input=captured, env=session.env,
                   check=True, timeout=5)
    session.key("v", "ctrl")
    session.observe("09-captured-name", ["Create from clipboard", "Captured text/plain"])
    session.key("a", "ctrl")
    type_text(session, "captured.txt")
    session.key("Return")
    destination = root / "captured.txt"
    wait_for(lambda: destination.exists(), "captured-content creation")
    assert destination.read_bytes() == captured
    session.observe("10-captured-created", ["5 items", "Create from clipboard"])
    # Focus an unobscured existing row; the success notice also contains the output path.
    session.observe("10b-captured-focus", ["c-source.rs"])
    x, y = session.word("10b-captured-focus", "c-source.rs")
    session.click(x + 10, y)
    subprocess.run(["wl-copy", "--type", "text/plain"], input=b"must not overwrite existing bytes\n",
                   env=session.env, check=True, timeout=5)
    session.key("v", "ctrl")
    session.observe("11-captured-collision", ["Create from clipboard", "Captured text/plain"])
    session.key("a", "ctrl")
    type_text(session, "captured.txt")
    session.key("Return")
    session.observe("12-collision-rejected", ["File exists"])
    assert destination.read_bytes() == captured
    assert not any(path.name.startswith(".little-clip-copy-") for path in root.iterdir())
    session.key("Escape")
    session.observe("13-collision-return", ["Folders", "5 items"])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    shutil.copyfile(repo / "crates/clip-workbench/assets/fonts/FiraSans-Regular.ttf", root / "a-font.ttf")
    shutil.copyfile("/usr/bin/true", root / "b-program.elf")
    (root / "c-source.rs").write_text('fn ordinary() { println!("ORDINARY_SOURCE"); }\n' * 120)
    Image.new("RGB", (120, 80), (25, 170, 220)).save(root / "d-image.png")
    return run_application_acceptance(args, output, root, exercise,
                                      "ordinary font/binary/text/image preview regression")


if __name__ == "__main__":
    raise SystemExit(main())
