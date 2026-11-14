#!/usr/bin/env python3
"""Check remembered folder views and inheritance across real application restarts."""
import ui_acceptance_capture as private
from pathlib import Path
import hashlib
import json
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import click_word


def menu(session, shot, name):
    session.click(190, 71)  # Existing main View menu.
    session.observe(name + "-view", ["Folder view"])
    x, y = session.word(name + "-view", "Folder", tree=True, required=False) or session.word(name + "-view", "Folder")
    session.click(x + 5, y)
    session.observe(name, ["Current view:"])


def action(session, shot, word, offset):
    # Full-window OCR can omit labels inside styled buttons. Pair the fallback
    # with the observed menu's Current row; exact state-file effects are checked.
    point = session.word(shot, word, required=False)
    if point:
        session.click(point[0] + 5, point[1])
    else:
        x, y = session.word(shot, "Current", tree=True, required=False) or session.word(shot, "Current")
        session.click(x + 100, y + offset)


def exercise(session, root, binary, chosen_view="photos"):
    expected_view = ["a-photo.png", "Columns"] if chosen_view == "columns" else ["Thumbnail size"]
    original = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
                for p in root.rglob("*") if p.is_file()}
    state = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/folder-views.json"

    def preference(path):
        if not state.exists():
            return None
        return next((item["preference"] for item in json.loads(state.read_text())["folders"]
                     if item["path"] == str(path)), None)

    session.observe("00-list", ["child", "a-photo.png"])
    assert not state.exists(), "Browsing alone must not create folder preferences"
    session.click(600, 160)  # Collapse Shelf while the initial List toolbar is observed.
    session.click(176 if chosen_view == "columns" else 265, 160)  # Observed shared view tabs.
    session.observe("01-photos", expected_view)
    menu(session, "01-photos", "02-remember-menu")
    action(session, "02-remember-menu", "Remember", 56)
    wait_for(lambda: preference(root) == {"view": chosen_view, "descendants": False}, "remembered root photo view")
    session.key("Escape")
    session.observe("03-remembered", expected_view)
    menu(session, "03-remembered", "04-inherit-menu")
    session.observe("04-inherit-menu", [f"Saved here: {chosen_view.title()}", "subfolders"])
    x, y = session.word("04-inherit-menu", "subfolders")
    session.click(x + 5, y)
    wait_for(lambda: preference(root) == {"view": chosen_view, "descendants": True}, "explicit inherited root view")
    session.key("Escape")
    session.address(root / "child")
    session.observe("05-child-inherits", expected_view)
    session.click(118, 160)
    session.observe("06-child-list", ["Name", "Modified", "child-image.png"])
    menu(session, "06-child-list", "07-child-override")
    action(session, "07-child-override", "Remember", 56)
    wait_for(lambda: preference(root / "child") == {"view": "list", "descendants": False}, "saved child override")
    session.key("Escape")
    session.close()
    session.restart(binary, root / "child")
    session.observe("08-restarted-override", ["Name", "Modified", "child-image.png"])
    menu(session, "08-restarted-override", "09-reset-menu")
    session.observe("09-reset-menu", ["Saved here: List"])
    action(session, "09-reset-menu", "inherited/default", 122)
    wait_for(lambda: preference(root / "child") is None, "removed child override")
    session.key("Escape")
    session.observe("10-inheritance-restored", expected_view)
    session.close()
    session.restart(binary, root / "child")
    session.observe("11-restarted-inheritance", expected_view)
    # Exercise ordinary compositor close on the last restarted client. wtype's
    # first modified key in a fresh client can retain the previous keymap (GDB
    # observed Ctrl+Escape for Ctrl+Q). Earlier closes exercise the shortcut.
    windows = private.own_windows(session.niri, session.env, {session.process.pid})
    assert len(windows) == 1
    private.run_checked([str(session.niri), "msg", "action", "close-window", "--id", str(windows[0]["id"])], env=session.env)
    session.process.wait(timeout=5)
    assert session.process.returncode == 0
    assert preference(root) == {"view": chosen_view, "descendants": True}
    assert {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file()} == original


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"), lambda parser: parser.add_argument("--columns", action="store_true"))
    output, root = media_fixture_directory(args)
    (root / "child").mkdir()
    Image.new("RGB", (120, 80), "#397c94").save(root / "a-photo.png")
    Image.new("RGB", (120, 80), "#d2ad66").save(root / "child/child-image.png")
    (root / "notes.txt").write_text("Folder view acceptance.\n")
    raise SystemExit(run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, args.binary, "columns" if args.columns else "photos"),
        "Saved parent view, explicit descendant inheritance, child override/reset, restart persistence and unchanged content folders"))
