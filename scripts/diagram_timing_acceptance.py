#!/usr/bin/env python3
"""Exercise timing painting, lossless saves and undo on the private desktop."""
from pathlib import Path

from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    run_application_acceptance,
    wait_for,
)

SOURCE = "{\n // Keep this comment\n signal:[{name:'request',wave:'0.....'},{name:'clock',wave:'p.....'}],\n extra:'retained',\n}\n"


def exercise(session, root):
    session.observe("01-folder", ["a-timing.json5"])
    x, y = session.word("01-folder", "a-timing.json5")
    session.click(x + 20, y)
    session.key("Return")
    session.observe("02-painter", ["Signal painter", "Signal name", "source cells"])
    # First signal cell, verified in the runner's fixed 1280x800 native window.
    # Headless pointer tests also cover fast drags and release outside the lane.
    session.click(790, 402)
    session.key("s", "ctrl")
    expected = SOURCE.replace("wave:'0.....'", "wave:'10....'")
    path = root / "a-timing.json5"
    wait_for(lambda: path.read_text() == expected, "painted cell and lossless guarded save")
    session.observe("03-painted", ["Signal painter", "Signal name"])
    session.key("z", "ctrl")
    session.key("s", "ctrl")
    wait_for(lambda: path.read_text() == SOURCE, "canvas undo restores exact source")
    session.key("Escape")
    session.observe("04-folder-return", ["a-timing.json5"])


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-timing.json5").write_text(SOURCE)
    return run_application_acceptance(
        args, output, root, exercise, "native WaveJSON timing painter"
    )


if __name__ == "__main__":
    raise SystemExit(main())
