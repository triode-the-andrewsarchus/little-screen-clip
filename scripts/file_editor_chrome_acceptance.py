#!/usr/bin/env python3
"""Check shared filename editing and menu navigation on private Weston/Niri."""
import struct
import subprocess
from pathlib import Path
import ui_acceptance_capture as private
import ui_acceptance_fixtures as fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import open_file


def exercise(session, root, audio_extension, audio_only=False):
    audio = f"music.{audio_extension}"
    audio_source = (root / audio).read_bytes()
    cases = [
        ("notes.txt", "renamed.txt", []),
        (audio, "renamed" + Path(audio).suffix, ["Selection info"]),
        ("document.pdf", "renamed.pdf", ["Pages"]),
        ("table.csv", "renamed.csv", ["Copy Table"]),
        ("score.mid", "renamed.mid", ["BPM"]),
        ("mesh.stl", "renamed.stl", ["triangles"]),
        ("scene.splat", "renamed.splat", []),
        ("letter.rtf", "renamed.rtf", []),
        ("clip.mp4", "renamed.mp4", ["Replay video"]),
    ]
    if audio_only:
        cases = cases[1:2]
    for index, (original, renamed, evidence) in enumerate(cases):
        open_file(session, root, original, f"{index:02d}-browser")
        name = f"{index:02d}-studio"
        text = session.observe(name, [original, "Back to folder", *evidence], expected_any=["Gaussians", "Preview unavailable"] if original == "scene.splat" else ())
        for duplicate in ["Open externally", "Audio Studio", "Image Studio", "Back to folder: browse"]:
            assert duplicate.lower() not in text.lower(), (original, duplicate)
        # All shared headers begin below the menu; Text adds its Preview/Edit tabs first.
        x, y = session.word(name, original, tree=True, minimum_y=95)
        session.click(x + 12, y)
        session.observe(f"{index:02d}-rename-dialog", ["Filename"])
        session.key("a", "ctrl")
        private.run_checked(["wtype", renamed], env=session.env)
        session.key("Return")
        wait_for(lambda: (root / renamed).exists() and not (root / original).exists(), f"rename {original}")
        text = session.observe(f"{index:02d}-renamed", [renamed, "Back to folder"])
        assert "Photo renamed" not in text
        if original == audio:
            assert "Track gain" not in text
            assert (root / renamed).read_bytes() == audio_source
        # Back is in the same top menu for every studio, regardless of its internal docks.
        session.click(1110, 70)
        session.observe(f"{index:02d}-returned", ["Ctrl+L"])
    assert not any(root.glob("*.tmp"))


def main():
    def configure(parser):
        parser.add_argument("--audio-extension", choices=["wav", "m4b", "mp2", "adts"], default="wav")
        parser.add_argument("--audio-only", action="store_true", help="Check the chosen audio format without the other editor fixtures")

    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager",
        configure)
    output, root = media_fixture_directory(args)
    (root / "notes.txt").write_text("A plain text fixture\n")
    fixture.wav_fixture(root / "music.wav", 2, "tone")
    if args.audio_extension != "wav":
        codec, container = {"m4b": ("aac", "ipod"), "mp2": ("mp2", "mp2"), "adts": ("aac", "adts")}[args.audio_extension]
        subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-i", str(root / "music.wav"), "-c:a", codec, "-f", container, str(root / f"music.{args.audio_extension}")], check=True, timeout=15)
        (root / "music.wav").unlink()
    fixture.pdf_fixture(root, 2, "document.pdf")
    (root / "table.csv").write_text("name,value\nalpha,1\nbeta,2\n")
    fixture.midi_fixture(root)
    (root / "music-three-track.mid").rename(root / "score.mid")
    fixture.stl_fixture(root)
    (root / "model-simple.stl").rename(root / "mesh.stl")
    (root / "scene.splat").write_bytes(struct.pack("<6f8B", 0, 0, 0, .1, .1, .1, 220, 80, 40, 255, 255, 128, 128, 128))
    (root / "letter.rtf").write_text(r"{\rtf1\ansi A document fixture\par}")
    subprocess.run(["ffmpeg", "-nostdin", "-v", "error", "-f", "lavfi", "-i", "color=c=blue:s=160x120:d=2", "-c:v", "mpeg4", str(root / "clip.mp4")], check=True, timeout=15)
    scope = "Files audio editor, source-preserving rename and folder return" if args.audio_only else "shared Files chrome and guarded rename across all built-in editors"
    return run_application_acceptance(args, output, root, lambda session, root: exercise(session, root, args.audio_extension, args.audio_only), scope)


if __name__ == "__main__":
    raise SystemExit(main())
