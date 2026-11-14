#!/usr/bin/env python3
"""Exercise initial virtual Home pins and guarded handoffs on private Weston/Niri."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for


def enter_home(session, shot):
    # Styled toolbar labels can be omitted by OCR. The target is paired with
    # the captured 1280x800 toolbar; Home contents are independently asserted.
    point = session.word(shot, "Home", required=False)
    session.click(*(point if point else (995, 118)))


def exercise(session, root, binary):
    state = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/home.json"
    def pins():
        return json.loads(state.read_text())["pins"] if state.exists() else []
    session.observe("folder", ["note.txt", "List"])
    session.click(600, 160)
    session.observe("selected-file", ["note.txt", "Name"])
    x, y = session.word("selected-file", "note.txt", minimum_y=240)
    session.click(x + 10, y)
    enter_home(session, "selected-file")
    session.observe("empty-home", ["Home", "Back to folder", "No pins yet"])
    session.click(308, 197)  # Captured Pin selection button; state effect asserted below.
    wait_for(lambda: len(pins()) == 1, "selected file pin saved")
    session.observe("file-pin", ["note.txt"])
    session.click(165, 197)  # Captured Pin current folder button.
    wait_for(lambda: len(pins()) == 2, "current folder pin saved")
    session.observe("two-pins", ["note.txt", "fixture"])
    ids = [pin["id"] for pin in pins()]
    session.close()
    session.restart(binary, root)
    session.observe("restarted-folder", ["note.txt", "Home"])
    enter_home(session, "restarted-folder")
    session.observe("restored-home", ["note.txt", "fixture"])
    assert [pin["id"] for pin in pins()] == ids
    # File opening first obtains its real parent listing, then opens the studio.
    session.click(168, 251)  # Captured first pin target; studio contents asserted.
    session.observe("pinned-text-studio", ["Back to folder", "Home fixture text"])
    session.key("Escape")
    session.observe("studio-return", ["note.txt", "List", "Home"])
    enter_home(session, "studio-return")
    session.observe("before-unpin", ["note.txt"])
    session.click(245, 251)  # Captured first pin Unpin action; saved IDs asserted.
    wait_for(lambda: len(pins()) == 1, "unpin persisted")
    assert (root / "note.txt").read_text() == "Home fixture text\n"
    assert pins()[0]["id"] == ids[1]
    session.observe("folder-pin-only", ["fixture", "1 pinned items"])
    session.click(168, 251)
    session.observe("pinned-folder-opened", ["note.txt", "List", "Home"])
    enter_home(session, "pinned-folder-opened")
    session.observe("before-home-escape", ["fixture", "Back to folder"])
    session.key("Escape")
    session.observe("home-escape", ["note.txt", "List", "Home"])
    assert sorted(p.name for p in root.iterdir()) == ["note.txt"], "Home must not create content sidecars"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "note.txt").write_text("Home fixture text\n")
    raise SystemExit(run_application_acceptance(args, output, root,
        lambda session, root: exercise(session, root, args.binary),
        "Virtual Home file/folder pins, restart identity, guarded file studio handoff, non-destructive unpin and Escape"))
