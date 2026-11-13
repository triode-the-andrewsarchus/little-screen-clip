#!/usr/bin/env python3
"""Verify native drawing text, label bindings and held corner/image resizing.

Use rebuilt apps only. Run light and dark with distinct --output-dir values.
All input/screenshots target the existing private Wayland runner. Each accepted
edit is saved and undone to exact authored bytes; the original fixture ends intact.
"""
from contextlib import contextmanager
import json
import subprocess
from pathlib import Path

from PIL import Image

from constellation_pitch_acceptance import control as isolated_control
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, private,
    run_application_acceptance, wait_for,
)

SOURCE = (Path(__file__).resolve().parents[1] /
          "docs/examples/diagrams/drawing-text-resize.excalidraw").read_bytes()
ORIGINAL = json.loads(SOURCE)
BLUE = (96, 165, 250)
GREEN = (211, 249, 216)


def element(document, identity):
    return next(value for value in document["elements"] if value.get("id") == identity)


def painted_bounds(path, color):
    """Measure actual flat authored fill, excluding toolbar and source words."""
    with Image.open(path).convert("RGB") as image:
        points = [(x, y) for y in range(280, image.height - 50)
                  for x in range(100, image.width - 20)
                  if all(abs(a - b) <= 3 for a, b in zip(image.getpixel((x, y)), color))]
    assert len(points) > 1500, f"authored {color} artwork must be visible"
    return (min(x for x, _ in points), min(y for _, y in points),
            max(x for x, _ in points) + 1, max(y for _, y in points) + 1)


def artwork(session, shot, expected=()):
    def ready():
        session.observe(shot, ["Pencil", *expected], timeout_seconds=45)
        try:
            blue = painted_bounds(session.output / f"{shot}.png", BLUE)
            green = painted_bounds(session.output / f"{shot}.png", GREEN)
            return blue, green
        except AssertionError:
            return False
    result = []
    def observe():
        value = ready()
        if value:
            result[:] = value
        return bool(value)
    wait_for(observe, "both authored filled shapes appear in native drawing output")
    return result


def control(session, shot, word, bounds=(90, 200, 1180, 345)):
    try:
        point = isolated_control(session, shot, word, bounds)
    except RuntimeError:
        point = (session.word(shot, word, required=False) or
                 session.word(shot, word, tree=True))
    session.click(point[0] + 4, point[1])


def mapping(blue):
    """Map authored source coordinates from a real unrotated rectangular fill."""
    box = element(ORIGINAL, "capture")
    scale = (blue[2] - blue[0]) / box["width"]
    def screen(x, y):
        return (round(blue[0] + (x - box["x"]) * scale),
                round(blue[1] + (y - box["y"]) * scale))
    return scale, screen


def preserved(document):
    assert document["files"] == ORIGINAL["files"], "embedded assets stay exact"
    assert document["futureRoot"] == ORIGINAL["futureRoot"]
    assert element(document, "unknown-neighbor") == element(ORIGINAL, "unknown-neighbor")
    image, old_image = element(document, "embedded-image"), element(ORIGINAL, "embedded-image")
    for key in ["fileId", "status", "crop", "scale"]:
        assert image[key] == old_image[key], f"image {key} must remain authored"
    arrow = element(document, "connection")
    old_arrow = element(ORIGINAL, "connection")
    for key in ["startBinding", "endBinding"]:
        assert arrow[key] == old_arrow[key], f"nested {key} fields must survive"
    old_ref = element(ORIGINAL, "capture")["boundElements"][0]
    refs = element(document, "capture")["boundElements"]
    assert next(ref for ref in refs if ref["id"] == "connection") == old_ref, \
        "Add label must not reduce future arrow binding records to id/type"


def save_document(session, path, predicate, reason):
    session.key("s", "ctrl")
    result = []
    def saved():
        document = json.loads(path.read_bytes())
        if predicate(document):
            preserved(document)
            result[:] = [path.read_bytes(), document]
            return True
        return False
    wait_for(saved, reason)
    return tuple(result)


def canvas_focus(session, screen):
    # Empty gap between the blue box and image, below their connecting arrow.
    session.click(*screen(380, 265))


def undo_exact(session, path, screen, expected, shot):
    canvas_focus(session, screen)
    session.key("z", "ctrl")
    session.key("s", "ctrl")
    wait_for(lambda: path.read_bytes() == expected, "shared Undo/Save restores exact prior bytes")
    return artwork(session, shot)


def replace_text(session, shot, value):
    session.observe(shot, ["Drawing text", "Apply text"])
    apply = (session.word(shot, "Apply", minimum_y=280, required=False, last=True) or
             isolated_control(session, shot, "Apply", (700, 280, 1180, 650)))
    # The three-row native TextEdit lies immediately above its Apply button.
    session.click(apply[0] + 120, apply[1] - 40)
    session.key("a", "ctrl")
    private.run_checked(["wtype", value], env=session.env)
    session.observe(shot + "-entered", [value, "Apply text"])
    # The form remains at its user-owned position while this field is edited.
    session.click(apply[0] + 8, apply[1])


def text_edits(session, path, blue):
    _, screen = mapping(blue)
    session.observe("03-text-tool", ["Pencil", "Text"])
    control(session, "03-text-tool", "Text")
    session.click(*screen(325, 350))
    replace_text(session, "04-new-text", "Native note")
    initial_ids = {entry["id"] for entry in ORIGINAL["elements"]}
    created_bytes, created = save_document(
        session, path,
        lambda doc: len(doc["elements"]) == len(ORIGINAL["elements"]) + 1,
        "Text creation reaches the existing guarded Save owner")
    new_text = next(entry for entry in created["elements"] if entry["id"] not in initial_ids)
    assert new_text["type"] == "text" and new_text["text"] == "Native note"
    assert new_text["originalText"] == "Native note" and new_text.get("containerId") is None
    artwork(session, "05-text-created", ["Native note"])
    # Click the actual outlined text, with its saved source position as evidence.
    session.click(*screen(new_text["x"] + 10, new_text["y"] + new_text["height"] / 2))
    replace_text(session, "06-edit-text", "Edited note")
    _, edited = save_document(session, path,
        lambda doc: element(doc, new_text["id"])["text"] == "Edited note",
        "Text editing updates the same source element")
    assert element(edited, new_text["id"])["originalText"] == "Edited note"
    artwork(session, "07-text-edited", ["Edited note"])
    undo_exact(session, path, screen, created_bytes, "08-text-edit-undone")
    undo_exact(session, path, screen, SOURCE, "09-text-create-undone")


def label_binding(session, path, blue):
    _, screen = mapping(blue)
    session.click(*screen(230, 175))
    session.observe("10-label-target", ["Add label", "Pencil"])
    control(session, "10-label-target", "label")
    replace_text(session, "11-add-label", "Bound label")
    _, labelled = save_document(session, path,
        lambda doc: len(element(doc, "capture")["boundElements"]) == 2,
        "A new label is added without dropping existing arrow reference metadata")
    refs = element(labelled, "capture")["boundElements"]
    label_ref = next(ref for ref in refs if ref["type"] == "text")
    label = element(labelled, label_ref["id"])
    assert label["containerId"] == "capture" and label["text"] == "Bound label"
    artwork(session, "12-label-bound", ["Bound label"])
    undo_exact(session, path, screen, SOURCE, "13-label-undone")


@contextmanager
def held_drag(session, start, end, shot, path, before_bytes):
    drag = subprocess.Popen(
        [str(session.pointer), "drag-held", "1280", "800", *map(str, start), *map(str, end)],
        env=session.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True)
    try:
        assert drag.stdout.readline().strip() == "holding private drag"
        session.observe(shot, ["Pencil"], preserve_pointer=True)
        assert drag.poll() is None, "artwork is inspected before pointer release"
        assert path.read_bytes() == before_bytes, "held resize cannot write the source"
        yield shot
    finally:
        drag.communicate("release\n", timeout=12)
        assert drag.returncode == 0


def resize_box(session, path, blue, green):
    scale, screen = mapping(blue)
    session.click(*screen(230, 175))
    artwork(session, "14-resize-selected")
    start = (blue[2], blue[3])
    end = (start[0] + 60, start[1] + 35)
    with held_drag(session, start, end, "15-resize-held", path, SOURCE):
        current = painted_bounds(session.output / "15-resize-held.png", BLUE)
        stationary = painted_bounds(session.output / "15-resize-held.png", GREEN)
        expected = (blue[0], blue[1], blue[2] + 60, blue[3] + 35)
        assert all(abs(a - b) <= 4 for a, b in zip(current, expected)), (blue, current)
        assert all(abs(a - b) <= 2 for a, b in zip(stationary, green))
    _, resized = save_document(session, path,
        lambda doc: element(doc, "capture")["width"] > 200,
        "Released corner resize reaches guarded Save")
    box, original = element(resized, "capture"), element(ORIGINAL, "capture")
    assert abs(box["x"] - original["x"]) < 1e-8 and abs(box["y"] - original["y"]) < 1e-8
    assert abs((box["width"] - original["width"]) * scale - 60) < 4
    assert abs((box["height"] - original["height"]) * scale - 35) < 4
    assert element(resized, "explore") == element(ORIGINAL, "explore")
    assert element(resized, "embedded-image") == element(ORIGINAL, "embedded-image")
    undo_exact(session, path, screen, SOURCE, "16-resize-undone")


def colorful_pixels(path, bounds):
    with Image.open(path).convert("RGB") as image:
        crop = image.crop(tuple(round(value) for value in bounds))
        return sum(max(pixel) - min(pixel) > 70 for pixel in crop.get_flattened_data())


def resize_cropped_image(session, path, blue):
    scale, screen = mapping(blue)
    image = element(ORIGINAL, "embedded-image")
    session.click(*screen(image["x"] + image["width"] / 2,
                          image["y"] + image["height"] / 2))
    artwork(session, "17-image-selected")
    start = screen(image["x"] + image["width"], image["y"] + image["height"])
    end = (start[0] + 24, start[1] + 18)
    top_left = screen(image["x"], image["y"])
    area = (*top_left, end[0] - 2, end[1] - 2)
    before = colorful_pixels(session.output / "17-image-selected.png", area)
    assert before > 1000, "the authored cropped/flipped raster is visible"
    with held_drag(session, start, end, "18-image-resize-held", path, SOURCE):
        during = colorful_pixels(session.output / "18-image-resize-held.png", area)
        assert during > before * 1.15, (before, during)
    _, resized = save_document(session, path,
        lambda doc: element(doc, "embedded-image")["width"] > image["width"] + 5,
        "Resized image preserves its authored crop, flip, resource and file identity")
    changed = element(resized, "embedded-image")
    assert abs(changed["width"] / changed["height"] - image["width"] / image["height"]) < 1e-8
    assert abs((changed["width"] - image["width"]) * scale - 24) < 5
    undo_exact(session, path, screen, SOURCE, "19-image-resize-undone")


def exercise(session, root):
    path = root / "a-text-resize.excalidraw"
    session.observe("01-folder", [path.name])
    session.click(*session.word("01-folder", path.name))
    session.key("Return")
    blue, green = artwork(session, "02-initial")
    # Source minimum x=140, minimum y=120, so export origin is genuinely nonzero.
    assert min(entry["x"] for entry in ORIGINAL["elements"]) == 140
    assert min(entry["y"] for entry in ORIGINAL["elements"]) == 120
    text_edits(session, path, blue)
    blue, green = artwork(session, "09-original-confirmed")
    label_binding(session, path, blue)
    blue, green = artwork(session, "13-original-confirmed")
    resize_box(session, path, blue, green)
    blue, _ = artwork(session, "16-original-confirmed")
    resize_cropped_image(session, path, blue)
    assert path.read_bytes() == SOURCE


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-text-resize.excalidraw").write_bytes(SOURCE)
    return run_application_acceptance(args, output, root, exercise,
        "native text creation/editing, reciprocal label bindings and retained corner/image resize")


if __name__ == "__main__":
    raise SystemExit(main())
