#!/usr/bin/env python3
"""Verify multisample pitch rings and rack halos on the private Weston/Niri runner."""
import math
import sqlite3
import struct
import subprocess
import wave
from pathlib import Path

from PIL import Image
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private,
)


def assert_sample_at(image, point):
    x, y = point
    patch = image.crop((x - 3, y - 3, x + 4, y + 4))
    assert any(max(pixel) - min(pixel) > 90 for pixel in patch.get_flattened_data()), "the original set moved"


def exercise(session, root):
    session.observe("folder", ["Constellation"])
    session.click(*session.word("folder", "Constellation"))
    session.observe("empty-map", ["Index folder"])
    session.click(*(session.word("empty-map", "Index", required=False) or session.word("empty-map", "Indexfolder", required=False) or (580, 558)))
    database = session.output / "data/little-screen-clip/constellation.sqlite"

    def indexed():
        if not database.exists():
            return False
        with sqlite3.connect(database) as db:
            return db.execute("SELECT count(*) FROM samples").fetchone()[0] == 3

    wait_for(indexed, "three recorded piano pitches")
    session.observe("indexed", ["Index complete", "Pitch height", "All pitches", "Multisamples only"])
    multisamples_button = session.word("indexed", "Multisamples")
    session.click(*multisamples_button)
    session.observe("multisamples-only", ["3 / 3 samples"])
    session.click(*multisamples_button)
    session.click(*(session.word("indexed", "Camera", required=False) or (815, 214)))
    session.observe("camera-controls", ["Azimuth", "Elevation", "Top view", "Oblique view", "Reset view"])
    session.key("Escape")
    # Reveal the entire canvas without changing projection or touching the host compositor.
    session.click(1164, 550)
    session.observe("overview", ["Size: duration"])
    private.run_checked([str(session.pointer), "right-drag", "1280", "800", "760", "560", "880", "620"], env=session.env)
    session.observe("orbited-plane", ["Size: duration"])
    before = Image.open(session.output / "overview.png").convert("RGB")
    after = Image.open(session.output / "orbited-plane.png").convert("RGB")
    assert before.crop((370, 450, 1130, 710)).tobytes() != after.crop((370, 450, 1130, 710)).tobytes(), "right-drag did not rotate the plane"
    session.click(*(session.word("orbited-plane", "Camera", required=False) or (815, 214)))
    session.observe("camera-reset-menu", ["Reset view"])
    session.click(*(session.word("camera-reset-menu", "Reset", required=False) or session.word("camera-reset-menu", "Resetview")))
    session.key("Escape")
    session.observe("reset-overview", ["Size: duration"])
    image = Image.open(session.output / "reset-overview.png").convert("RGB")
    # There is one folded family. Select its vivid central disk, away from chrome/legend.
    stars = [(x, y) for y in range(450, 710) for x in range(370, 1130)
             if max(image.getpixel((x, y))) > 130
             and max(image.getpixel((x, y))) - min(image.getpixel((x, y))) > 90]
    assert stars, "the folded multisample family must be visible"
    point = tuple(sum(p[axis] for p in stars) // len(stars) for axis in (0, 1))
    hover = subprocess.Popen([str(session.pointer), "move", "1280", "800", *map(str, point)],
                             env=session.env, stdout=subprocess.DEVNULL)
    try:
        session.observe("middle-note-hover", ["old_piano_C4.wav", "3 recordings"], preserve_pointer=True)
    finally:
        hover.terminate()
        hover.wait(timeout=3)
    session.click(*point)
    session.observe("set-selected", ["Explore set", "3 notes"])
    # Selection must retain the folded set's disk at its original pixel position.
    selected = Image.open(session.output / "set-selected.png").convert("RGB")
    assert_sample_at(selected, point)
    session.click(*(session.word("set-selected", "Explore", required=False) or (832, 292)))
    session.observe("pitch-rings", ["3 notes", "Back to Map"])
    # Explore selects the lowest recording. Assign it, then capture detail and overview halos.
    session.key("z", "shift")
    session.observe("collected-pitch-rings", ["3 notes"])
    session.click(*(session.word("collected-pitch-rings", "Back", required=False) or (400, 297)))
    session.observe("collected-overview", ["Explore set"])
    restored = Image.open(session.output / "collected-overview.png").convert("RGB")
    assert_sample_at(restored, point)
    session.click(1164, 550)
    session.observe("rack-and-glow", ["old_piano_C3.wav", "Packs"])
    # Undo the assignment so the private app closes without a dirty-document prompt.
    session.key("z", "ctrl")
    session.observe("assignment-undone", ["Packs"])
    # Reload the cached snapshot through folder navigation; families must survive without reindexing.
    empty = root / "empty"
    empty.mkdir()
    session.address(empty)
    session.observe("empty-folder", ["Empty folder"])
    session.address(root)
    session.observe("returned-folder", ["Constellation"])
    session.click(*session.word("returned-folder", "Constellation"))
    session.observe("cached-map", ["All pitches", "3 / 3 samples"])
    # The pitch control is a second two-ended range; narrow it to the lower octave.
    session.observe("pitch-filter-ready", ["All pitches"])
    label = session.word("pitch-filter-ready", "Pitch", minimum_y=230, required=False) or (450, 244)
    high = (label[0] + 195, label[1])
    middle = (label[0] + 123, label[1])
    private.run_checked([str(session.pointer), "drag", "1280", "800", *map(str, (*high, *middle))], env=session.env)
    session.observe("pitch-filtered", ["2 / 3 samples"])
    session.click(*middle, delay=0.05)
    session.click(*middle, delay=0.05)
    session.observe("pitch-reset", ["3 / 3 samples", "All pitches"])



def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    for note, frequency, duration in [("C3", 130.8128, 0.1), ("C4", 261.6256, 0.7), ("C5", 523.2511, 2.0)]:
        with wave.open(str(root / f"old_piano_{note}.wav"), "wb") as wav:
            wav.setnchannels(1)
            wav.setsampwidth(2)
            wav.setframerate(44100)
            wav.writeframes(b"".join(struct.pack("<h", int(18000 * math.sin(2 * math.pi * frequency * i / 44100)
                                 * min(i / 300, 1.0) * math.exp(-i / 22050)))
                                     for i in range(int(44100 * duration))))

    def prepare_environment(env, _children):
        config = output / "alsa-null.conf"
        config.write_text("pcm.!default { type null }\n")
        env["ALSA_CONFIG_PATH"] = str(config)

    return run_application_acceptance(args, output, root, exercise,
        "pitch height, duration size, folded middle-note audition, pitch rings and collected halos", prepare_environment)


if __name__ == "__main__":
    raise SystemExit(main())
