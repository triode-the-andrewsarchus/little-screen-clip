#!/usr/bin/env python3
"""Photo similarity smoke test using only the private Weston/Niri compositor.

Reuses cached models without downloads; screenshots, configuration and the photo
feature database belong to the test's output directory.
"""
from pathlib import Path
import shutil
import time

from PIL import Image, ImageDraw

from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)
import ui_acceptance_capture as private
from file_manager_phase4_acceptance import click_word


def configure(parser):
    parser.add_argument("--model-cache", type=Path,
                        default=Path.home() / ".cache/little-screen-clip/models")


def prepare(output, model_cache, appearance, env, children):
    del children
    config = f'[workbench]\nappearance = "{appearance}"\n[file_manager]\nphoto_mode = "similarity"\nphoto_recurse = true\n'
    # Both possible configuration paths are private to this runner.
    for target in [output / "scratch/config.toml",
                   Path(env["XDG_CONFIG_HOME"]) / "little-screen-clip/config.toml"]:
        target.write_text(config)
    cache = output / "cache"
    models = cache / "little-screen-clip/models"
    models.mkdir(parents=True)
    for name in ["vision_model_fp16.onnx", "vision_model_fp16.onnx_data",
                 "siglip2-giant-text.onnx",
                 "siglip2-giant-tokenizer.json"]:
        shutil.copyfile(model_cache / name, models / name)
    env["XDG_CACHE_HOME"] = str(cache)


def exercise(session, root):
    capture = session.capture_text
    def capture_canvas(name, path, expected):
        text = capture(name, path, [])
        # Sparse full-screen OCR omits floating overlays; inspect their own rows.
        with Image.open(path) as screen:
            for label, bounds in [("search", (350, 255, 1180, 315)),
                                  ("status", (350, 490, 1180, 545))]:
                crop = screen.crop(bounds)
                target = session.output / f"{name}-{label}.png"
                crop.resize((crop.width * 3, crop.height * 3)).save(target)
                text += private.run_checked(["tesseract", str(target), "stdout", "--psm", "6"], env=session.env)
        return text
    session.capture_text = capture_canvas
    session.observe("01-photos", ["Photos"])
    click_word(session, "01-photos", "Photos")
    session.observe("02-similarity", ["Reset View"])
    # Readiness is the persisted result count, rather than an arbitrary inference delay.
    import sqlite3
    database = session.output / "cache/little-screen-clip/photo-similarity.sqlite"

    def indexed():
        if not database.exists():
            return False
        try:
            with sqlite3.connect(database) as connection:
                return connection.execute("SELECT COUNT(*) FROM features WHERE length(vector)=6144").fetchone()[0] == 24
        except sqlite3.OperationalError:
            return False
    deadline = time.monotonic() + 120
    while not indexed():
        if session.process.poll() is not None or time.monotonic() >= deadline:
            raise RuntimeError("Indexing did not persist all 24 image features")
        time.sleep(0.1)
    session.observe("03-indexed", ["24 photos"])
    # Search input is in the top-left overlay; locating its hint avoids assumptions
    # about the canvas's screen position or the sidebar width.
    session.click(480, 283)
    private.run_checked(["wtype", "red"], env=session.env)
    session.key("Return")
    session.observe("04-search", ["target grid"])
    # Escape clears the text focus before browser selection shortcuts are used.
    session.key("Escape")
    session.click(750, 450)
    session.move_pointer_to_title()
    for _ in range(3):
        session.key("equal")
    session.observe("04-controls", ["24 photos"])
    session.key("a", "ctrl")
    session.observe("05-selection", ["24 files"])
    session.address(root / "nested")
    session.observe("06-navigation", ["photo-16.png"])
    click_word(session, "06-navigation", "Photos")
    session.observe("06-nested", ["8 photos"])
    session.address(root)
    session.observe("07-navigation", ["nested"])
    click_word(session, "07-navigation", "Photos")
    session.observe("07-return", ["Reset View", "24 photos"])
    session.observe("08-final", [])


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"), configure)
    for name in ["vision_model_fp16.onnx", "vision_model_fp16.onnx_data",
                 "siglip2-giant-text.onnx",
                 "siglip2-giant-tokenizer.json"]:
        if not (args.model_cache / name).is_file():
            raise SystemExit(f"Required cached model: {args.model_cache / name}")
    output, root = media_fixture_directory(args)
    (root / "nested").mkdir()
    for index in range(24):
        image = Image.new("RGB", (160, 120), ((index * 41) % 256, (index * 79) % 256, (index * 113) % 256))
        ImageDraw.Draw(image).ellipse((25, 10, 135, 110), fill="red" if index % 2 else "blue")
        destination = root / "nested" if index >= 16 else root
        image.save(destination / f"photo-{index:02}.png")
    return run_application_acceptance(
        args, output, root, exercise, "photo similarity indexing, searches and recursive selection",
        lambda env, children: prepare(output, args.model_cache, args.appearance, env, children))


if __name__ == "__main__":
    raise SystemExit(main())
