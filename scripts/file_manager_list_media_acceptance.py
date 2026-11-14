#!/usr/bin/env python3
"""Check that List replaces the redundant media views without losing playback."""
from pathlib import Path
import math
import os
import re
import struct
import wave
from PIL import Image
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_media_audio import start_private_audio, audio_tail


def exercise(session, root):
    session.address(root / "audio")
    text = session.observe("audio-list", ["tone.wav", "List", "Columns", "Photos", "Constellation"])
    assert not re.search(r"\b(?:Music|Videos)\b", text), text
    _, title_y = session.word("audio-list", "tone.wav", minimum_y=210)
    wave_y = title_y + 43
    def waveform_ready():
        session.capture_text("waveform-ready", session.output / "waveform-ready.png", ["tone.wav"])
        with Image.open(session.output / "waveform-ready.png") as image:
            pixels = image.convert("RGB").crop((500, int(wave_y - 15), 1100, int(wave_y + 15))).get_flattened_data()
            return sum(max(pixel) - min(pixel) > 35 for pixel in pixels) > 500
    wait_for(waveform_ready, "colored waveform rendered in List")
    session.click(600, wave_y)
    wait_for(lambda: any(abs(sample) > 500 for sample in audio_tail(session)[0][-1600:]), "List waveform playback on the private sink")
    session.key("Escape")
    wait_for(lambda: not any(abs(sample) > 100 for sample in audio_tail(session)[0][-1600:]), "Escape stops List playback")
    private.run_checked([str(session.pointer), "drag", "1280", "800", "600", str(wave_y), "850", str(wave_y)], env=session.env)
    session.key("space")
    wait_for(lambda: any(abs(sample) > 500 for sample in audio_tail(session)[0][-1600:]), "Space auditions the focused List range")
    session.key("Escape")
    session.address(root / "movies")
    text = session.observe("video-list", ["clip.mp4", "List", "Columns", "Photos", "Constellation"])
    assert not re.search(r"\b(?:Music|Videos)\b", text), text
    _, title_y = session.word("video-list", "clip.mp4", minimum_y=210)
    session.click(700, title_y + 43)
    session.observe("embedded-video", ["Back to folder", "clip.mp4"])
    session.key("Escape")
    session.observe("video-return", ["clip.mp4", "Columns", "Constellation"])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "audio").mkdir()
    (root / "movies").mkdir()
    with wave.open(str(root / "audio/tone.wav"), "wb") as sound:
        sound.setparams((1, 2, 8000, 0, "NONE", "not compressed"))
        sound.writeframes(b"".join(struct.pack("<h", round(math.sin(index * 2 * math.pi * 440 / 8000) * 12000)) for index in range(32000)))
    private.run_checked(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10:duration=2", "-c:v", "libx264", "-pix_fmt", "yuv420p", str(root / "movies/clip.mp4")], env=os.environ)
    return run_application_acceptance(args, output, root, exercise,
        "Only List/Columns/Photos/Constellation; List waveform click/range playback, stop and embedded video activation", lambda env, children: start_private_audio(output, env, children))


if __name__ == "__main__":
    raise SystemExit(main())
