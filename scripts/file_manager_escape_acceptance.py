#!/usr/bin/env python3
"""Check Escape returns from file-backed viewers on private Weston/Niri."""
from pathlib import Path
import shutil
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance

CASES = [("calendar", "harbor.ics"), ("toolpath", "frame.gcode"), ("notebook", "sample.ipynb")]


def exercise(session, root):
    for folder, name in CASES:
        session.address(root / folder)
        shot = f"{folder}-browser"
        session.observe(shot, [name, "Columns"])
        session.click(*session.word(shot, name, minimum_y=210, last=True))
        session.key("Return")
        session.observe(f"{folder}-viewer", ["Back to folder", name])
        session.key("Escape")
        text = session.observe(f"{folder}-returned", [name, "Columns", "Folder previews"])
        assert "Back to folder" not in text, "Escape left the file viewer open"


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    sources = [repo / "crates/little-clip-core/tests/fixtures/calendar/harbor.ics",
               repo / "crates/little-clip-core/tests/fixtures/gcode/frame.gcode",
               repo / "crates/clip-file-manager/tests/fixtures/notebooks/sample.ipynb"]
    for (folder, name), source in zip(CASES, sources):
        (root / folder).mkdir()
        shutil.copyfile(source, root / folder / name)
    return run_application_acceptance(args, output, root, exercise,
        "Escape returns calendar, toolpath and notebook viewers to their existing folders")


if __name__ == "__main__":
    raise SystemExit(main())
