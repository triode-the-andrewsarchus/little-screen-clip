#!/usr/bin/env python3
"""Persistent search exclusions, retirement and re-inclusion on private Wayland."""
import json
import sqlite3
import tomllib
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)
from file_manager_maintenance_acceptance import search_environment, scroll, word
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit
from file_manager_search_settings_acceptance import click_word


def set_exclusions(session, paths, label, toggle_pause=False):
    session.click(151, 70)
    session.observe(label + "-edit", ["Search settings"])
    click_word(session, label + "-edit", "Search")
    session.observe(label + "-settings", ["Pause background indexing", "Excluded search paths"])
    if toggle_pause:
        click_word(session, label + "-settings", "Pause")
    if not getattr(session, "exclusions_expanded", False):
        click_word(session, label + "-settings", "Excluded")
        session.exclusions_expanded = True
    session.scroll_to_visible(label + "-field", ["Apply with Save preferences above"], position=(500, 450))
    anchor = word(session, label + "-field", "Apply")
    session.click(260, anchor[1] - 40)
    session.key("a", "ctrl")
    session.key("BackSpace")
    private.run_checked(["wtype", "\n".join(str(path) for path in paths)], env=session.env)
    scroll(session, -600)
    session.observe(label + "-save", ["Save preferences"])
    click_word(session, label + "-save", "Save")
    config = session.output / "scratch/config.toml"
    wait_for(lambda: tomllib.loads(config.read_text())["file_manager"]["search"]["excluded_paths"] == [str(path) for path in paths], "persistent exclusion addresses")
    click_word(session, label + "-save", "Close")


def cached_paths(index):
    with sqlite3.connect(index) as db:
        return sorted(bytes(path).decode() for path, in db.execute("SELECT path FROM paths"))


def exercise(session, root):
    originals = {path: path.read_bytes() for path in root.rglob("*") if path.is_file()}
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    session.observe("initial", ["One.txt", "Private"])
    assert not index.exists()
    submit(session, ':content:"cats";scope:recursive')
    session.observe("excluded-from-content", ["One.txt", "Sibling.txt", "2 matching entries"])
    assert not index.exists()
    session.key("Escape")
    session.click(993, 118)
    session.key("Escape")
    generation, paths, vectors = inventory(index)
    expected = sorted([str(root / "One.txt"), str(root / "Private-ish/Sibling.txt")])
    assert sorted(paths) == expected and vectors == 2, (paths, vectors)
    submit(session, ':emb:"cats";scope:recursive')
    session.observe("semantic-before-policy", ["One.txt", "Sibling.txt", "2 matching entries"])
    set_exclusions(session, [root / "Private", root / "One.txt"], "exclude-file", toggle_pause=True)
    # Retire old references even though reindexing is explicitly paused.
    wait_for(lambda: cached_paths(index) == [str(root / "Private-ish/Sibling.txt")], "retire cached excluded paths while paused")
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL").fetchone()[0] == 1
    text = session.observe("old-search-retired", ["Folders", "One.txt"])
    assert "matching entries" not in text.lower(), "Policy changes must retire the prior search view"
    set_exclusions(session, [root / "Private", root / "One.txt"], "resume", toggle_pause=True)
    generation, paths, vectors = inventory(index, generation)
    assert paths == [str(root / "Private-ish/Sibling.txt")] and vectors == 1
    submit(session, ':emb:"cats";scope:indexed')
    text = session.observe("semantic-after-policy", ["Sibling.txt", "1 matching entries"])
    assert "one.txt" not in text.lower() and "secret.txt" not in text.lower()
    session.key("Escape")
    set_exclusions(session, [], "reenable")
    generation, paths, vectors = inventory(index, generation)
    assert vectors == 3 and str(root / "Private/Secret.txt") in paths, (paths, vectors)
    set_exclusions(session, [root], "exclude-root", toggle_pause=True)
    wait_for(lambda: not cached_paths(index), "retire whole root")
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 0
    session.click(993, 118)  # Disabled Index cannot undo a configured exclusion.
    submit(session, ':content:"cats";scope:recursive')
    session.observe("excluded-root-content", ["0 matching entries"])
    for path, content in originals.items():
        assert path.read_bytes() == content


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "One.txt").write_text("cats have whiskers")
    for directory, filename in [("Private", "Secret.txt"), ("Private-ish", "Sibling.txt"), (".git", "secret.txt")]:
        (root / directory).mkdir()
        (root / directory / filename).write_text("cats rest on a windowsill")

    def environment(env, children):
        search_environment(env, children)
        settings = "\n[file_manager.search]\nexcluded_paths = " + json.dumps([str(root / "Private")]) + "\n"
        for config in [output / "scratch/config.toml", Path(env["XDG_CONFIG_HOME"]) / "little-screen-clip/config.toml"]:
            with config.open("a") as stream:
                stream.write(settings)

    return run_application_acceptance(args, output, root, exercise,
        "Exact file/subtree exclusions, component-safe siblings, paused cache retirement, re-inclusion and blocked whole-root enrollment with unchanged sources", environment)


if __name__ == "__main__":
    raise SystemExit(main())
