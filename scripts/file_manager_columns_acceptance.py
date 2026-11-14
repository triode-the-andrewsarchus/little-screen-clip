#!/usr/bin/env python3
"""Exercise column navigation and its adjacent preview on the private compositor."""
from pathlib import Path
import subprocess
import ui_acceptance_capture as private
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for


def exercise(session, root):
    session.ocr_regions = [("current-column-ocr.png", (580, 235, 850, 310))]
    session.observe("initial", ["List", "Columns"])
    session.click(176, 160)
    session.observe("columns", ["Preview", "child", "notes.txt"])
    x, y = session.word("columns", "notes.txt", minimum_y=210, last=True)
    session.click(x + 8, y)
    session.observe("text-preview", ["Preview", "Musical", "columns"])
    x, y = session.word("text-preview", "child", minimum_y=210, last=True)
    session.click(x + 8, y)
    session.observe("child", ["Preview", "melody.txt"])
    session.key("Left", "alt")
    session.observe("history-back", ["Preview", "notes.txt", "browse its contents"])
    x, y = session.word("history-back", "artwork.png", minimum_y=210, last=True)
    session.click(x + 8, y)
    def painted_image():
        session.capture_text("image-ready", session.output / "image-ready.png", ["Preview"])
        with Image.open(session.output / "image-ready.png") as screen:
            return sum(b > 150 and r < 100 and g < 120 for r, g, b in screen.convert("RGB").crop((850, 300, 1170, 520)).get_flattened_data()) > 500
    wait_for(painted_image, "selected native image preview", 20)
    session.observe("image-preview", ["Preview", "artwork.png"])
    session.key("Right", "alt")
    session.observe("history-forward", ["Preview", "melody.txt"])
    x, y = session.word("history-forward", "melody.txt", minimum_y=210, last=True)
    session.click(x + 8, y)
    session.observe("leaf-preview", ["Preview", "Nested", "instrument"])
    session.key("Left")
    session.observe("keyboard-parent", ["Preview", "artwork.png", "browse its contents"])
    session.key("Right")
    session.observe("keyboard-child", ["Preview", "melody.txt"])
    session.key("Down")
    session.observe("keyboard-preview", ["Preview", "Nested", "instrument"])


def resize(session, width, height):
    window_id = str(private.own_windows(session.niri, session.env, {session.process.pid})[0]["id"])
    for action in [["set-window-width", "--id", window_id, str(width)], ["set-window-height", "--id", window_id, str(height)]]:
        private.run_checked([str(session.niri), "msg", "action", *action], env=session.env)
    def configured():
        windows = private.own_windows(session.niri, session.env, {session.process.pid})
        size = private.layout_size(windows[0]) if windows else None
        return size is not None and abs(size[0] - width) <= 8 and abs(size[1] - height) <= 8
    wait_for(configured, f"owned window resized to {width}x{height}")
    private.run_checked([str(session.niri), "msg", "action", "move-floating-window", "--id", window_id, "--x", "88", "--y", "20"], env=session.env)
    def positioned():
        position = private.geometry_position(private.own_windows(session.niri, session.env, {session.process.pid})[0])
        return position is not None and abs(position[0] - 88) <= 8 and abs(position[1] - 20) <= 8
    wait_for(positioned, "resized private window placed inside its output")


def click_word(session, shot, label):
    x, y = session.word(shot, label, minimum_y=200, last=True)
    session.click(x + 8, y)


def geometry_exercise(session, root):
    session.ocr_regions = [("wide-back-ocr.png", (1510, 40, 1650, 95))]
    resize(session, 1560, 900)
    session.observe("wide-initial", ["List", "Columns"])
    session.click(730, 160)  # Collapse Shelf before comparing viewport geometry.
    session.click(176, 160)
    session.observe("wide-columns", ["Preview", "tmp", "child", "notes.txt", "unknown.xyz"])
    click_word(session, "wide-columns", "unknown.xyz")
    session.observe("unknown-preview", ["Preview", "unknown.xyz"])
    click_word(session, "unknown-preview", "notes.txt")
    session.observe("wide-text", ["Preview", "Musical", "columns"])
    session.key("Return")
    session.observe("opened-studio", ["Back to folder", "Musical"])
    session.click(1570, 68)  # Back button observed in the resized studio header.
    session.observe("studio-return", ["Columns", "Preview", "Musical"])
    click_word(session, "studio-return", "many")
    session.observe("many-files", ["Columns", "row000.txt"])
    private.run_checked(["wtype", "row099.txt"], env=session.env)
    session.observe("last-row-wide", ["row099.txt", "Row 99"])
    resize(session, 760, 800)
    session.observe("last-row-narrow", ["Columns", "row099.txt"])
    resize(session, 1560, 900)
    session.key("Left", "alt")
    session.observe("many-return", ["Columns", "notes.txt"])
    click_word(session, "many-return", "notes.txt")
    session.observe("text-again", ["Preview", "Musical"])
    resize(session, 760, 800)
    session.observe("narrow-columns", ["Columns", "notes.txt"])
    session.scroll(700, 420, 700, horizontal=True)
    session.observe("narrow-preview", ["Preview", "Musical", "columns"])
    session.key("c", "ctrl")
    def copied():
        result = subprocess.run(["wl-paste", "--no-newline", "--type", "text/uri-list"], capture_output=True, env=session.env, timeout=3)
        return (root / "notes.txt").as_uri().encode() in result.stdout
    wait_for(copied, "Columns keyboard copy publishes the selected file reference")
    session.observe("copied-selection", ["Preview", "Musical"])
    resize(session, 1560, 900)
    session.observe("wide-restored", ["Columns", "Preview", "Musical", "unknown.xyz"])
    session.click(118, 160)
    session.observe("normal-list", ["Type", "Name", "unknown.xyz", "notes.txt"])
    session.click(176, 160)
    session.observe("columns-restored", ["Columns", "Preview", "Musical", "unknown.xyz"])


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"), lambda parser: parser.add_argument("--geometry", action="store_true"))
    if args.geometry:
        args.output_size = (1760, 1000)
    output, root = media_fixture_directory(args)
    if args.geometry:
        (root / "many").mkdir()
        for index in range(100):
            (root / "many" / f"row{index:03}.txt").write_text(f"Row {index} in the long column listing.\n")
    (root / "unknown.xyz").write_bytes(bytes([0, 255, 1, 128]))
    (root / "child").mkdir()
    Image.new("RGB", (160, 100), (30, 70, 220)).save(root / "artwork.png")
    (root / "notes.txt").write_text("Musical columns show the selected source.\n")
    (root / "child" / "melody.txt").write_text("Nested instrument notes.\n")
    raise SystemExit(run_application_acceptance(args, output, root, geometry_exercise if args.geometry else exercise,
        "Column navigation, adjacent previews, history, mixed types and wide/narrow viewport integration"))
