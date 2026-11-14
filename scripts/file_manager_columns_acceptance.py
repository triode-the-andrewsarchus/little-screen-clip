#!/usr/bin/env python3
"""Exercise column navigation and its adjacent preview on the private compositor."""
from pathlib import Path
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


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "child").mkdir()
    Image.new("RGB", (160, 100), (30, 70, 220)).save(root / "artwork.png")
    (root / "notes.txt").write_text("Musical columns show the selected source.\n")
    (root / "child" / "melody.txt").write_text("Nested instrument notes.\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Column navigation, adjacent text previews and browser history restoration"))
