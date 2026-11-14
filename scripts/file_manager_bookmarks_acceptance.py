#!/usr/bin/env python3
"""Shared GTK bookmarks, sidebar layout, menu gating and native reference drops."""
import subprocess
import time
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import Session, application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import click_word
from file_manager_columns_acceptance import resize


def open_edit(session, shot, expected):
    click_word(session, shot, "Edit", minimum_y=40, maximum_y=90)
    name = shot + "-edit"
    session.observe(name, [expected])
    return name


def add_selected(session, shot, plural=False):
    menu = open_edit(session, shot, "Add bookmarks" if plural else "Add bookmark")
    click_word(session, menu, "Add", minimum_y=90, maximum_y=200)


def drag(session, source, target):
    session.click(*source)
    private.run_checked([str(session.pointer), "drag", *map(str, session.logical_size), *map(str, (*source, *target))], env=session.env)


def exercise(session, root):
    session.move_pointer_to_title = lambda: None
    shared = session.output / "config/gtk-3.0/bookmarks"
    text = lambda: shared.read_text() if shared.exists() else ""
    uri = lambda name: (root / name).as_uri()
    # Folder rows now include square entry targets. Give this sidebar-growth
    # scenario enough height for all fixture rows and the open Shelf.
    resize(session, 1104, 1030)
    time.sleep(1)  # Let compositor resize/move animation settle before OCR hit targets.
    session.observe("empty", ["ALPHA", "BETA", "GAMMA", "DELTA", "note.txt"])
    assert session.word("empty", "Bookmarks", tree=True, minimum_y=190, required=False) is None
    session.click(*session.word("empty", "ALPHA", minimum_y=240))
    session.observe("one-selected", ["1 Folder"])
    add_selected(session, "one-selected")
    wait_for(lambda: uri("ALPHA") in text(), "shared GTK bookmark written")
    session.observe("one-bookmark", ["Bookmarks", "ALPHA"])
    _, one_title_y = session.word("one-bookmark", "Bookmarks", tree=True, minimum_y=190)
    # A bookmark click navigates, then history restores the browser selection.
    session.click(*session.word("one-bookmark", "ALPHA", tree=True, minimum_y=one_title_y))
    session.observe("bookmark-navigation", ["Empty folder"])
    session.key("Left", "alt")
    session.observe("returned", ["ALPHA", "BETA", "GAMMA"])
    session.click(*session.word("returned", "BETA", minimum_y=240))
    gamma = session.word("returned", "GAMMA", minimum_y=240)
    modifier = subprocess.Popen(["wtype", "-M", "ctrl", "-s", "1200", "-m", "ctrl"], env=session.env)
    time.sleep(0.1)
    session.click(*gamma)
    modifier.wait(timeout=3)
    session.observe("two-selected", ["2 Folders"])
    add_selected(session, "two-selected", plural=True)
    wait_for(lambda: uri("BETA") in text() and uri("GAMMA") in text(), "batch bookmark additions")
    session.observe("three-bookmarks", ["Bookmarks", "BETA", "GAMMA"])
    _, three_title_y = session.word("three-bookmarks", "Bookmarks", tree=True, minimum_y=190)
    assert three_title_y < one_title_y - 40, "bookmarks must grow upward from the Shelf"
    session.click(*session.word("three-bookmarks", "note.txt", minimum_y=240))
    session.key("a", "ctrl")
    # Select All includes the visible unreadable descendant in DELTA's strip.
    session.observe("mixed-selection", ["5 folders", "1 file"])
    menu = open_edit(session, "mixed-selection", "No actions available")
    session.key("Escape")
    session.key("Escape")
    session.observe("before-drag", ["Bookmarks", "DELTA"])
    _, title_y = session.word("before-drag", "Bookmarks", tree=True, minimum_y=190)
    drag(session, session.word("before-drag", "DELTA", minimum_y=240), (185, title_y + 42))
    wait_for(lambda: uri("DELTA") in text(), "native shortcut drop")
    assert (root / "DELTA/unreadable").is_dir(), "shortcut drops must not remove source folders"
    Session.move_pointer_to_title(session)
    session.observe("four-bookmarks", ["Bookmarks", "DELTA"])
    # Only the shortcut is removed, not its payload folder or other bookmarks.
    x, y = session.word("four-bookmarks", "BETA", tree=True, minimum_y=title_y + 20, last=True)
    private.run_checked([str(session.pointer), "right-click", *map(str, session.logical_size), str(x + 10), str(y)], env=session.env)
    session.observe("remove-menu", ["Remove shortcut"])
    click_word(session, "remove-menu", "Remove")
    wait_for(lambda: uri("BETA") not in text(), "shortcut removed from GTK file")
    assert (root / "BETA").is_dir()
    assert uri("DELTA") in text() and uri("ALPHA") in text()
    # Simulate an atomic replacement from Nautilus/Thunar, including a custom label.
    external = session.output / "external"
    external.mkdir()
    staged = shared.with_suffix(".new")
    staged.write_text(text() + external.as_uri() + " EXTERNALBOOKMARK\n")
    staged.replace(shared)
    Session.move_pointer_to_title(session)
    session.observe("external-update", ["EXTERNALBOOKMARK", "Bookmarks"])
    shared.write_text("")

    def empty_again():
        session.capture_text("empty-again", session.output / "empty-again.png", [])
        return session.word("empty-again", "Bookmarks", tree=True, minimum_y=190, required=False) is None

    wait_for(empty_again, "empty bookmark section disappears")
    # The invisible lower edge of the folder pane accepts the first shortcut.
    _, shelf_y = session.word("empty-again", "Shelf", tree=True, minimum_y=400)
    drag(session, session.word("empty-again", "DELTA", minimum_y=240), (185, shelf_y - 40))
    wait_for(lambda: uri("DELTA") in text(), "first shortcut added by drag into empty section")
    session.observe("restored-by-drop", ["Bookmarks", "DELTA"])
    assert (root / "DELTA").is_dir()


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/debug/clip-file-manager")
    args.output_size = (1280, 1100)
    output, root = media_fixture_directory(args)
    for name in ["ALPHA", "BETA", "GAMMA", "DELTA"]:
        (root / name).mkdir()
    (root / "note.txt").write_text("ordinary file")
    unreadable = root / "DELTA/unreadable"
    unreadable.mkdir()
    unreadable.chmod(0)
    try:
        return run_application_acceptance(args, output, root, exercise, "GTK bookmark sharing, adaptive sidebar and copy-only native shortcut drops")
    finally:
        unreadable.chmod(0o700)


if __name__ == "__main__":
    raise SystemExit(main())
