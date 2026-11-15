#!/usr/bin/env python3
"""Exercise live source-backed project checkboxes on the native Home desk."""
import json
import uuid
import fcntl
import ui_acceptance_capture as private
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word, enter_home
from file_manager_project_acceptance import add_action


def exercise(session, root):
    todo = root / "TODO.md"
    original = todo.read_bytes()
    home = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/home.json"
    session.ocr_regions = [("contrast-line-tasks-header-ocr.png", (96,96,900,140)),
                           ("contrast-line-tasks-arrange-ocr.png", (400,104,462,131)),
                           ("contrast-line-tasks-fit-ocr.png", (670,104,695,131)),
                           ("contrast-line-tasks-save-error-ocr.png", (98,142,400,162))]
    session.observe("folder", ["README.md", "TODO.md", "Home"])
    enter_home(session, "folder")
    session.observe("home", ["Home", "Add", "Projects"])
    add_action(session, "home", "Live", "area-menu")
    wait_for(lambda: home.exists() and any(card["content"]["kind"] == "projects" for card in json.loads(home.read_text())["board"]["cards"]), "live Projects area persisted")
    session.observe("empty-area", ["Registered projects appear here", "Home"])
    add_action(session, "empty-area", "Register", "register-menu")
    session.observe("live-tasks", ["Garden", "Water herbs", "Plan beds", "Reload TODO"])
    click_word(session, "live-tasks", "Water", minimum_y=200)
    checked = original.replace(b"[ ] **Water", b"[x] **Water")
    wait_for(lambda: todo.read_bytes() == checked, "native checkbox changed only its marker byte")
    assert "Water herbs" not in home.read_text(), "source task content must not be copied into Home layout"
    session.observe("checked-task", ["Garden", "Water herbs", "Plan beds"])
    click_word(session, "checked-task", "Arrange", minimum_y=90, maximum_y=145)
    session.observe("arrange", ["Garden", "Water herbs", "Fit"])
    session.click(1100,700)
    session.key("z", "ctrl")
    wait_for(lambda: not any(card["content"]["kind"] == "projects" for card in json.loads(home.read_text())["board"]["cards"]), "layout undo removed only the live area")
    assert todo.read_bytes() == checked
    private.run_checked(["wtype", "-M", "ctrl", "-M", "shift", "-k", "z", "-m", "shift", "-m", "ctrl"], env=session.env)
    wait_for(lambda: any(card["content"]["kind"] == "projects" for card in json.loads(home.read_text())["board"]["cards"]), "layout redo restored area")
    assert todo.read_bytes() == checked
    session.observe("redone-area", ["Garden", "Water herbs"])
    click_word(session, "redone-area", "Arrange", minimum_y=90, maximum_y=145)
    session.observe("before-failed-save", ["Garden", "Water herbs"])
    with (root / ".fmproject.lock").open("rb") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        click_word(session, "before-failed-save", "Water", minimum_y=200)
        session.observe("failed-save", ["TODO save failed", "Reload task previews"])
        assert todo.read_bytes() == checked, "a rejected write must preserve the real source"
    click_word(session, "failed-save", "Reload", minimum_y=135, maximum_y=240)
    session.observe("failed-save-recovered", ["Garden", "Water herbs"])
    external = b"# Work\r\n\r\n- [ ] Inspect soil\r\n"
    todo.write_bytes(external)
    session.observe("external-edit", ["Garden", "Inspect soil"])
    click_word(session, "external-edit", "Inspect", minimum_y=200)
    wait_for(lambda: todo.read_bytes() == external.replace(b"[ ]", b"[x]"), "refreshed task writes the new source revision")
    todo.unlink()
    session.observe("missing-todo", ["Garden", "No such file", "Reload TODO"])
    assert not todo.exists(), "missing TODO must not be silently recreated by preview"
    restored = b"- [ ] Restored task\n"
    todo.write_bytes(restored)
    session.observe("restored-todo", ["Garden", "Restored task"])
    click_word(session, "restored-todo", "Open", minimum_y=200)
    session.observe("todo-studio", ["Back to folder", "Restored task"])
    session.key("Escape")
    session.observe("studio-return", ["README.md", "TODO.md", "Home"])
    enter_home(session, "studio-return")
    session.observe("returned-desk", ["Garden", "Restored task", "Projects"])
    click_word(session, "returned-desk", "Projects", minimum_y=90, maximum_y=145)
    session.observe("project-menu", ["Garden", "Unregister"])
    click_word(session, "project-menu", "Unregister", minimum_y=135)
    session.observe("unregistered-area", ["Registered projects appear here", "Home"])
    assert todo.read_bytes() == restored
    assert (root / ".fmproject").is_file()


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / ".fmproject").write_text(json.dumps({"version":1, "id":str(uuid.uuid4()), "title":"Garden", "readme":"README.md", "todo":"TODO.md"}))
    (root / "README.md").write_text("# Garden\n")
    (root / "TODO.md").write_bytes(b"# Work\r\n\r\n- [ ] **Water herbs**\r\n- [x] Plan beds\r\n\r\n```md\r\n- [ ] Ignore code\r\n```\r\n")
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Live project tasks, exact source checkbox writes, layout undo independence, external refresh, missing-source recovery and unregister"))
