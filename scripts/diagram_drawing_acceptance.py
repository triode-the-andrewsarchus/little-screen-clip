#!/usr/bin/env python3
"""Exercise native whiteboard creation, guarded save and shared source undo."""
import json
from pathlib import Path

from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    private,
    run_application_acceptance,
    wait_for,
)

SOURCE = (Path(__file__).resolve().parents[1] / "docs/examples/diagrams/drawing.excalidraw").read_text()


def exercise(session, root):
    path = root / "a-drawing.excalidraw"
    session.observe("01-folder", [path.name])
    x, y = session.word("01-folder", path.name)
    session.click(x + 20, y)
    session.key("Return")
    session.observe("02-canvas", ["Diagram", "Pencil", "Box"])
    x, y = session.word("02-canvas", "Box", tree=True)
    session.click(x, y)
    # Private runner fixes the native client at 1280×800. This is within the
    # whiteboard below its controls and creates over existing paper if needed.
    private.run_checked(
        [str(session.pointer), "drag", "1280", "800", "900", "490", "1060", "620"],
        env=session.env,
    )
    session.key("s", "ctrl")
    original_count = len(json.loads(SOURCE)["elements"])
    wait_for(lambda: len(json.loads(path.read_text())["elements"]) == original_count + 1,
             "one complete drawing gesture reaches its guarded save")
    saved = json.loads(path.read_text())
    assert saved["elements"][-1]["type"] == "rectangle"
    assert saved["files"] == json.loads(SOURCE)["files"]
    session.observe("03-created", ["Pencil", "Undo"])
    session.key("z", "ctrl")
    session.key("s", "ctrl")
    wait_for(lambda: path.read_text() == SOURCE, "canvas undo restores and saves exact source bytes")
    session.observe("04-undone", ["Pencil", "Redo"])


def main():
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "a-drawing.excalidraw").write_text(SOURCE)
    return run_application_acceptance(args, output, root, exercise, "native whiteboard canvas")


if __name__ == "__main__":
    raise SystemExit(main())
