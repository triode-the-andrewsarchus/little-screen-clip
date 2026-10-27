#!/usr/bin/env python3
"""Live incremental indexing on the private Weston/Niri runner."""
import json
from pathlib import Path
import sqlite3
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory


def directory_generations(index):
    with sqlite3.connect(index) as db:
        return {bytes(path).decode(): generation for path, generation in db.execute("SELECT path,generation FROM directories")}


def exercise(session, root):
    session.observe("initial", ["Alpha", "Quiet", "Zulu"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    generation, paths, vectors = inventory(index)
    assert len(paths) == vectors == 3
    original = directory_generations(index)
    (root / "Alpha/one.txt").write_text("A volcano ejects molten lava and ash.")
    generation, paths, vectors = inventory(index, generation)
    changed = directory_generations(index)
    assert changed[str(root / "Alpha")] > original[str(root / "Alpha")]
    for directory in [root, root / "Quiet", root / "Zulu"]:
        assert changed[str(directory)] == original[str(directory)], (directory, original, changed)
    session.click(993, 118)
    session.observe("automatic-edit", ["4 generated", "0 reused"])
    session.key("Escape")
    (root / "Alpha/one.txt").rename(root / "Zulu/renamed.txt")
    generation, paths, vectors = inventory(index, generation)
    assert str(root / "Zulu/renamed.txt") in paths
    assert str(root / "Alpha/one.txt") not in paths
    assert vectors == 3
    session.click(993, 118)
    session.observe("automatic-move-reuses-vector", ["4 generated", "1 reused"])
    session.key("Escape")
    new = root / "Alpha/New"
    new.mkdir()
    (new / "new.txt").write_text("A sailboat crosses a quiet lake.")
    generation, paths, vectors = inventory(index, generation)
    assert str(new / "new.txt") in paths and vectors == 4
    (root / "Zulu/renamed.txt").unlink()
    generation, paths, vectors = inventory(index, generation)
    assert str(root / "Zulu/renamed.txt") not in paths and vectors == 3
    assert directory_generations(index)[str(root / "Quiet")] == original[str(root / "Quiet")]
    root.rename(session.output / "displaced-root")
    root.mkdir()
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        with sqlite3.connect(index) as db:
            state, available = db.execute("SELECT state,available FROM roots").fetchone()
            if state == "offline":
                assert available == 0
                assert db.execute("SELECT COUNT(*) FROM files").fetchone()[0] == 3
                break
        time.sleep(0.1)
    else:
        raise AssertionError("Root replacement was not detected automatically")
    session.click(993, 118)
    session.observe("automatic-root-replacement", ["identity changed", "Remove indexed folder"])
    session.key("Escape")
    (session.output / "monitor-results.json").write_text(json.dumps({
        "automatic_edit": True, "automatic_move_reuse": True, "new_subdirectory": True,
        "automatic_delete": True, "unrelated_directories_unchanged": True,
        "root_replacement_offline": True, "original_generations": original,
        "first_edit_generations": changed,
    }, indent=2))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    for directory, text in [("Alpha", "Cats have whiskers."), ("Quiet", "A guitar has strings."), ("Zulu", "A mountain trail reaches a rocky summit.")]:
        (root / directory).mkdir()
        (root / directory / "one.txt").write_text(text)

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ["runtime", "models"]:
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "Automatic edits, move reuse, new directories, deletion and replaced roots without manual rebuilds or rescanning unrelated folders", environment)


if __name__ == "__main__":
    raise SystemExit(main())
