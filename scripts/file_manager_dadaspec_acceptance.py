#!/usr/bin/env python3
"""Verify native stereo Dadaspec strips, selection, persistence and Audio Studio."""
import math
import struct
import tomllib
import wave
from pathlib import Path

from PIL import Image

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file
from file_manager_spectrum_acceptance import choose_audio_view


def exercise(session, root):
    session.observe("waveform", ["stereo.wav", "mono.wav"])
    choose_audio_view(session, "enable", "Dadaspec")
    config = tomllib.loads((session.output / "scratch/config.toml").read_text())
    assert config["file_manager"]["audio_dadaspec"] is True
    assert config["file_manager"]["audio_spectrum"] is False
    session.observe("music-dadaspec", ["stereo.wav"])
    _, title_y = session.word("music-dadaspec", "stereo.wav", minimum_y=250)

    def rendered(name="stereo-colors"):
        session.capture_text(name, session.output / f"{name}.png", [])
        with Image.open(session.output / f"{name}.png") as image:
            pixels = image.crop((430, title_y + 25, 1160, title_y + 82)).convert("RGB").get_flattened_data()
            red = blue = 0
            for r, g, b in pixels:
                red += r > 50 and r > b * 1.6
                blue += b > 50 and b > r * 1.6
        return red > 300 and blue > 300
    wait_for(rendered, "independent bass-red left and high-blue right halves")
    with Image.open(session.output / "stereo-colors.png") as image:
        image = image.convert("RGB")
        # The top of the bass-only left channel has no spectral energy. Its
        # pixels must show exactly the row background, without a strip backplate.
        outside = image.getpixel((1170, title_y + 30))
        for x in [440, 700, 1000]:
            assert image.getpixel((x, title_y + 30)) == outside, "Dadaspec still has a filled backdrop"
    session.observations.append({"name": "stereo-colors", "screenshot": str(session.output / "stereo-colors.png"), "expected_visible_text": ["stereo.wav"], "verified_stereo_colors": True})
    private.run_checked([str(session.pointer), "drag", "1280", "800", "600", str(title_y + 48), "850", str(title_y + 48)], env=session.env)
    session.observe("dadaspec-range", ["Selection: 1 file"])
    def range_painted(name):
        session.capture_text(name, session.output / f"{name}.png", [])
        with Image.open(session.output / f"{name}.png") as image:
            inside = image.crop((650, title_y + 30, 800, title_y + 78)).convert("RGB")
            outside = image.crop((930, title_y + 30, 1080, title_y + 78)).convert("RGB")
            mean = lambda crop: [sum(p[c] for p in crop.get_flattened_data()) / (crop.width * crop.height) for c in range(3)]
            # Selection lightens dark surfaces and darkens light surfaces.
            return sum(abs(a - b) for a, b in zip(mean(inside), mean(outside))) > 15
    wait_for(lambda: range_painted("dadaspec-range-overlay"), "visible selected Dadaspec interval")
    choose_audio_view(session, "waveform-return", "Waveform")
    session.observe("retained-range", ["Selection: 1 file"])
    choose_audio_view(session, "dadaspec-return", "Dadaspec")
    session.observe("retained-dadaspec-range", ["Selection: 1 file"])
    wait_for(lambda: range_painted("retained-range-overlay"), "same interval after switching visualizations")
    # The first view tab stays at this position in the private 1280 × 800 window.
    # OCR loses the selected light-theme label against its underline.
    session.click(120, 160)
    session.observe("list-dadaspec", ["Name", "Modified", "stereo.wav"])
    open_file(session, root, "stereo.wav", "open-stereo")
    session.observe("studio-waveform", ["Slice", "Copy"])
    assert "Dadaspec" not in session.capture_text("studio-waveform-only", session.output / "studio-waveform-only.png", [])
    session.close()
    session.restart(session.binary, root)
    session.observe("restored-dadaspec", ["stereo.wav"])
    _, title_y = session.word("restored-dadaspec", "stereo.wav", minimum_y=250)
    wait_for(lambda: rendered("restored-stereo-colors"), "Dadaspec pixels after restart")
    session.observations.append({"name": "restored-stereo-colors", "screenshot": str(session.output / "restored-stereo-colors.png"), "verified_stereo_colors": True})
    # Exercise the ordinary compositor close request for the reopened client.
    windows = private.own_windows(session.niri, session.env, {session.process.pid})
    private.run_checked([str(session.niri), "msg", "action", "close-window", "--id", str(windows[0]["id"])], env=session.env)
    session.process.wait(timeout=10)
    assert session.process.returncode == 0
    assert tomllib.loads((session.output / "scratch/config.toml").read_text())["file_manager"]["audio_dadaspec"] is True


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    for name, stereo in [("stereo", True), ("mono", False)]:
        with wave.open(str(root / f"{name}.wav"), "wb") as audio:
            audio.setparams((2 if stereo else 1, 2, 32000, 0, "NONE", "not compressed"))
            frames = []
            for frame in range(32000 * 4):
                left = int(14000 * math.sin(2 * math.pi * 80 * frame / 32000))
                right = int(14000 * math.sin(2 * math.pi * 5000 * frame / 32000))
                if stereo:
                    frames.append(struct.pack("<hh", left, right))
                else:
                    # Silence and all seven combinations of bass/mid/high;
                    # smooth edges also leave clear transparent gaps to inspect.
                    bands = [(), (80,), (800,), (5000,), (80, 800),
                             (800, 5000), (80, 5000), (80, 800, 5000)][frame // 16000]
                    phase = (frame % 16000) / 16000
                    envelope = math.sin(math.pi * min(1, phase / .8)) ** 2 if phase < .8 else 0
                    sample = int(8000 * envelope * sum(math.sin(2 * math.pi * hz * frame / 32000) for hz in bands))
                    frames.append(struct.pack("<h", sample))
            audio.writeframes(b"".join(frames))
    def prepare(env, children):
        config = output / "scratch/config.toml"
        with config.open("a") as file:
            file.write("\n[audio]\ndadaspec = true\n")
    def run(session, root):
        session.binary = args.binary
        exercise(session, root)
    return run_application_acceptance(args, output, root, run, "Native Dadaspec stereo colors, ranges, List, Studio and restart persistence", prepare)


if __name__ == "__main__":
    raise SystemExit(main())
