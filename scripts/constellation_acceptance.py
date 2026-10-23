#!/usr/bin/env python3
"""Exercise constellation indexing, search, slots, and versioned saves on private Weston/Niri."""
import json
import math
import sqlite3
import struct
import wave
from pathlib import Path

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)


def exercise(session, root):
    session.observe("folder", ["Constellation", "File"])
    session.click(*session.word("folder", "Constellation"))
    session.observe("empty-map", ["Sample Constellation Map", "Pack Shelf"])
    # The blue empty-state action is visibly centered here in the runner's
    # fixed 1280x800 viewport; Tesseract misses its reversed-color label.
    session.click(600, 480)
    database = session.output / "data/little-screen-clip/constellation.sqlite"
    def indexed():
        if not database.exists():
            return False
        with sqlite3.connect(database) as db:
            return db.execute("SELECT count(*) FROM samples").fetchone()[0] == 1
    wait_for(indexed, "owned sample indexing")
    session.observe("indexed-map", ["Pack Shelf", "Hover to audition"])
    session.key("f", "ctrl")
    from file_manager_acceptance import private
    private.run_checked(["wtype", "kick"], env=session.env)
    session.observe("search", ["Hover to audition"])
    session.key("Escape")
    session.observe("search-cleared", ["Hover to audition"])
    # Select the actual kick star, rather than the Random toolbar action. This
    # exercises the map's focus behavior before Shift+1 on a real Wayland keyboard.
    from PIL import Image
    image = Image.open(session.output / "search-cleared.png").convert("RGB")
    stars = [(x, y) for y in range(310, 715) for x in range(355, 840)
             if (lambda c: c[0] > 140 and c[0] > c[1] * 1.5 and c[0] > c[2] * 1.5)(image.getpixel((x, y)))]
    assert stars, "owned kick star must be visible in the map"
    session.click(sum(x for x, _ in stars) // len(stars), sum(y for _, y in stars) // len(stars))
    session.key("1", "shift")
    session.observe("assigned-slot", ["kick.wav"])
    session.key("2", "shift")
    for key in ["1", "1", "2", "1"]:
        session.key(key)
    session.observe("retriggered-slots", ["kick.wav"])
    session.click(885, 412)
    packs = root / "little-screen-clip-sample-packs"
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 1, "first atomic pack save")
    first = next(packs.glob("*/bank.json"))
    bank = json.loads(first.read_text())
    assert len(bank["slots"]) == 10
    assert len(bank["slots"][0]["samples"]) == 1
    assert len(bank["slots"][1]["samples"]) == 1
    assert bank["slots"][0]["samples"][0]["end_sec"] > 0
    marker = first.parent / "owned-preservation-marker.txt"
    marker.write_text("preserve this owned fixture")
    before = first.read_bytes()
    session.observe("first-save", ["Pack Shelf"])
    session.click(885, 412)
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 2, "versioned second pack save")
    assert first.read_bytes() == before
    assert marker.read_text() == "preserve this owned fixture"
    session.observe("second-save", ["Pack Shelf"])
    session.click(565, 264)
    session.observe("pack-browser", ["Sample Pack Browser", "Saved Packs", "Untitled"])
    session.key("Escape")
    session.address(root.parent)
    session.observe("navigated-away", ["fixture"])
    assert "POLLERR" not in (session.output / "files.log").read_text()


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    with wave.open(str(root / "kick.wav"), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(44100)
        wav.writeframes(b"".join(struct.pack("<h", int(20000 * math.sin(2 * math.pi * 60 * i / 44100) * math.exp(-i / 44100))) for i in range(88200)))

    def prepare_environment(env, _children):
        # Keep acceptance playback away from the user's audio device/server.
        config = output / "alsa-null.conf"
        config.write_text("pcm.!default { type null }\n")
        env["ALSA_CONFIG_PATH"] = str(config)

    return run_application_acceptance(args, output, root, exercise,
                                      "constellation indexing, slots, search, and non-destructive pack saves",
                                      prepare_environment)


if __name__ == "__main__":
    raise SystemExit(main())
