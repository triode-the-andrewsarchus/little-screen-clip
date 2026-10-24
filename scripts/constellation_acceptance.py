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
    session.observe("folder", ["Constellation", "File"])
    session.click(*session.word("folder", "Constellation"))
    session.observe("empty-map", ["Sample constellation", "Rack", "Override velocity"])
    session.click(*(session.word("empty-map", "Index", required=False) or (580, 540)))
    database = session.output / "data/little-screen-clip/constellation.sqlite"
    def indexed():
        if not database.exists():
            return False
        with sqlite3.connect(database) as db:
            return db.execute("SELECT count(*) FROM samples").fetchone()[0] == 1
    wait_for(indexed, "owned sample indexing")
    session.observe("indexed-map", ["Rack", "Packs"])
    session.key("f", "ctrl")
    from file_manager_acceptance import private
    private.run_checked(["wtype", "kick"], env=session.env)
    session.observe("search", ["Rack"])
    session.key("Escape")
    session.observe("search-cleared", ["Rack"])
    # Select the actual kick star, rather than the Random toolbar action. This
    # exercises the map's focus behavior before Shift+1 on a real Wayland keyboard.
    from PIL import Image
    image = Image.open(session.output / "search-cleared.png").convert("RGB")
    stars = [(x, y) for y in range(400, 680) for x in range(355, 840)
             if (lambda c: c[0] > 140 and c[0] > c[1] * 1.5 and c[0] > c[2] * 1.5)(image.getpixel((x, y)))]
    assert stars, "owned kick star must be visible in the map"
    session.click(sum(x for x, _ in stars) // len(stars), sum(y for _, y in stars) // len(stars))
    session.key("semicolon" if hardware_keys else "z", "shift")
    session.observe("assigned-slot", ["Mute", "Solo", "Gain", "Pan", "Attack", "Release"])
    session.key("q" if hardware_keys else "x", "shift")
    if hardware_keys:
        session.key("apostrophe", "shift")
    for key in (["semicolon", "semicolon", "q", "apostrophe"] if hardware_keys else ["z", "z", "x", "z"]):
        session.key(key)
    session.observe("retriggered-slots", ["Packs"])
    # Docks obscure the anchored map. Hide the editor to expose the drag source.
    session.click(*(session.word("retriggered-slots", "Editor", required=False) or (1115, 430)))
    session.observe("map-for-drag", ["Packs"])
    # Exercise real Copy-only Wayland map drags, including Shift layering.
    image = Image.open(session.output / "map-for-drag.png").convert("RGB")
    editor_y = 680
    red = lambda c: c[0] > 140 and c[0] > c[1] * 1.5 and c[0] > c[2] * 1.5
    star = [(x, y) for y in range(400, editor_y - 20) for x in range(355, 805) if red(image.getpixel((x, y)))]
    pad_line = [(x, y) for y in range(550, 695) for x in range(822, 904) if red(image.getpixel((x, y)))]
    assert star and pad_line, "owned map star and first-pad category line must be visible"
    source = tuple(sum(point[axis] for point in star) // len(star) for axis in (0, 1))
    bottom = max(y for _, y in pad_line)
    pad_line = [(x, y) for x, y in pad_line if y >= bottom - 2]
    destination = (sum(x for x, _ in pad_line) // len(pad_line), bottom - 10)
    def drag():
        private.run_checked([str(session.pointer), "drag", "1280", "800", *map(str, (*source, *destination))], env=session.env)
        time.sleep(0.4)
    drag()
    session.observe("replace-drop", ["Packs"])
    modifier = subprocess.Popen(["wtype", "-M", "shift", "-s", "2500", "-m", "shift"], env=session.env)
    try:
        time.sleep(0.15)
        drag()
    finally:
        modifier.wait(timeout=4)
    session.observe("layer-drop", ["Packs"])
    assert (root / "kick.wav").is_file(), "pad assignment must preserve its source"
    pad_gestures(session, root, destination, red)
    session.click(*(session.word("layer-drop", "Override", required=False) or (500, 708)))
    session.observe("velocity-override", ["Packs"])
    session.click(740, 714)  # Footer bypass; avoid similarly named output controls.
    session.observe("performance-controls", ["Packs"])
    session.click(*session.word("performance-controls", "Output"))
    session.observe("visual-compressor", ["Preview compressor", "Threshold", "Ratio"])
    session.click(880, 379)  # Rack tab, rather than the toolbar’s Hide rack.
    session.observe("performance-controls", ["Packs"])
    session.click(*(session.word("performance-controls", "Save", required=False) or (1038, 472)))
    packs = root / "little-screen-clip-sample-packs"
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 1, "first atomic pack save")
    first = next(packs.glob("*/bank.json"))
    bank = json.loads(first.read_text())
    assert len(bank["slots"]) == 16
    assert {i for i, slot in enumerate(bank["slots"]) if slot["samples"]} == ({0, 1, 8} if hardware_keys else {0, 1})
    assert len(bank["slots"][0]["samples"]) == 2
    assert bank["slots"][0]["choke_group"] == bank["slots"][1]["choke_group"] > 0
    assert ".edited" in bank["slots"][0]["samples"][0]["filename"]
    assert len(bank["slots"][1]["samples"]) == 1
    assert bank["slots"][0]["samples"][0]["end_sec"] > 0
    assert bank["override_velocity"] and bank["compressor_bypass"]
    if hardware_keys:
        assert len(bank["slots"][8]["samples"]) == 1
    marker = first.parent / "owned-preservation-marker.txt"
    marker.write_text("preserve this owned fixture")
    before = first.read_bytes()
    session.observe("first-save", ["Packs"])
    session.click(*(session.word("first-save", "Save", required=False) or (1038, 472)))
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 2, "versioned second pack save")
    assert first.read_bytes() == before
    assert marker.read_text() == "preserve this owned fixture"
    session.observe("second-save", ["Packs"])
    session.click(1085, 222)
    session.observe("index-menu", ["Index folder again"])
    session.click(*session.word("index-menu", "Index"))
    session.observe("reused-index", ["1 reused", "1 analyzed"])
    session.click(*session.word("reused-index", "Filters"))
    session.observe("filters-expanded", ["Any duration"])
    session.click(*(session.word("filters-expanded", "channels", required=False) or (635, 263)))
    session.observe("channel-menu", ["Mono", "Stereo"])
    session.click(*session.word("channel-menu", "Stereo"))
    session.observe("stereo-filter", ["0 / 2"])
    session.click(*(session.word("stereo-filter", "Reset", required=False) or (751, 263)))
    session.observe("filters-reset", ["All channels", "Any duration"])
    session.click(*(session.word("filters-reset", "Actions", required=False) or (1125, 514)))
    session.observe("export-menu", ["WAV kit"])
    session.click(*session.word("export-menu", "WAV"))
    wait_for(lambda: len(list(packs.glob("*/bank.json"))) == 3, "portable WAV kit export")
    exported = next(path for path in packs.glob("*/bank.json") if (path.parent / "pad01_layer1.wav").exists())
    portable = json.loads(exported.read_text())
    assert not Path(portable["slots"][0]["samples"][0]["file_path"]).is_absolute()
    assert not (exported.parent / "pad01_layer1.wav").is_symlink()
    assert (exported.parent / "pad01_layer1.wav").read_bytes()[:4] == b"RIFF"
    session.observe("portable-export", ["Packs"])
    session.key("z", "ctrl")
    session.observe("undo-edit", ["Unsaved changes"])
    private.run_checked([str(session.niri), "msg", "action", "close-window"], env=session.env)
    session.observe("close-guard", ["This sample pack has unsaved changes."])
    session.click(*(session.word("close-guard", "Keep", required=False) or (745, 112)))
    session.key("y", "ctrl")
    session.observe("redo-edit", ["Saved"])
    session.click(*(session.word("redo-edit", "Packs", required=False) or (994, 422)))
    session.observe("pack-browser", ["Saved packs", "Untitled"])
    session.key("Escape")
    session.address(root.parent)
    session.observe("navigated-away", ["fixture"])
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
                                      "constellation pads, native drops, filters, export, undo and close protection",
                                      prepare_environment)


if __name__ == "__main__":
    raise SystemExit(main())
