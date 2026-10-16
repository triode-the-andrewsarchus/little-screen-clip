#!/usr/bin/env python3
"""Clickable List sorting headers on the private Weston/Niri compositor."""
import os
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def ordered(session, shot, names):
    session.observe(shot, ["Type", "Name", "Size", "Modified", *names])
    positions = [session.word(shot, name, minimum_y=240)[1] for name in names]
    assert positions == sorted(positions), f"{shot}: wrong filename order: {positions}"


def sort_header(session, shot, label):
    # Cache each inactive heading's position: its active arrow can join the OCR
    # word even though the heading and click target remain in the same place.
    positions = getattr(session, "sort_header_positions", {})
    if label not in positions:
        positions[label] = session.word(shot, label, minimum_y=190)
    session.sort_header_positions = positions
    session.click(*positions[label])


def exercise(session, root):
    ordered(session, "name-ascending", ["Alpha.txt", "Beta.txt", "Gamma.txt"])
    sort_header(session, "name-ascending", "Size")
    ordered(session, "size-ascending", ["Beta.txt", "Gamma.txt", "Alpha.txt"])
    sort_header(session, "size-ascending", "Size")
    ordered(session, "size-descending", ["Alpha.txt", "Gamma.txt", "Beta.txt"])
    sort_header(session, "size-descending", "Modified")
    ordered(session, "newest-first", ["Alpha.txt", "Gamma.txt", "Beta.txt"])
    sort_header(session, "newest-first", "Modified")
    ordered(session, "oldest-first", ["Beta.txt", "Gamma.txt", "Alpha.txt"])
    sort_header(session, "oldest-first", "Name")
    ordered(session, "name-restored", ["Alpha.txt", "Beta.txt", "Gamma.txt"])
    sort_header(session, "name-restored", "Name")
    ordered(session, "name-descending", ["Gamma.txt", "Beta.txt", "Alpha.txt"])
    # Header clicks hand focus back to the browser for typing and arrow keys.
    private.run_checked(["wtype", "Beta"], env=session.env)
    session.observe("type-after-sort", ["Find: Beta", "Selection: 1 file"])

    session.address(root.parent / "types")
    session.observe("types-start", ["Type", "Name", "Size", "Modified"])
    sort_header(session, "types-start", "Type")
    ordered(session, "type-groups", ["Delta.mp3", "Beta.wav", "Alpha.jpg"])
    sort_header(session, "type-groups", "Type")
    ordered(session, "type-groups-reversed", ["Gamma.png", "Alpha.jpg", "Beta.wav"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    for name, size, age in [("Alpha.txt", 90, 3), ("Beta.txt", 30, 1), ("Gamma.txt", 60, 2)]:
        path = root / name
        path.write_text("x" * size)
        timestamp = 1_600_000_000 + age * 60
        os.utime(path, (timestamp, timestamp))
    types = root.parent / "types"
    types.mkdir()
    for name in ["Alpha.jpg", "Beta.wav", "Gamma.png", "Delta.mp3"]:
        (types / name).write_bytes(b"preview failure must still sort")
    return run_application_acceptance(args, output, root, exercise, "Natural name, category/extension, size and modified header sorting")


if __name__ == "__main__":
    raise SystemExit(main())
