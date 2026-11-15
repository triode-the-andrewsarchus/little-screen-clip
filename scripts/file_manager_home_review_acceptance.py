#!/usr/bin/env python3
"""Verify the editable review desk against real project sources and native ink."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import artwork_target, click_word, drag
from file_manager_home_review import create
import ui_acceptance_capture as private


def exercise(session, root, binary):
    session.observe("initial", ["Home"])
    session.close()
    state_home = Path(session.env["XDG_STATE_HOME"])
    desk = root / "desk"
    original = create(desk, state_home=state_home)
    home = state_home / "little-screen-clip/home.json"
    session.restart(binary)
    session.observe("review-desk", ["My little field desk", "Field recorder", "Courtyard garden", "Pack spare batteries", "Sketch the planting beds", "Click the ink"])
    todo = desk / "projects/field-recorder/TODO.md"
    before = todo.read_bytes()
    click_word(session, "review-desk", "Pack", minimum_y=150)
    checked = before.replace(b"[ ] Pack", b"[x] Pack")
    wait_for(lambda: todo.read_bytes() == checked, "review desk edits the real checkbox marker")
    assert "Pack spare batteries" not in home.read_text()
    session.observe("checked", ["Field recorder", "Courtyard garden"])
    external = checked + b"- [ ] External addition\n"
    todo.write_bytes(external)
    session.observe("external-refresh", ["External addition"])
    wait_for(lambda: json.loads(home.read_text())["board"]["view"], "initial camera persisted")
    board = json.loads(home.read_text())["board"]
    drag(session, (1100, 700), (1135, 680))
    session.observe("primary-drag-still", ["Field recorder", "Courtyard garden"])
    assert json.loads(home.read_text())["board"]["view"] == board["view"], "primary drag must not pan"
    private.run_checked([str(session.pointer), "middle-drag", *map(str, session.logical_size),
                         "1100", "700", "1135", "680"], env=session.env)
    wait_for(lambda: json.loads(home.read_text())["board"]["view"] != board["view"], "middle drag pans the composition")
    board = json.loads(home.read_text())["board"]
    session.observe("middle-drag-panned", ["Field recorder", "Courtyard garden"])
    notebook = next(element for element in json.loads(board["drawing"])["elements"] if element["id"] == "notebook")
    target = []
    def ink():
        point = artwork_target(session, "notebook-ink", notebook, board["view"])
        if point:
            target[:] = point
        return point
    wait_for(ink, "real notebook ink at the current camera")
    session.click(*target)
    session.observe("notebook-folder", ["ideas.md", "field-notes.md", "Shown"])
    session.key("Left", "alt")
    session.observe("home-return", ["My little field desk", "External addition"])
    session.close()
    session.restart(binary)
    session.observe("restart", ["My little field desk", "External addition", "Courtyard garden"])
    assert todo.read_bytes() == external
    assert json.loads(home.read_text())["board"]["view"] == board["view"]
    assert json.loads(home.read_text())["board"]["drawing"] == original["board"]["drawing"], "theme adaptation must never rewrite source colors"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    raise SystemExit(run_application_acceptance(args, output, root,
        lambda session, root: exercise(session, root, args.binary),
        "Real review desk, source-backed tasks, external refresh, clickable vector ink, visibility and restart"))
