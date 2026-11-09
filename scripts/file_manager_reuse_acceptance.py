#!/usr/bin/env python3
"""Real embedding reuse after moves, copies and edits in the private compositor."""
import json
import os
from pathlib import Path
import shutil
import sqlite3
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def link_installed_inference(index):
    """Expose installed weights only when the scenario explicitly requests them."""
    index.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    installed = Path.home() / ".local/share/little-screen-clip/search"
    for directory in ["runtime", "models"]:
        (index.parent / directory).symlink_to(installed / directory, target_is_directory=True)


def inventory(index, after=0):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        if index.exists():
            with sqlite3.connect(index) as db:
                row = db.execute("SELECT generation,state,error FROM roots").fetchone()
                if row and row[0] > after:
                    if row[1] == "ready":
                        paths = [bytes(path).decode() for path, in db.execute("SELECT path FROM paths")]
                        vectors = db.execute("SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL").fetchone()[0]
                        return row[0], paths, vectors
                    if row[1] == "error":
                        raise AssertionError(row)
        time.sleep(0.1)
    raise AssertionError("Index did not finish within 90 seconds")


def rebuild(session, index, generation, label):
    session.click(993, 118)
    session.observe(label + "-menu", ["Rebuild index"])
    session.click(980, 272)  # Verified Rebuild button on the fixed private output.
    return inventory(index, generation)


def exercise(session, root):
    session.observe("initial", ["Alpha", "Zulu"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    generation, paths, vectors = inventory(index)
    assert paths == [str(root / "Alpha/one.txt")] and vectors == 1
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM source_digests WHERE length(digest)=32").fetchone()[0] == 1
    (root / "Alpha/one.txt").rename(root / "Zulu/renamed.txt")
    generation, paths, vectors = rebuild(session, index, generation, "move")
    assert paths == [str(root / "Zulu/renamed.txt")] and vectors == 1
    session.click(993, 118)
    session.observe("moved-reuses-vector", ["1 generated", "1 reused"])
    session.key("Escape")
    shutil.copyfile(root / "Zulu/renamed.txt", root / "Zulu/copied.txt")
    generation, paths, vectors = rebuild(session, index, generation, "copy")
    assert len(paths) == 2 and vectors == 2
    session.click(993, 118)
    session.observe("copy-reuses-vector", ["1 generated", "2 reused"])
    session.key("Escape")
    # A source revision changes while its entire byte content remains identical.
    os.utime(root / "Zulu/renamed.txt", None)
    generation, paths, vectors = rebuild(session, index, generation, "metadata-only")
    assert len(paths) == 2 and vectors == 2
    session.click(993, 118)
    session.observe("metadata-only-reuses-vector", ["1 generated", "3 reused"])
    session.key("Escape")
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM source_digests").fetchone()[0] == 2
        assert db.execute("SELECT COUNT(DISTINCT digest) FROM source_digests").fetchone()[0] == 1
    (root / "Zulu/renamed.txt").write_text("A volcano ejects molten lava and ash.")
    generation, paths, vectors = rebuild(session, index, generation, "edit")
    assert len(paths) == 2 and vectors == 2
    session.click(993, 118)
    session.observe("changed-input-generates-vector", ["2 generated", "3 reused"])
    session.key("Escape")
    (root / "Zulu/copied.txt").unlink()
    generation, paths, vectors = rebuild(session, index, generation, "remove")
    assert paths == [str(root / "Zulu/renamed.txt")] and vectors == 1
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM files").fetchone()[0] == 1
        assert db.execute("PRAGMA user_version").fetchone()[0] == 6
    root.rename(session.output / "displaced-root")
    root.mkdir()
    session.click(993, 118)
    session.observe("replacement-menu", ["Rebuild index"])
    session.click(980, 272)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        with sqlite3.connect(index) as db:
            state, available = db.execute("SELECT state,available FROM roots").fetchone()
            if state == "offline":
                assert available == 0
                assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 1
                break
        time.sleep(0.1)
    else:
        raise AssertionError("Replacement root was not marked offline")
    session.click(993, 118)
    session.observe("replacement-requires-enrollment", ["identity changed", "Remove indexed folder"])
    session.key("Escape")
    (session.output / "reuse-results.json").write_text(json.dumps({"moves": True, "copies": True, "changed_input": True, "metadata_only": True, "full_source_digests": True, "orphan_cleanup": True, "replacement_root_rejected": True, "paths": paths}, indent=2))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    for directory in ["Alpha", "Zulu"]:
        (root / directory).mkdir()
    (root / "Alpha/one.txt").write_text("Cats are small domesticated animals with whiskers and soft fur.")

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ["runtime", "models"]:
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "Real CUDA embedding reuse after cross-directory moves and copies, changed-input inference and bounded orphan retirement", environment)


if __name__ == "__main__":
    raise SystemExit(main())
