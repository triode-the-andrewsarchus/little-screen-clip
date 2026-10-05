#!/usr/bin/env python3
"""Phase 5 media acceptance on the private Weston/Niri runner."""
from pathlib import Path
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_phase4_acceptance import click_word
import ui_acceptance_capture as private
from file_manager_media_audio import start_private_audio
from file_manager_acceptance import wait_for
import os



def gallery(session, root):
    session.observe("01-list", ["Photos", "notes.txt", "a.png", "b.png"])
    click_word(session, "01-list", "Photos")
    session.observe("02-gallery", ["Thumbnail size", "notes.txt", "subfolder", "a.png", "b.png"])
    x, y = session.word("02-gallery", "a.png")
    session.click(x, y)
    session.key("Return")
    session.observe("03-first-photo", ["a.png", "Next photo", "Start slideshow"])
    click_word(session, "03-first-photo", "Next")
    session.observe("04-next-photo", ["b.png"])
    click_word(session, "04-next-photo", "Previous")
    session.observe("05-previous-photo", ["a.png"])
    click_word(session, "05-previous-photo", "Start")
    session.observe("06-slideshow", ["Pause slideshow"])
    session.observe("07-slideshow-advanced", ["b.png", "Pause slideshow"])
    click_word(session, "07-slideshow-advanced", "Pause")
    session.observe("08-paused", ["Start slideshow"])
    click_word(session, "08-paused", "Back")
    session.observe("09-return-gallery", ["Thumbnail size", "notes.txt", "subfolder"])
    x, y = session.word("09-return-gallery", "notes.txt")
    session.click(x, y)
    session.key("Return")
    session.observe("10-nonmedia", ["GALLERYNOTE", "Edit text"])
    # Text studio has the existing Files back button at this fixture coordinate.
    session.click(150, 118)
    session.observe("11-return", ["Thumbnail size", "subfolder"])
    click_word(session, "11-return", "List")
    session.observe("12-list", ["Name", "Size", "Modified", "notes.txt"])


def music_fixture(output, root):
    folder = root / "Music"
    folder.mkdir()
    (folder / "notes.txt").write_text("MUSICNOTE")
    (folder / "subfolder").mkdir()
    cover = output / "cover.png"
    Image.new("RGB", (160, 160), "purple").save(cover)
    for filename, title, frequency in [("a-source.wav", "ALPHATUNE", "440"), ("b.wav", "BETATUNE", "550"), ("c.wav", "GAMMATUNE", "660")]:
        destination = output / filename if filename == "a-source.wav" else folder / filename
        private.run_checked(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"sine=frequency={frequency}:sample_rate=8000:duration=60", "-metadata", f"title={title}", "-metadata", "artist=ASTER", "-metadata", "album=TESTALBUM", str(destination)], env=os.environ)
    private.run_checked(["ffmpeg", "-v", "error", "-i", str(output / "a-source.wav"), "-i", str(cover), "-map", "0:a", "-map", "1:v", "-c:a", "flac", "-c:v", "png", "-disposition:v", "attached_pic", str(folder / "a.flac")], env=os.environ)
    return folder


def music(session, root):
    session.observe("01-auto-music", ["Auto Music threshold", "Title", "Artist", "Album"])
    session.observe("02-metadata", ["ALPHATUNE", "BETATUNE", "ASTER", "TESTALBUM", "subfolder"])
    click_word(session, "02-metadata", "Play")
    session.observe("03-playing", ["Pause music", "a.flac", "Seek", "Queue"])
    click_word(session, "03-playing", "Pause")
    session.observe("04-paused", ["Play music", "a.flac"])
    click_word(session, "04-paused", "Next")
    session.observe("05-next", ["b.wav", "Pause music"])
    click_word(session, "05-next", "Previous")
    session.observe("06-previous", ["a.flac", "Pause music"])
    click_word(session, "06-previous", "Repeat")
    click_word(session, "06-previous", "Shuffle")
    session.observe("07-repeat-shuffle", ["Repeat: all", "Shuffle: on"])
    click_word(session, "07-repeat-shuffle", "Pause")
    session.address(root.parent)
    session.observe("08-navigation", ["a.flac", "Play music", "Music"])
    session.address(root)
    session.observe("09-return", ["Auto Music threshold", "a.flac", "Play music"])
    click_word(session, "09-return", "List")
    session.observe("10-manual-list", ["Name", "Modified"])
    session.key("F5")
    session.observe("11-manual-refresh", ["Name", "Modified", "a.flac"])
    click_word(session, "11-manual-refresh", "Music", last=True, minimum_y=220, maximum_y=270)
    session.observe("12-music-return", ["Auto Music threshold", "ALPHATUNE"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager", lambda p: p.add_argument("--scenario", choices=["gallery", "music"], default="gallery"))
    output, root = media_fixture_directory(args)
    if args.scenario == "music":
        folder = music_fixture(output, root)
        # The shared runner owns every compositor and audio process.
        return run_application_acceptance(args, output, folder, music, "music", lambda env, children: start_private_audio(output, env, children))
    for name, color in [("a.png", "red"), ("b.png", "blue")]:
        Image.new("RGB", (300, 180), color).save(root / name)
    (root / "notes.txt").write_text("GALLERYNOTE")
    (root / "subfolder").mkdir()
    return run_application_acceptance(args, output, root, gallery, "gallery")


if __name__ == "__main__":
    raise SystemExit(main())
