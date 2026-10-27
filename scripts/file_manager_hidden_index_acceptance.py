#!/usr/bin/env python3
"""Hidden-index policy changes and semantic visibility on private Wayland."""
from pathlib import Path
import sqlite3
import os
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit
from file_manager_search_settings_acceptance import click_word


def toggle_index_hidden(session, enabled, label):
    session.click(151, 70)
    session.observe(label + "-edit", ["Search settings"])
    click_word(session, label + "-edit", "Search")
    session.observe(label + "-settings", ["Include eligible hidden files when indexing"])
    click_word(session, label + "-settings", "Include")
    click_word(session, label + "-settings", "Save")
    config = session.output / "scratch/config.toml"
    wait_for(lambda: tomllib.loads(config.read_text())["file_manager"]["search"]["include_hidden"] == enabled, "persistent hidden indexing policy")
    click_word(session, label + "-settings", "Close")


def exercise(session, root):
    originals = {path: path.read_bytes() for path in root.rglob("*") if path.is_file()}
    session.observe("initial", ["One.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    toggle_index_hidden(session, True, "include")
    assert not index.exists(), "Changing policy must not enroll or create an index"
    session.click(993, 118)
    session.key("Escape")
    generation, paths, vectors = inventory(index)
    assert len(paths) == vectors == 3, (paths, vectors)
    submit(session, ':emb:"cats";scope:recursive')
    session.observe("recursive-hidden-ancestors-filtered", ["One.txt", "1 matching entries"])
    submit(session, ':emb:"cats";scope:indexed')
    session.observe("indexed-hidden-ancestors-filtered", ["One.txt", "1 matching entries"])
    session.click(1130, 118)  # Browser visibility is independent from indexing.
    submit(session, ':emb:"cats";scope:indexed')
    session.observe("show-hidden-indexed", ["Hidden.txt", "top.txt", "3 matching entries"])
    session.key("Escape")
    toggle_index_hidden(session, False, "exclude")
    generation, paths, vectors = inventory(index, generation)
    assert paths == [str(root / "One.txt")] and vectors == 1, (paths, vectors)
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM memberships WHERE verified=0").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM directories").fetchone()[0] == 1
    # An old hidden-directory watch may still deliver a delta. Touch a visible
    # file too so the observed generation proves that notifications settled.
    os.utime(root / ".archive/Hidden.txt", None)
    os.utime(root / "One.txt", None)
    generation, paths, vectors = inventory(index, generation)
    assert paths == [str(root / "One.txt")] and vectors == 1, (paths, vectors)
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM directories").fetchone()[0] == 1
    # Hidden browsing stays on: this proves retired inventory is absent even
    # when result visibility would otherwise permit it.
    submit(session, ':emb:"cats";scope:indexed')
    session.observe("retired-hidden-inventory", ["Hidden: on", "One.txt", "1 matching entries"])
    session.key("Escape")
    toggle_index_hidden(session, True, "reenable")
    generation, paths, vectors = inventory(index, generation)
    assert len(paths) == vectors == 3, (paths, vectors)
    submit(session, ':emb:"cats";scope:indexed')
    session.observe("hidden-index-rebuilt", ["Hidden.txt", "top.txt", "3 matching entries"])
    for path, content in originals.items():
        assert path.read_bytes() == content


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "One.txt").write_text("Cats have whiskers and soft fur.")
    (root / ".archive").mkdir()
    (root / ".archive/Hidden.txt").write_text("Cats rest on a sunny windowsill.")
    (root / ".top.txt").write_text("Cats chase toys and climb trees.")
    (root / ".git").mkdir()
    (root / ".git/secret.txt").write_text("Never embed this repository payload.")
    (root / "credentials.json").write_text("Never embed this credential payload.")
    return run_application_acceptance(args, output, root, exercise,
        "Hidden ancestor filtering, persistent policy retirement/reenablement, no automatic enrollment and unchanged sources", search_environment)


if __name__ == "__main__":
    raise SystemExit(main())
