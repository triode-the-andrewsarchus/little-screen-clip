#!/usr/bin/env python3
"""Exercise native project creation, registration and explicit relocation."""
import json
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_home_acceptance import click_word, enter_home
from file_manager_phase4_acceptance import click_word as menu_click


def project_menu(session, shot, name):
    click_word(session, shot, "Projects", minimum_y=90, maximum_y=145)
    session.observe(name, ["fixture", "Unregister", "Refresh project list"])


def add_action(session, shot, word, name):
    click_word(session, shot, "Add", minimum_y=90, maximum_y=145)
    session.observe(name, ["Project destination", "Create project in current folder", "Register current folder", "Pin current folder"])
    menu_click(session, name, word, last=True, minimum_y=135, maximum_y=600)


def exercise(session, root, binary):
    session.ocr_regions = [("contrast-line-project-use-ocr.png", (114,257,232,276)),
                           ("contrast-line-project-create-ocr.png", (470,230,575,251)),
                           ("contrast-line-project-back-ocr.png", (106,108,212,129))]
    state = Path(session.env["XDG_STATE_HOME"]) / "little-screen-clip/projects.json"
    saved = lambda: json.loads(state.read_text())["projects"] if state.exists() else []
    session.observe("folder", ["README.md", "TODO.md", "Home"])
    enter_home(session, "folder")
    session.observe("home", ["Home", "Add", "Projects"])
    add_action(session, "home", "Create", "create-menu")
    session.observe("project-form", ["Create a project here", "Missing documents"])
    click_word(session, "project-form", "Create", minimum_y=200, maximum_y=350,
               control_bounds=(470,230,575,251))
    wait_for(lambda: len(saved()) == 1, "created project registry record")
    manifest = json.loads((root / ".fmproject").read_text())
    project_id = manifest["id"]
    (root.parent / "copy" / ".fmproject").write_bytes((root / ".fmproject").read_bytes())
    assert saved()[0]["id"] == project_id
    assert (root / "README.md").read_bytes() == b"# My existing project\r\n"
    assert (root / "TODO.md").read_bytes() == b"# Work\r\n\r\n- [ ] Existing task\r\n"
    session.observe("created", ["Home", "Projects"])
    project_menu(session, "created", "created-list")
    session.key("Escape")
    session.close()
    session.restart(binary, root)
    session.observe("restarted-folder", ["README.md", "TODO.md", "Home"])
    enter_home(session, "restarted-folder")
    session.observe("restarted-home", ["Home", "Projects"])
    project_menu(session, "restarted-home", "restarted-list")
    assert saved()[0]["id"] == project_id
    click_word(session, "restarted-list", "Unregister", minimum_y=135)
    wait_for(lambda: not saved(), "unregistered reference")
    assert json.loads((root / ".fmproject").read_text())["id"] == project_id
    assert (root / "TODO.md").is_file()
    session.observe("unregistered", ["Home", "Projects"])
    add_action(session, "unregistered", "Register", "register-menu")
    wait_for(lambda: len(saved()) == 1, "registered existing project")
    assert saved()[0]["id"] == project_id
    session.observe("registered", ["Home", "Projects", "Back to folder"])
    click_word(session, "registered", "Back", minimum_y=90, maximum_y=145,
               control_bounds=(106,108,212,129))
    session.address(root.parent / "copy")
    session.observe("copy-folder", ["Home", "List"])
    enter_home(session, "copy-folder")
    session.observe("copy-home", ["Home", "Projects"])
    add_action(session, "copy-home", "Register", "copy-register-menu")
    session.observe("location-conflict", ["registered at another location", "Current:", "Chosen:", "Use this location"])
    assert saved()[0]["folder"] == str(root)
    click_word(session, "location-conflict", "Use", minimum_y=250, maximum_y=285,
               control_bounds=(114,257,232,276))
    wait_for(lambda: saved()[0]["folder"] == str(root.parent / "copy"), "explicit relocation persisted")
    assert (root / ".fmproject").is_file()
    assert (root / "TODO.md").read_bytes() == b"# Work\r\n\r\n- [ ] Existing task\r\n"
    session.observe("relocated", ["Home", "Projects"])
    project_menu(session, "relocated", "relocated-list")
    click_word(session, "relocated-list", "fixture", minimum_y=135)
    session.observe("project-folder-opened", ["copy", "Home", "List"])


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "README.md").write_bytes(b"# My existing project\r\n")
    (root / "TODO.md").write_bytes(b"# Work\r\n\r\n- [ ] Existing task\r\n")
    clone = root.parent / "copy"
    clone.mkdir()
    raise SystemExit(run_application_acceptance(args, output, root,
        lambda session, root: exercise(session, root, args.binary),
        "Project creation preserves documents; registry restart, unregister, registration and explicit copied-ID relocation"))
