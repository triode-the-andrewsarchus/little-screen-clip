#!/usr/bin/env python3
"""Review grouped audio metadata columns on the private native compositor."""
from pathlib import Path
import subprocess
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    for name, artist, album, genre, duration in [
        ("amber.wav", "Zulu", "Second", "Jazz", 1),
        ("birch.wav", "Alpha", "First", "Rock", 2),
        ("cedar.wav", "", "Third", "Ambient", 3),
    ]:
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"sine=frequency=220:duration={duration}",
            "-metadata", f"artist={artist}", "-metadata", f"album={album}", "-metadata", f"genre={genre}", str(root / name)], check=True, timeout=10)
    Image.new("RGB", (40, 30), (80, 120, 160)).save(root / "picture.png")


def ordered(session, name, files):
    session.observe(name, ["Audio", "Artist", "Album", "Genre", "Duration", "3/3", *files])
    positions = [session.word(name, file, minimum_y=300)[1] for file in files]
    assert positions == sorted(positions), (name, positions)


def exercise(session, root):
    session.observe("initial", ["List", "Music"])
    session.click(118, 160)  # Select List even when the fixture opens in Music.
    text = session.observe("list-start", ["Type", "Name", "Size"])
    if "Hands" in text:
        session.click(645, 160)  # Collapse Shelf on this fixed private output.
    session.click(*session.word("list-start", "Type", minimum_y=190))
    session.observe("loaded-tags", ["Audio", "Artist", "Album", "Genre", "Duration", "Zulu", "Alpha", "Jazz", "Rock", "3/3"])
    positions = {field: session.word("loaded-tags", field, minimum_y=260) for field in ["Artist", "Album", "Genre", "Duration"]}
    session.click(*positions["Artist"])
    ordered(session, "artist-ascending", ["birch.wav", "amber.wav", "cedar.wav"])
    session.click(*positions["Artist"])
    ordered(session, "artist-descending", ["amber.wav", "birch.wav", "cedar.wav"])
    session.click(*positions["Album"])
    ordered(session, "album-ascending", ["birch.wav", "amber.wav", "cedar.wav"])
    session.click(*positions["Genre"])
    ordered(session, "genre-ascending", ["cedar.wav", "amber.wav", "birch.wav"])
    session.click(*positions["Duration"])
    ordered(session, "duration-ascending", ["amber.wav", "birch.wav", "cedar.wav"])
    session.click(*positions["Duration"])
    ordered(session, "duration-descending", ["cedar.wav", "birch.wav", "amber.wav"])
    session.click(*session.word("list-start", "Type", minimum_y=190))
    session.observe("images-use-image-columns", ["Images", "picture.png", "Width", "Height", "Audio", "Artist"])


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Grouped artist/album/genre/duration columns, reverse sorting, missing tags and mixed-family headers"))
