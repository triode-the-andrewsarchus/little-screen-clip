#!/usr/bin/env python3
"""Check compact Files chrome and selection totals on private Weston/Niri."""
from pathlib import Path

from file_manager_acceptance import (
    application_arguments,
    media_fixture_directory,
    run_application_acceptance,
)


def exercise(session, root):
    text = session.observe("compact-browser", [
        "File", "Edit", "View", "Shelf", "Clear held files",
        "2 folders", "2 files (10.5KB)", "Ctrl+L", "Ctrl+Q",
    ])
    for removed in ["New folder", "New file", "Select all", "F5: refresh"]:
        assert removed.lower() not in text.lower(), removed
    session.move_pointer_to_title = lambda: None
    for menu in ["File", "Edit", "View"]:
        session.click(*session.word("compact-browser", menu, tree=True))
        session.observe(f"menu-{menu.lower()}", [{"View": "Audio", "Edit": "No actions available", "File": "No actions yet"}[menu]])
        session.key("Escape")
    session.click(*session.word("compact-browser", "payload", minimum_y=150))
    session.observe("one-file", ["Selection: 1 file (10.0KB)"])
    session.key("a", "ctrl")
    session.observe("all-selected", ["Selection: 2 folders", "2 files (10.5KB)"])
    session.key("Escape")
    session.observe("selection-cleared", ["2 folders", "2 files (10.5KB)"])
    session.click(*session.word("selection-cleared", "alpha", minimum_y=150))
    session.observe("one-folder", ["1 Folder"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "alpha").mkdir()
    (root / "beta").mkdir()
    (root / "payload").write_bytes(bytes(10_000))
    (root / "small").write_bytes(bytes(500))
    return run_application_acceptance(args, output, root, exercise, "compact file-manager chrome and status summaries")


if __name__ == "__main__":
    raise SystemExit(main())
