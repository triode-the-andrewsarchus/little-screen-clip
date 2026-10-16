#!/usr/bin/env python3
"""Basic type-to-navigate on the private Weston/Niri compositor."""
import time
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe("initial", ["41 folders", "Alpha"])
    session.click(*session.word("initial", "Alpha", minimum_y=240))
    private.run_checked(["wtype", "targetx"], env=session.env)
    session.key("BackSpace")
    session.observe("offscreen-match", ["TargetFolder", "1 Folder", "Find: target"])
    session.key("Escape")  # Dismiss the query, preserving its selected folder.
    session.key("Return")
    session.observe("opened-match", ["landmark.txt", "1 file"])

    session.address(root)
    session.observe("returned", ["41 folders"])
    private.run_checked(["wtype", "getfold"], env=session.env)
    session.key("Return")  # Ordinary substring matching uses the same selection.
    session.observe("substring-match", ["landmark.txt", "1 file"])

    session.address(root)
    session.observe("returned-for-expiry", ["41 folders"])
    private.run_checked(["wtype", "target"], env=session.env)
    time.sleep(2.2)
    private.run_checked(["wtype", "alpha"], env=session.env)
    session.key("Return")
    session.observe("expired-query", ["Alpha", "Empty folder"])

    session.address(root)
    session.observe("returned-for-rename", ["41 folders"])
    private.run_checked(["wtype", "target"], env=session.env)
    session.key("F2")
    private.run_checked(["wtype", "draft name"], env=session.env)
    session.observe("rename-focus", ["Rename", "draft"])
    session.key("Escape")
    assert (root / "TargetFolder").is_dir()
    assert not (root / "draft name").exists()
    session.address(root / "TargetFolder")
    session.observe("address-focus", ["landmark.txt", "1 file"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Alpha").mkdir()
    for index in range(39):
        (root / f"Before{index:02}").mkdir()
    target = root / "TargetFolder"
    target.mkdir()
    (target / "landmark.txt").write_text("the selected folder opened")
    return run_application_acceptance(args, output, root, exercise, "Literal filename navigation, reveal, expiry and text focus isolation")


if __name__ == "__main__":
    raise SystemExit(main())
