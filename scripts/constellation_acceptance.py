#!/usr/bin/env python3
"""Exercise constellation indexing, search, slots, and versioned saves on private Weston/Niri."""
import json
import math
import sqlite3
import struct
import subprocess
import time
import wave
from pathlib import Path

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)


def pad_gestures(session, root, destination, red):
    """Check actual grid swaps, mute-link gestures and the studio round trip."""
    from file_manager_acceptance import private
    from PIL import Image
    # The rack width is unchanged: adjacent pad centers remain 87 logical pixels apart.
    empty = (destination[0] + 174, destination[1])
    def swap(source, target):
        private.run_checked([str(session.pointer), "drag", "1280", "800", *map(str, (*source, *target))], env=session.env)
    swap(destination, empty)
    session.observe("pad-swapped", ["Packs"])
    image = Image.open(session.output / "pad-swapped.png").convert("RGB")
    def line_near(point):
        return any(red(image.getpixel((x, y))) for x in range(point[0] - 25, point[0] + 25) for y in range(point[1] + 6, point[1] + 14))
    assert not line_near(destination) and line_near(empty), "drag must move the instrument to the empty pad"
    swap(empty, destination)
    session.observe("pad-swap-restored", ["Packs"])
    modifier = subprocess.Popen(["wtype", "-M", "shift", "-s", "1800", "-m", "shift"], env=session.env)
    try:
        time.sleep(0.15)
        for x, y in [destination, (destination[0] + 87, destination[1])]:
            private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x), str(y)], env=session.env)
            time.sleep(0.15)
    finally:
        modifier.wait(timeout=4)
    hover = subprocess.Popen([str(session.pointer), "move", "1280", "800", *map(str, destination)], env=session.env, stdout=subprocess.DEVNULL)
    try:
        session.observe("mute-group-hover", ["Packs"], preserve_pointer=True)
    finally:
        hover.terminate()
        hover.wait(timeout=3)
    original = (root / "kick.wav").read_bytes()
    session.click(*destination, delay=0.05)
    session.click(*destination, delay=0.1)
    session.observe("pad-audio-studio", ["AUDIO STUDIO", "Selection info"])
    session.click(*(session.word("pad-audio-studio", "Back", tree=True, required=False) or (150, 118)))
    session.observe("pad-studio-close", ["Save an edited copy", "replace pad"])
    session.click(580, 457)  # Center of the confirmed modal save button.
    wait_for(lambda: bool(list(root.glob("kick.edited*.wav"))), "published edited sibling")
    session.observe("pad-studio-return", ["Packs"])
    assert (root / "kick.wav").read_bytes() == original


def exercise(session, root, hardware_keys=False):
    from file_manager_acceptance import private
    from PIL import Image
    session.observe("folder", ["Constellation", "File"])
    session.click(*session.word("folder", "Constellation"))
    text = session.observe("empty-map", ["Sample constellation", "Packs", "Mono samples only"])
    assert "MIDI input" not in text and "Layer actions" not in text
    session.click(*(session.word("empty-map", "Index", required=False) or (580, 578)))
    database = session.output / "data/little-screen-clip/constellation.sqlite"
    def indexed():
        if not database.exists():
            return False
        with sqlite3.connect(database) as db:
            return db.execute("SELECT count(*) FROM samples").fetchone()[0] == 1
    wait_for(indexed, "owned sample indexing")
    session.observe("indexed-map", ["Index complete"])
    session.key("f", "ctrl")
    private.run_checked(["wtype", "kick"], env=session.env)
    session.observe("search", ["Packs"])
    session.key("Escape")
    session.observe("search-cleared", ["Packs"])
    image = Image.open(session.output / "search-cleared.png").convert("RGB")
    red = lambda color: color[0] > 140 and color[0] > color[1] * 1.5 and color[0] > color[2] * 1.5
    stars = [(x, y) for y in range(390, 710) for x in range(355, 800) if red(image.getpixel((x, y)))]
    assert stars
    session.click(*(sum(point[axis] for point in stars) // len(stars) for axis in (0, 1)))
    first_key = "semicolon" if hardware_keys else "z"
    session.key(first_key, "shift")
    text = session.observe("assigned-slot", ["Sound", "Trim", "Start", "End", "Gain", "Pan"])
    assert "Layer actions" not in text and "Fit selection" not in text
    session.key("Space")
    session.key("Space")
    # Duplicate keyboard assignments must be confirmable and cancellable.
    session.key(first_key, "shift")
    session.observe("duplicate-cancel", ["Duplicate sample layers", "Add anyway", "Cancel"])
    session.click(*(session.word("duplicate-cancel", "Cancel", required=False) or (632, 488)))
    session.key(first_key, "shift")
    session.observe("duplicate-add", ["Duplicate sample layers", "Add anyway"])
    session.click(*(session.word("duplicate-add", "anyway", required=False) or (535, 488)))
    session.observe("two-layers", ["Layer 1", "Layer 2", "Sound"])
    session.key("q" if hardware_keys else "x", "shift")
    for key in [first_key, first_key, "q" if hardware_keys else "x"]:
        session.key(key)
    session.observe("retriggered-slots", ["Packs"])
    session.click(*(session.word("retriggered-slots", "Editor", required=False) or (1094, 470)))
    session.observe("map-for-drag", ["Packs"])
    image = Image.open(session.output / "map-for-drag.png").convert("RGB")
    pad_line = [(x, y) for y in range(500, 720) for x in range(822, 902) if red(image.getpixel((x, y)))]
    assert pad_line
    bottom = max(y for _, y in pad_line)
    edge = [(x, y) for x, y in pad_line if y >= bottom - 2]
    destination = (sum(x for x, _ in edge) // len(edge), bottom - 12)
    pad_gestures(session, root, destination, red)
    # Collapse and reopen the sidebar; editor visibility must survive both actions.
    session.observe("before-collapse", ["Packs"])
    header = session.word("before-collapse", "Packs", minimum_y=300)
    session.click(1150, header[1])
    text = session.observe("collapsed-rack", ["Mono samples only"])
    assert "Packs" not in text
    session.click(1164, 550)
    text = session.observe("reopened-rack", ["Packs"])
    assert "Sound" not in text, "a previously closed editor stays closed"
    session.click(*(session.word("reopened-rack", "Editor", required=False) or (1094, 470)))
    session.observe("editor-open-before-collapse", ["Sound", "Trim"])
    header = session.word("editor-open-before-collapse", "Packs", minimum_y=300)
    session.click(1150, header[1])
    text = session.observe("editor-collapsed", ["Mono samples only"])
    assert "Sound" not in text
    session.click(1164, 550)
    session.observe("editor-reopened", ["Sound", "Gain", "Pan"])
    session.click(*session.word("editor-reopened", "Output"))
    session.observe("visual-compressor", ["Compressor", "Threshold", "Ratio", "Knee", "Makeup"])
    session.click(*session.word("visual-compressor", "Rack", minimum_y=300))
    session.observe("rack-return", ["Override MIDI velocity"])
    session.click(*session.word("rack-return", "Override"))
    session.observe("before-save", ["Packs"])
    session.click(*(session.word("before-save", "Save", required=False) or (1109, 500)))
    packs = root / "little-screen-clip-sample-packs"
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 1, "automatic portable pack save")
    first = next(packs.glob("*/bank.json"))
    bank = json.loads(first.read_text())
    assert len(bank["slots"]) == 16
    assert len(bank["slots"][0]["samples"]) == 2
    assert len(bank["slots"][1]["samples"]) == 1
    assert bank["slots"][0]["choke_group"] == bank["slots"][1]["choke_group"] > 0
    assert bank["override_velocity"]
    for slot in bank["slots"]:
        for layer in slot["samples"]:
            assert not Path(layer["file_path"]).is_absolute()
            assert not (first.parent / layer["file_path"]).is_symlink()
            assert (first.parent / layer["file_path"]).is_file()
    marker = first.parent / "preserve.txt"
    marker.write_text("owned marker")
    before = first.read_bytes()
    session.observe("first-save", ["Packs"])
    session.click(*(session.word("first-save", "Save", required=False) or (1109, 500)))
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 2, "second atomic version")
    assert first.read_bytes() == before and marker.read_text() == "owned marker"
    session.observe("second-save", ["Saved"])
    session.click(466, 533)  # Select Layer 2, then its adjacent remove control.
    session.click(519, 533)
    text = session.observe("layer-removed", ["Layer 1", "Sound", "Unsaved changes"])
    assert "Layer 2" not in text
    session.click(978, 470)  # Button center; OCR can include its left border.
    session.observe("layer-restored", ["Layer 1", "Layer 2"])
    session.click(110, 70)
    session.observe("file-menu", ["constellation", "recursive"])
    session.click(*(session.word("file-menu", "Re-index", tree=True, required=False) or (155, 130)))
    session.observe("reused-index", ["1 reused", "1 analyzed"])
    session.click(*(session.word("reused-index", "Mono", required=False) or (1060, 222)))
    session.observe("mono-filter", ["2 / 2 samples"])
    session.click(*session.word("mono-filter", "Packs", minimum_y=300))
    session.observe("pack-browser", ["Saved packs", "Untitled"])
    session.key("Escape")
    session.observe("rack-before-close", ["Packs"])
    session.click(*(session.word("rack-before-close", "Override", required=False) or (944, 534)))
    session.key("q", "ctrl")
    session.observe("close-guard", ["This sample pack has unsaved changes", "Discard changes"])
    session.click(616, 112)  # Center of Discard changes in the close guard.
    session.process.wait(timeout=5)
    assert session.process.returncode == 0
    assert "POLLERR" not in (session.output / "files.log").read_text()


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager",
                                 lambda parser: parser.add_argument("--hardware-keys", action="store_true"))
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
        if args.hardware_keys:
            config = output / "scratch/config.toml"
            with config.open("a") as file:
                file.write('\n[file_manager]\nconstellation_pad_keys = [";qjk", "aoeu", "\',.p", "1234"]\n')

    return run_application_acceptance(args, output, root, lambda session, root: exercise(session, root, args.hardware_keys),
                                      "constellation controls, pad gestures, studio return, portable saves and close protection",
                                      prepare_environment)


if __name__ == "__main__":
    raise SystemExit(main())
