#!/usr/bin/env python3
"""Verify both saved audio-strip modes across fresh private Files launches."""
import math
import struct
import tomllib
import wave
from pathlib import Path

from PIL import Image

from file_manager_acceptance import application_arguments, run_application_acceptance, wait_for
from file_manager_spectrum_acceptance import toggle_spectrum


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    base = args.output_dir.resolve()
    base.mkdir(mode=0o700, parents=True)
    root = base / "fixture"
    root.mkdir()
    with wave.open(str(root / "tone.wav"), "wb") as audio:
        audio.setparams((1, 2, 8000, 0, "NONE", "not compressed"))
        audio.writeframes(b"".join(struct.pack("<h", int(12000 * math.sin(2 * math.pi * 500 * frame / 8000))) for frame in range(16000)))
    saved = None
    for name, initial, toggle in [("enable", False, True), ("disable-after-restart", True, False), ("waveform-after-restart", False, None)]:
        output = base / name
        output.mkdir(mode=0o700)

        def prepare(env, children):
            if saved is not None:
                (output / "scratch/config.toml").write_text(saved)

        def exercise(session, root):
            nonlocal saved
            session.move_pointer_to_title = lambda: None
            session.observe("loaded", ["tone.wav", "Auto Music threshold"])
            _, title_y = session.word("loaded", "tone.wav", minimum_y=250)

            def rendered():
                session.capture_text("restored-mode", output / "restored-mode.png", [])
                with Image.open(output / "restored-mode.png") as image:
                    pixels = image.crop((430, title_y + 25, 1160, title_y + 82)).convert("RGB").get_flattened_data()
                    if initial:
                        count = sum(r > 80 and g > 20 and r > b * 1.3 and g > b * 1.3 for r, g, b in pixels)
                    else:
                        count = sum(g > 80 and g > r * 1.1 for r, g, b in pixels)
                return count > 300

            wait_for(rendered, f"restored {'spectrum' if initial else 'waveform'} pixels")
            session.observations.append({"name": "restored-mode", "screenshot": str(output / "restored-mode.png"), "spectrum": initial})
            if toggle is not None:
                toggle_spectrum(session, "toggle")
                saved = (output / "scratch/config.toml").read_text()
                config = tomllib.loads(saved)
                assert config["file_manager"]["audio_spectrum"] is toggle
                assert config["workbench"]["appearance"] == args.appearance
            else:
                assert tomllib.loads((output / "scratch/config.toml").read_text())["file_manager"]["audio_spectrum"] is False

        result = run_application_acceptance(args, output, root, exercise, name, prepare)
        if result:
            return result
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
