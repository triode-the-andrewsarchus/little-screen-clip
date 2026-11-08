#!/usr/bin/env python3
"""Check failed audio decoding and recovery on private Weston/Niri."""
from pathlib import Path
import subprocess
import ui_acceptance_fixtures as fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_phase4_acceptance import open_file


def exercise(session, root):
    originals = {path.name: path.read_bytes() for path in root.iterdir() if path.is_file()}
    session.observe("folder", ["List", "Folders"])
    session.click(120, 160)
    for index, filename in enumerate(["broken.wav", "surround.opus"]):
        open_file(session, root, filename, f"{index}-row")
        text = session.observe(f"{index}-failed", [filename, "Audio analysis unavailable", "Back to folder"])
        assert "decoding audio track" not in text.lower()
        session.click(1110, 70)
        session.observe(f"{index}-folder", ["Ctrl+L"])
    open_file(session, root, "valid.wav", "valid-row")
    text = session.observe("valid", ["valid.wav", "00:00:02.", "Selection info"])
    assert "audio analysis unavailable" not in text.lower()
    assert all((root / name).read_bytes() == content for name, content in originals.items())


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    fixture.wav_fixture(root / "valid.wav", 2, "tone")
    (root / "broken.wav").write_bytes(b"This is not an audio stream.\n")
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(root / "valid.wav"),
                    "-ac", "6", "-c:a", "libopus", "-mapping_family", "1", str(root / "surround.opus")],
                   check=True, timeout=15)
    return run_application_acceptance(args, output, root, exercise,
        "corrupt and unsupported audio report failure, preserve sources and allow valid audio afterward")


if __name__ == "__main__":
    raise SystemExit(main())
