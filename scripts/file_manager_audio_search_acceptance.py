#!/usr/bin/env python3
"""Stale search-result audio playback rejection on the private compositor."""
import wave
from pathlib import Path

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)
from file_manager_search_acceptance import submit


def write_audio(path):
    with wave.open(str(path), "wb") as audio:
        audio.setnchannels(1)
        audio.setsampwidth(2)
        audio.setframerate(44100)
        audio.writeframes(bytes(44100 * 2 * 2))


def click_strip(session, shot):
    _, y = session.word(shot, "sample.wav", minimum_y=190)
    session.click(700, y + 54)


def exercise(session, root):
    session.observe("initial", ["Nested"])
    submit(session, ":audio;scope:recursive")
    session.observe("audio-result", ["sample.wav", "2 s", "Title"])
    target = root / "Nested/sample.wav"
    target.write_bytes(b"a replaced source must not enter an audio decoder")
    click_strip(session, "audio-result")
    session.observe("changed-audio-rejected", ["Cannot play this track", "Search result changed"])
    session.key("Escape")
    write_audio(target)
    session.key("F5")
    submit(session, ":audio;scope:recursive")
    session.observe("fresh-observation", ["sample.wav", "2 s", "Title"])
    outside = session.output / "outside"
    (root / "Nested").rename(outside)
    (root / "Nested").symlink_to(outside, target_is_directory=True)
    click_strip(session, "fresh-observation")
    session.observe("moved-parent-rejected", ["Cannot play this track", "Search result changed"])
    assert not (Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite").exists()


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Nested").mkdir()
    write_audio(root / "Nested/sample.wav")
    return run_application_acceptance(args, output, root, exercise,
        "Waveform-click playback rejects replaced audio and moved-parent aliases using the result's original source context")


if __name__ == "__main__":
    raise SystemExit(main())
