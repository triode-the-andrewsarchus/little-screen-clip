#!/usr/bin/env python3
"""Verify Files FFT strips and their existing range interactions on private Wayland."""
import math
import struct
import subprocess
import time
import tomllib
import wave
from pathlib import Path

from PIL import Image

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import click_word
from file_manager_phase5_acceptance import audio_tail
from file_manager_media_audio import start_private_audio


def choose_audio_view(session, name, mode):
    session.observe(f"{name}-before", [])
    view_x, view_y = session.word(f"{name}-before", "View", tree=True, minimum_y=40)
    session.click(view_x, view_y)
    parent = f"{name}-view-menu"
    audio_position = None
    def parent_visible():
        nonlocal audio_position
        session.capture_text(parent, session.output / f"{parent}.png", [])
        audio_position = session.word(parent, "Audio", tree=True, minimum_y=90, required=False)
        if audio_position is None:
            session.click(view_x, view_y)
        return audio_position
    wait_for(parent_visible, "View audio submenu")
    audio_x, audio_y = audio_position
    hover = subprocess.Popen([str(session.pointer), "move", "1280", "800", str(audio_x), str(audio_y)], env=session.env, stdout=subprocess.DEVNULL)
    try:
        shot = f"{name}-audio"
        position = None
        def menu_visible():
            nonlocal position
            session.capture_text(shot, session.output / f"{shot}.png", [])
            position = session.word(shot, mode, minimum_y=90, required=False)
            return position if position and position[0] > 300 else None
        wait_for(menu_visible, "Audio view choices")
        session.observations.append({"name": shot, "screenshot": str(session.output / f"{shot}.png"), "expected_visible_text": ["Waveform", "Spectrum", "Dadaspec"]})
        session.click(*position)
    finally:
        hover.terminate()
        hover.wait(timeout=3)


def toggle_spectrum(session, name):
    config = tomllib.loads((session.output / "scratch/config.toml").read_text())
    current = config.get("file_manager", {}).get("audio_spectrum", False)
    choose_audio_view(session, name, "Waveform" if current else "Spectrum")


def exercise(session, root):
    session.move_pointer_to_title = lambda: None
    session.observe("waveforms", ["low.wav", "high.wav", "Auto Music threshold"])
    toggle_spectrum(session, "enable")
    assert tomllib.loads((session.output / "scratch/config.toml").read_text())["file_manager"]["audio_spectrum"] is True
    # Wait for the visible bounded workers and capture the fully rendered strips.
    time.sleep(2)
    session.observe("spectra", ["low.wav", "high.wav"])
    _, title_y = session.word("spectra", "high.wav", minimum_y=250)
    y = title_y + 48
    with Image.open(session.output / "spectra.png") as image:
        pixels = image.crop((430, title_y + 25, 1160, title_y + 82)).convert("RGB").get_flattened_data()
        warm = sum(r > 80 and g > 20 and r > b * 1.3 and g > b * 1.3 for r, g, b in pixels)
        assert warm > 500, f"Expected FFT heatmap energy, found {warm} warm pixels"
    private.run_checked([str(session.pointer), "drag", "1280", "800", "600", str(y), "850", str(y)], env=session.env)
    session.observe("spectrum-range", ["Selected", "Copy audio selection"])
    session.key("space")
    wait_for(lambda: any(abs(sample) > 500 for sample in audio_tail(session)[0]), "spectrum range audio on private sink")
    session.observe("range-playback", ["Selected"])
    session.key("Escape")
    # Toggling the visual keeps the selected source interval and file selection.
    toggle_spectrum(session, "disable")
    assert tomllib.loads((session.output / "scratch/config.toml").read_text())["file_manager"]["audio_spectrum"] is False
    session.observe("waveform-range", ["Selected", "Selection: 1 file"])
    # The same setting also applies to music rows embedded in List.
    toggle_spectrum(session, "reenable")
    session.observe("music-return", ["Selected"])
    click_word(session, "music-return", "List", minimum_y=135, maximum_y=185)
    time.sleep(2)
    session.observe("list-spectra", ["Name", "Modified", "high.wav", "low.wav"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    for name, frequency in [("low", 500), ("high", 1500)]:
        with wave.open(str(root / f"{name}.wav"), "wb") as audio:
            audio.setparams((1, 2, 8000, 0, "NONE", "not compressed"))
            audio.writeframes(b"".join(struct.pack("<h", int(12000 * math.sin(2 * math.pi * frequency * frame / 8000))) for frame in range(8000 * 4)))
    return run_application_acceptance(args, output, root, exercise, "FFT strips, menu toggle, range selection and List reuse", lambda env, children: start_private_audio(output, env, children))


if __name__ == "__main__":
    raise SystemExit(main())
