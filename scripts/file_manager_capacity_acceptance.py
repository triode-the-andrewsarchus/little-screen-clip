#!/usr/bin/env python3
"""Pause at a real vector limit, retain search, raise it and resume in place."""
from pathlib import Path
import sqlite3
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit
from file_manager_search_settings_acceptance import click_word
import ui_acceptance_capture as private


def exercise(session, root):
    session.observe("initial", ["One.txt", "Two.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    def paused():
        if not index.exists():
            return False
        with sqlite3.connect(index) as db:
            return db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 1
    wait_for(paused, "first completed file at one-vector limit")
    session.click(993, 118)
    session.observe("quota-paused", ["Indexing paused", "vector limit reached"])
    session.key("Escape")
    with sqlite3.connect(index) as db:
        published = Path(bytes(db.execute("SELECT p.path FROM paths p JOIN files f ON f.id=p.file_id WHERE f.state='ready'").fetchone()[0]).decode()).name
        assert db.execute("SELECT generation FROM roots").fetchone()[0] == 1
    submit(session, ':emb:"document"')
    session.observe("search-while-paused", [published, "Semantic candidates"])
    session.key("Escape")
    session.click(151, 70)
    session.observe("edit-menu", ["Search settings"])
    click_word(session, "edit-menu", "Search")
    session.scroll_to_visible("limits-header", ["Index storage limits"], position=(500, 450))
    click_word(session, "limits-header", "storage")
    session.scroll_to_visible("vector-control", ["MiB", "Vectors"], position=(500, 450))
    label = session.word("vector-control", "Vectors", tree=True, required=False, last=True) or session.word("vector-control", "Vectors", last=True)
    session.click(label[0] + 80, label[1])
    session.click(label[0] + 80, label[1])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "2"], env=session.env)
    session.key("Return")
    private.run_checked([str(session.pointer), "scroll", "1280", "800", "500", "450", "-800"], env=session.env)
    session.observe("before-saving-limit", ["Automatic filename-search locations"])
    click_word(session, "before-saving-limit", "Save")
    generation, paths, vectors = inventory(index)
    assert generation == 1 and len(paths) == 2 and vectors == 2, (generation, paths, vectors)
    config = tomllib.loads((session.output / "scratch/config.toml").read_text())
    assert config["file_manager"]["search"]["limits"]["max_vectors"] == 2
    click_word(session, "before-saving-limit", "Close")
    session.click(993, 118)
    session.observe("resumed-in-place", ["2 files", "2 vectors"])


def inventory_budget(session, root):
    session.observe("initial", ["entry_0.rs"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    session.click(993, 118)
    session.observe("inventory-paused-before-model", ["Indexing paused", "storage limit reached"])
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM files").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 0
    assert not (index.parent / "runtime").exists()
    assert sum(path.stat().st_size for path in index.parent.glob("index.sqlite*")) < 16 * 1024 * 1024


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager", lambda parser: parser.add_argument("--inventory-budget", action="store_true"))
    output, root = media_fixture_directory(args)
    if args.inventory_budget:
        for i in range(5000):
            (root / f"entry_{i}.rs").touch()
    else:
        (root / "One.txt").write_text("This document describes small cats and their whiskers.")
        (root / "Two.txt").write_text("This document describes a volcano ejecting lava and ash.")
    def environment(env, children):
        with (output / "scratch/config.toml").open("a") as config:
            config.write("\n[file_manager.search.limits]\nmax_vectors = 1\nmax_mib = 16\n")
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ([] if args.inventory_budget else ["runtime", "models"]):
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)
    return run_application_acceptance(args, output, root, inventory_budget if args.inventory_budget else exercise,
        "Bounded directory inventory rejected before row writes or model loading" if args.inventory_budget else "Real vector quota pause, retained semantic search, persistent raised limit and same-generation continuation", environment)


if __name__ == "__main__":
    raise SystemExit(main())
