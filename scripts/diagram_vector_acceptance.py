#!/usr/bin/env python3
"""Verify live filled artwork while a native drawing drag is still held."""
import json
import subprocess

from PIL import Image

from diagram_drawing_acceptance import SOURCE, open_drawing, run_scenario
from file_manager_acceptance import (
    private, wait_for,
)


def color_bounds(path, color=(208, 235, 255)):
    with Image.open(path).convert("RGB") as image:
        mask = image.point(lambda _: 0).convert("L")
        pixels, marked = image.load(), mask.load()
        count = 0
        for y in range(280, image.height - 50):
            for x in range(250, image.width - 20):
                if all(abs(a - b) <= 3 for a, b in zip(pixels[x, y], color)):
                    marked[x, y] = 255
                    count += 1
        assert count > 2000, "native artwork must retain the rectangle's filled interior"
        return mask.getbbox(), count


def exercise(session, root):
    path = open_drawing(session, root)
    # Allow the worker's first valid source to arrive, using actual artwork
    # rather than a fixed delay or a toolbar that can appear before loading.
    def filled():
        session.capture_text("03-loaded", session.output / "03-loaded.png", ["Pencil"])
        try:
            color_bounds(session.output / "03-loaded.png")
            return True
        except AssertionError:
            return False
    wait_for(filled, "the canonical filled drawing is visible")
    before, _ = color_bounds(session.output / "03-loaded.png")
    green_before, _ = color_bounds(session.output / "03-loaded.png", (211, 249, 216))
    x, y = before[0] + 15, (before[1] + before[3]) // 2
    drag = subprocess.Popen(
        [str(session.pointer), "drag-held", "1280", "800", str(x), str(y), str(x + 60), str(y + 35)],
        env=session.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert drag.stdout.readline().strip() == "holding private drag"
        session.observe("04-held-artwork", ["Pencil"], preserve_pointer=True)
        assert drag.poll() is None, "capture must finish while the pointer is still held"
        during, _ = color_bounds(session.output / "04-held-artwork.png")
        green_during, _ = color_bounds(session.output / "04-held-artwork.png", (211, 249, 216))
        assert all(abs(a - b) <= 2 for a, b in zip(green_before, green_during)), "moving one object must leave the other artwork in place"
        assert all(abs((b - a) - delta) <= 3 for a, b, delta in zip(before, during, (60, 35, 60, 35))), (before, during)
        assert path.read_text() == SOURCE, "a held gesture cannot mutate the source file"
    finally:
        drag.communicate("release\n", timeout=12)
        assert drag.returncode == 0
    session.key("s", "ctrl")
    wait_for(lambda: path.read_text() != SOURCE, "the released visual movement reaches guarded save")
    saved = json.loads(path.read_text())
    assert saved["files"] == json.loads(SOURCE)["files"]
    session.key("z", "ctrl")
    session.key("s", "ctrl")
    wait_for(lambda: path.read_text() == SOURCE, "shared undo restores the exact drawing source")
    session.observe("05-undone", ["Pencil", "Redo"])
    bounds, _ = color_bounds(session.output / "05-undone.png")
    private.run_checked([str(session.pointer), "scroll", "1280", "800", str((bounds[0] + bounds[2]) // 2), str((bounds[1] + bounds[3]) // 2), "-1000"], env=session.env)
    session.observe("06-vector-zoom", ["1600%", "Pencil"])


if __name__ == "__main__":
    raise SystemExit(run_scenario(exercise, "live vector drawing artwork"))
