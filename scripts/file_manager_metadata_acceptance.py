#!/usr/bin/env python3
"""Review sticky family headers and the single-family header on a private compositor."""
from pathlib import Path
import shutil
import subprocess
import tempfile
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_columns_acceptance import resize


def fixtures(root):
    single = root / "audio-only"
    single.mkdir()
    with tempfile.TemporaryDirectory() as scratch:
        sources = []
        for index, artist in enumerate(["Alpha", "Zulu", "Birch"]):
            source = Path(scratch) / f"source-{index}.wav"
            subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"sine=frequency=220:duration={0.2 * (index + 1)}",
                "-metadata", f"artist={artist}", "-metadata", "album=Preview", "-metadata", "genre=Jazz", str(source)], check=True, timeout=10)
            sources.append(source)
        for index in range(20):
            for folder in [root, single]:
                shutil.copyfile(sources[index % 3], folder / f"track-{index:02}.wav")
            Image.new("RGB", (40, 30), (80, 120, 160)).save(root / f"picture-{index:02}.png")


def exercise(session, root):
    resize(session, 1120, 760)
    session.observe("initial", ["List", "Columns", "Type"])
    session.click(600, 160)  # Collapse Shelf on the observed browser toolbar.
    session.observe("list-start", ["Type", "Name", "Size", "Modified"])
    session.click(*session.word("list-start", "Type", minimum_y=190))
    session.observe("grouped", ["Audio", "Artist", "Album", "Duration", "Size", "Modified"])
    session.scroll(1050, 550, 900)
    session.observe("pinned-audio", ["Audio", "Artist", "Album", "Genre", "Duration", "Size", "Modified"])
    _, first_y = session.word("pinned-audio", "Artist", minimum_y=200)
    session.scroll(1050, 550, 500)
    session.observe("pinned-audio-deeper", ["Audio", "Artist", "Duration", "Size", "Modified"])
    _, next_y = session.word("pinned-audio-deeper", "Artist", minimum_y=200)
    assert abs(first_y - next_y) < 3, "Family headings must stay pinned while scrolling their rows"
    x, y = session.word("pinned-audio-deeper", "Duration", minimum_y=200)
    session.click(x + 8, y)
    session.observe("pinned-duration-sort", ["Audio", "Duration", "Size", "Modified"])
    for _ in range(6):
        session.scroll(1050, 550, 900)
    session.observe("pinned-images", ["Images", "Width", "Height", "Megapixels", "Size", "Modified"])
    session.address(root / "audio-only")
    session.observe("single-family", ["Audio", "Artist", "Album", "Genre", "Duration", "Size", "Modified"])
    assert session.word("single-family", "Type", required=False, minimum_y=190) is None
    session.scroll(1050, 550, 900)
    session.observe("single-family-scrolled", ["Audio", "Duration", "Size", "Modified"])
    x, y = session.word("single-family-scrolled", "Duration", minimum_y=190)
    session.click(x + 8, y)
    session.observe("single-family-sort", ["Audio", "Duration", "Size", "Modified"])
    resize(session, 1560, 900)
    session.observe("wide-list", ["Columns", "Audio", "Duration"])
    x, y = session.word("wide-list", "Columns", tree=True, minimum_y=100)
    session.click(x + 20, y)
    session.observe("single-family-columns", ["Audio", "Artist", "Duration", "Size", "Modified", "picture-00.png"])
    assert session.word("single-family-columns", "Type", required=False, minimum_y=190) is None
    session.key("q", "ctrl")


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    args.output_size = (1760, 1000)
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Pinned family headers, live duration sorting after scrolling, common columns and a single header for homogeneous folders"))
