#!/usr/bin/env python3
"""Phase 5 media acceptance on the private Weston/Niri runner."""
from pathlib import Path
from PIL import Image, ImageChops
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_phase4_acceptance import click_word, back
import ui_acceptance_capture as private
from file_manager_media_audio import start_private_audio
from file_manager_acceptance import wait_for
import os
import struct
import hashlib
import json
import time
import subprocess


def audio_tail(session):
    path = session.output / "audio-output.pcm"
    if not path.exists():
        return (), 0
    with path.open("rb") as source:
        size = source.seek(0, 2)
        source.seek(max(0, size - 32000))
        data = source.read(32000)
    count = len(data) // 2
    return struct.unpack("<" + "h" * count, data[:count * 2]), size


def unavailable_video(session, root):
    session.observe("01-video-tools", ["movie.mp4"])
    x, y = session.word("01-video-tools", "movie.mp4", minimum_y=300)
    session.click(x, y)
    session.key("Return")
    session.observe("02-unavailable-player", ["install the FFmpeg player package", "Open externally", "Back to folder"])
    windows = json.loads(private.run_checked([str(session.niri), "msg", "--json", "windows"], env=session.env))
    assert not any(window["title"] == "Files video" for window in windows)
    back(session, "02-unavailable-player")
    session.observe("03-tools-return", ["movie.mp4", "notes.txt"])


def unavailable_environment(output, env, children):
    tools = output / "tools"
    tools.mkdir()
    unavailable = tools / "ffplay"
    unavailable.write_text("#!/bin/sh\nexit 127\n")
    unavailable.chmod(0o700)
    env["PATH"] = str(tools) + os.pathsep + env["PATH"]
    return env



def gallery(session, root):
    session.observe("01-list", ["Photos", "notes.txt", "4 items"])
    click_word(session, "01-list", "Photos")
    session.observe("02-gallery", ["Thumbnail size"])
    with Image.open(session.output / "02-gallery.png") as image:
        pixels = list(image.convert("RGB").get_flattened_data())
        red = [index for index, (r, g, b) in enumerate(pixels) if r > 200 and g < 60 and b < 60]
        blue = [index for index, (r, g, b) in enumerate(pixels) if b > 200 and r < 60 and g < 60]
        assert len(red) > 500 and len(blue) > 500, "both photo thumbnails must display their source pixels"
        x = round(sum(index % image.width for index in red) / len(red))
        y = round(sum(index // image.width for index in red) / len(red))
        # Full-screen sparse OCR misses captions beside large solid-color tiles.
        # Infer the adjacent square cells from the actual rendered photo bounds.
        top = min(index // image.width for index in red)
        bottom = max(index // image.width for index in red) + 1
        square = bottom - top
        left = min(index % image.width for index in red)
        right = max(index % image.width for index in blue) + 1
        for label, cell in [("subfolder", (left - square - 2, left - 2)),
                            ("notes.txt", (right + 2, right + square + 2))]:
            caption = image.crop((cell[0], bottom - 30, cell[1], bottom))
            path = session.output / f"02-gallery-{label}-caption.png"
            caption.resize((caption.width * 3, caption.height * 3)).save(path)
            text = private.run_checked(["tesseract", str(path), "stdout", "--psm", "7"], env=session.env)
            assert label in text, f"gallery caption missing: {label}: {text}"
        note_position = (round(right + 2 + square / 2), round((top + bottom) / 2))
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
    session.observe("09-return-gallery", ["Thumbnail size"])
    session.click(*note_position)
    session.key("Return")
    session.observe("10-nonmedia", ["GALLERYNOTE", "Edit text"])
    # Text studio has the existing Files back button at this fixture coordinate.
    session.click(150, 118)
    session.observe("11-return", ["Thumbnail size"])
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
    session.observe("01-auto-music", ["Use automatic view", "Title", "Artist", "Album"])
    session.observe("02-metadata", ["ALPHATUNE", "BETATUNE", "ASTER", "TESTALBUM", "subfolder"])
    click_word(session, "02-metadata", "Play")
    session.observe("03-playing", ["Pause music", "a.flac", "Seek", "Queue"])
    def audible():
        return any(abs(sample) > 500 for sample in audio_tail(session)[0])
    wait_for(audible, "actual music PCM on the private null sink")
    click_word(session, "03-playing", "Pause")
    session.observe("04-paused", ["Play music", "a.flac"])
    click_word(session, "04-paused", "Next")
    session.observe("05-next", ["b.wav", "Pause music"])
    click_word(session, "05-next", "Previous")
    session.observe("06-previous", ["a.flac", "Pause music"])
    session.click(485, 635)  # Repeat button in the fixed private player dock.
    session.click(600, 635)
    session.observe("07-repeat-shuffle", ["Repeat: all", "Shuffle: on"])
    click_word(session, "07-repeat-shuffle", "Pause")
    session.address(root.parent)
    session.observe("08-navigation", ["a.flac", "Play music", "Music"])
    session.address(root)
    session.observe("09-return", ["Use automatic view", "a.flac", "Play music"])
    session.click(270, 243)  # List tab in the fixed private viewport.
    session.observe("10-manual-list", ["Name", "Modified"])
    session.key("F5")
    session.observe("11-manual-refresh", ["Name", "Modified", "a.flac"])
    click_word(session, "11-manual-refresh", "Music", last=True, minimum_y=220, maximum_y=270)
    session.observe("12-music-return", ["Use automatic view", "ALPHATUNE"])
    original = hashlib.sha256((root / "a.flac").read_bytes()).hexdigest()
    _, title_y = session.word("12-music-return", "ALPHATUNE", minimum_y=400)
    session.click(795, title_y + 43)
    session.observe("13-waveform-seek", ["Play music", "Seek"])
    with Image.open(session.output / "13-waveform-seek.png") as image:
        pixels = image.crop((790, title_y + 20, 800, title_y + 65)).get_flattened_data()
        assert sum(b - r > 60 and g - r > 30 for r, g, b, *rest in pixels) > 20, "paused playhead must seek to the clicked waveform coordinate"
    private.run_checked([str(session.pointer), "drag", "1280", "800", "500", str(title_y + 43), "700", str(title_y + 43)], env=session.env)
    session.observe("14-audio-selection", ["Selected", "Copy audio selection"])
    session.key("c", "ctrl")
    wait_for(lambda: "text/uri-list" in private.run_checked(["wl-paste", "--list-types"], env=session.env), "copied audio URI")
    before = set(root.iterdir())
    session.key("v", "ctrl")
    wait_for(lambda: len(set(root.iterdir()) - before) == 1, "pasted waveform selection")
    pasted = (set(root.iterdir()) - before).pop()
    probe = json.loads(private.run_checked(["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", str(pasted)], env=session.env))
    duration = float(probe["format"]["duration"])
    assert 14 < duration < 18, f"copied segment must match dragged waveform range, got {duration} s"
    assert hashlib.sha256((root / "a.flac").read_bytes()).hexdigest() == original
    session.key("F5")
    session.observe("15-pasted-audio", ["6 items"])
    session.click(270, 243)
    session.observe("16-studio-source", ["a.flac", "Name", "Play music"])
    click_word(session, "16-studio-source", "Play", minimum_y=600)
    session.click(500, 370)  # First regular-file row, beneath the folder row.
    session.key("Return")
    session.observe("17-audio-studio", ["AUDIO STUDIO", "Selection info"])
    paused_at = audio_tail(session)[1]
    def studio_quiet():
        samples, size = audio_tail(session)
        return size > paused_at + 16000 and all(abs(sample) < 50 for sample in samples[-4000:])
    wait_for(studio_quiet, "Audio Studio pauses queued music on the private sink")
    back(session, "17-audio-studio")
    session.observe("18-studio-return", ["Name", "Play music", "a.flac"])
    session.click(275, 635)
    session.address(root.parent)
    session.observe("19-playing-navigation", ["Pause music", "a.flac"])
    session.address(root)
    session.observe("20-playing-return", ["Pause music", "a.flac", "Use automatic view"])
    session.click(275, 635)
    session.observe("21-navigation-paused", ["Play music"])


def video(session, root):
    session.observe("01-video-folder", ["movie.mp4", "notes.txt", "subfolder"])
    x, y = session.word("01-video-folder", "movie.mp4", minimum_y=300)
    session.click(x, y)
    session.key("Return")
    def windows():
        return json.loads(private.run_checked([str(session.niri), "msg", "--json", "windows"], env=session.env))
    def player_window():
        return next((window for window in windows() if window["title"] == "Files video"), None)
    wait_for(player_window, "owned video window")
    window = player_window()
    private.run_checked([str(session.niri), "msg", "action", "focus-window", "--id", str(window["id"])], env=session.env)
    if not window.get("is_floating"):
        private.run_checked([str(session.niri), "msg", "action", "toggle-window-floating"], env=session.env)
    def capture(name):
        path = session.output / f"{name}.png"
        private.screenshot(session.niri, session.env, path)
        return Image.open(path).convert("RGB")
    def changed(a, b):
        return sum(pixel != (0, 0, 0) for pixel in ImageChops.difference(a, b).get_flattened_data())
    first = capture("02-video-frame")
    time.sleep(0.5)
    second = capture("03-video-advanced")
    assert changed(first, second) > 1000, "video must display advancing frames"
    session.key("space")
    time.sleep(0.3)
    paused = capture("04-video-paused")
    time.sleep(0.5)
    assert changed(paused, capture("05-video-still")) < 100, "Space must pause video frames"
    session.key("Right")
    session.key("space")
    session.key("q")
    wait_for(lambda: player_window() is None, "closed video window")
    files = next(window for window in windows() if window["app_id"] == "little-clip-file-manager")
    private.run_checked([str(session.niri), "msg", "action", "focus-window", "--id", str(files["id"])], env=session.env)
    session.observe("06-video-closed", ["Video playback", "Video finished or closed", "Replay video"])
    click_word(session, "06-video-closed", "Replay")
    wait_for(player_window, "replayed video window")
    child_pid = int(subprocess.check_output(["pgrep", "-P", str(session.process.pid), "ffplay"]).strip())
    private.run_checked([str(session.niri), "msg", "action", "focus-window", "--id", str(files["id"])], env=session.env)
    session.observe("07-video-replayed", ["Playing in the Files video window", "Back to folder"])
    click_word(session, "07-video-replayed", "Back")
    wait_for(lambda: not Path(f"/proc/{child_pid}").exists(), "returning to folder reaps FFplay")
    session.observe("08-return-folder", ["movie.mp4", "notes.txt", "subfolder"])
    x, y = session.word("08-return-folder", "movie.mp4", minimum_y=300)
    session.click(x, y)
    session.key("Return")
    wait_for(player_window, "video session for application-close ownership check")
    pid = int(subprocess.check_output(["pgrep", "-P", str(session.process.pid), "ffplay"]).strip())
    session.required_closed_processes = [pid]
    private.run_checked([str(session.niri), "msg", "action", "focus-window", "--id", str(files["id"])], env=session.env)


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager", lambda p: p.add_argument("--scenario", choices=["gallery", "music", "video", "video-unavailable"], default="gallery"))
    output, root = media_fixture_directory(args)
    if args.scenario == "music":
        folder = music_fixture(output, root)
        # The shared runner owns every compositor and audio process.
        return run_application_acceptance(args, output, folder, music, "music", lambda env, children: start_private_audio(output, env, children))
    if args.scenario in ("video", "video-unavailable"):
        (root / "notes.txt").write_text("VIDEONOTE")
        (root / "subfolder").mkdir()
        private.run_checked(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24:duration=60", "-f", "lavfi", "-i", "sine=frequency=440:duration=60", "-c:v", "libx264", "-preset", "ultrafast", "-c:a", "aac", "-shortest", str(root / "movie.mp4")], env=os.environ, timeout=30)
        if args.scenario == "video-unavailable":
            return run_application_acceptance(args, output, root, unavailable_video, "video-unavailable", lambda env, children: unavailable_environment(output, env, children))
        return run_application_acceptance(args, output, root, video, "video", lambda env, children: start_private_audio(output, env, children))
    for name, color in [("a.png", "red"), ("b.png", "blue")]:
        Image.new("RGB", (300, 180), color).save(root / name)
    (root / "notes.txt").write_text("GALLERYNOTE")
    (root / "subfolder").mkdir()
    return run_application_acceptance(args, output, root, gallery, "gallery")


if __name__ == "__main__":
    raise SystemExit(main())
