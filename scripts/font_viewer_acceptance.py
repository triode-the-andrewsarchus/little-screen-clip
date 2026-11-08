#!/usr/bin/env python3
"""Exercise interactive font specimens on private Weston/Niri."""
from pathlib import Path
import hashlib
import shutil
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file


def exercise(session, root):
    original = next(root.glob("specimen.*")).name
    renamed = "renamed" + Path(original).suffix
    original_hash = hashlib.sha256((root / original).read_bytes()).digest()
    open_file(session, root, original, "01-font-strip")
    session.observe("02-font-viewer", [original, "Back to folder", "Specimen size", "Character coverage"])
    x, y = session.word("02-font-viewer", "Specimen", tree=True)
    session.click(x + 120, y + 50)
    session.key("a", "ctrl")
    private.run_checked(["wtype", "Interactive typography 0123"], env=session.env)
    session.observe("03-custom-specimen", ["Interactive typography 0123"])
    # The character grid is anchored beneath its label; select Latin '!'.
    x, y = session.word("03-custom-specimen", "Character", tree=True, last=True)
    session.click(x + 72, y + 42)
    session.scroll_to_visible("04-character", ["Selected", "U+0021", "Copy character"], step=160)
    x, y = session.word("04-character", "character", tree=True, required=False, last=True) or session.word("04-character", "character", last=True)
    session.click(x, y)
    copied = subprocess.run(["wl-paste", "--no-newline"], env=session.env, capture_output=True, check=True).stdout
    assert copied == b"!", copied
    # Filename editing uses the same shared chrome as every other Files editor.
    x, y = session.word("02-font-viewer", original, tree=True, minimum_y=90)
    session.click(x + 10, y)
    session.observe("05-rename", ["Filename"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", renamed], env=session.env)
    session.key("Return")
    wait_for(lambda: (root / renamed).exists(), "font rename")
    assert hashlib.sha256((root / renamed).read_bytes()).digest() == original_hash
    session.observe("06-renamed", [renamed])
    session.click(1110, 70)
    session.observe("07-returned", ["Ctrl+L"])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager",
        lambda parser: parser.add_argument("--extension", choices=["ttf", "otf"], default="ttf"))
    output, root = media_fixture_directory(args)
    shutil.copyfile(repo / "crates/little-clip-render/assets/fonts/NotoSansCJK-JP-Regular.ttf", root / f"specimen.{args.extension}")
    return run_application_acceptance(args, output, root, exercise, "font strip, live specimen, glyph selection/copy and guarded rename")


if __name__ == "__main__":
    raise SystemExit(main())
