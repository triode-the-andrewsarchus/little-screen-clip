#!/usr/bin/env python3
"""Exercise retained Miller columns, horizontal scrolling and narrow navigation."""
from pathlib import Path
import ui_acceptance_capture as private
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for


def exercise(session, root):
    session.observe("initial", ["List", "Columns"])
    session.click(600, 160)
    session.click(176, 160)
    session.observe("columns", ["child", "notes.txt", "Name"])
    deep = root / "child" / "second" / "third" / "fourth"
    session.address(deep)
    session.observe("deep-chain", ["second", "third", "fourth", "melody.txt"])
    for _ in range(4):
        session.scroll(700, 420, -900, horizontal=True)
    session.observe("retained-ancestors", ["tmp", "config", "unknown.xyz"])
    for _ in range(4):
        session.scroll(700, 420, 900, horizontal=True)
    session.observe("current-returned", ["melody.txt", "fourth"])
    # The divider to the left of the rich List is 15px left of its Type header.
    before, y = session.word("current-returned", "Type", minimum_y=200)
    private.run_checked([str(session.pointer), "drag", *map(str, session.logical_size),
                         str(before - 15), str(y + 90), str(before + 65), str(y + 90)], env=session.env)
    session.observe("resized-divider", ["melody.txt", "fourth"])
    after, _ = session.word("resized-divider", "Type", minimum_y=200)
    assert after > before + 50, "Dragging the divider must resize its folder column"
    session.click(after - 15, y + 90, delay=0.06)
    session.click(after - 15, y + 90)
    session.observe("reset-divider", ["melody.txt", "fourth"])
    reset, _ = session.word("reset-divider", "Type", minimum_y=200)
    assert abs(reset - before) < 8, "Double-click must restore the default column width"
    resize(session, 760, 700)
    session.observe("narrow-current", ["melody.txt", "fourth"])
    resize(session, 1120, 760)
    session.observe("wide-current", ["melody.txt", "fourth"])
    session.key("Left")
    session.observe("keyboard-parent", ["third", "fourth"])
    session.key("Right")
    session.observe("keyboard-child", ["melody.txt", "fourth"])
    session.key("Left", "alt")
    session.observe("history-back", ["third", "fourth"])
    session.key("Right", "alt")
    session.observe("history-forward", ["fourth", "melody.txt"])
    x, y = session.word("history-forward", "fixture", minimum_y=600, last=True)
    session.click(x + 12, y)
    session.observe("breadcrumb-return", ["child", "notes.txt", "artwork.png"])
    session.key("q", "ctrl")


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


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"), lambda parser: parser.add_argument("--geometry", action="store_true"))
    if args.geometry:
        args.output_size = (1760, 1000)
    output, root = media_fixture_directory(args)
    (root / "unknown.xyz").write_bytes(bytes([0, 255, 1, 128]))
    (root / "child" / "second" / "third" / "fourth").mkdir(parents=True)
    Image.new("RGB", (160, 100), (30, 70, 220)).save(root / "artwork.png")
    (root / "notes.txt").write_text("Musical columns show the selected source.\n")
    (root / "child" / "melody.txt").write_text("Nested instrument notes.\n")
    (root / "child" / "second" / "third" / "fourth" / "melody.txt").write_text("Nested instrument notes.\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Retained Miller columns, horizontal scrolling, history, keyboard navigation and window resizing"))
