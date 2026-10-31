#!/usr/bin/env python3
"""Real backup/clear/resume through the private file-manager settings window."""
from pathlib import Path
import hashlib
import sqlite3
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit
from file_manager_search_settings_acceptance import click_word
import ui_acceptance_capture as private


def enroll_folder(session, folder):
    """Navigate, explicitly enroll and wait for a complete durable inventory."""
    session.address(folder)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    session.click(993, 118)
    session.key('Escape')
    return index, inventory(index)


def word(session, shot, text):
    return session.word(shot, text, tree=True, required=False, last=True) or session.word(shot, text, last=True)


def scroll(session, amount):
    private.run_checked([str(session.pointer), "scroll", "1280", "800", "500", "450", str(amount)], env=session.env)


def backup_through_settings(session, root):
    session.click(151, 70)
    session.observe("edit-menu", ["Search settings"])
    click_word(session, "edit-menu", "Search")
    session.scroll_to_visible("maintenance-header", ["Index maintenance"], position=(500, 450))
    click_word(session, "maintenance-header", "maintenance")
    session.scroll_to_visible("backup-destination", ["Backup destination"], position=(500, 450))
    scroll(session, 100)
    session.observe("backup-controls", ["Backup destination"])
    anchor = word(session, "backup-controls", "destination")
    session.click(190, anchor[1] + 64)
    backup = root / "file-search-backup.sqlite"
    wait_for(backup.exists, "atomic backup installation")
    return backup


def exercise(session, root):
    source = root / "One.txt"
    original = source.read_bytes()
    session.observe("initial", ["One.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    generation, _, vectors = inventory(index)
    assert vectors == 1
    submit(session, ':emb:"cats"')
    session.observe("semantic-before-maintenance", ["One.txt", "Semantic candidates"])
    backup = backup_through_settings(session, root)
    with sqlite3.connect(backup) as db:
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 1
    backup_hash = hashlib.sha256(backup.read_bytes()).hexdigest()
    session.scroll_to_visible("backup-completed", ["Index backup saved"], position=(500, 450))
    session.scroll_to_visible("clear-controls", ["Backup destination"], position=(500, 450))
    scroll(session, 100)
    session.observe("clear-controls-visible", ["Backup destination"])
    anchor = word(session, "clear-controls-visible", "destination")
    session.click(200, anchor[1] + 102)
    session.scroll_to_visible("clear-confirmation", ["Source files are unchanged"], position=(500, 450))
    scroll(session, 100)
    session.observe("clear-confirmation-visible", ["Source files are unchanged"])
    anchor = word(session, "clear-confirmation-visible", "Enrolled")
    session.click(200, anchor[1] + 33)
    config_path = session.output / "scratch/config.toml"
    def cleared():
        with sqlite3.connect(index) as db:
            count = db.execute("SELECT COUNT(*) FROM segments").fetchone()[0]
        return count == 0 and tomllib.loads(config_path.read_text())["file_manager"]["search"]["paused"]
    wait_for(cleared, "clear commits and persists pause")
    scroll(session, -800)
    session.observe("cleared", ["Index cleared", "indexing paused"])
    assert source.read_bytes() == original
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    assert (index.parent / "models").exists()
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM files").fetchone()[0] == 0
        assert db.execute("SELECT COUNT(*) FROM roots").fetchone()[0] == 1
        cleared_generation = db.execute("SELECT generation FROM roots").fetchone()[0]
        assert cleared_generation > generation
    click_word(session, "cleared", "Pause")
    click_word(session, "cleared", "Save")
    resumed, paths, vectors = inventory(index, cleared_generation - 1)
    assert resumed == cleared_generation and vectors == 1 and len(paths) == 2, (resumed, paths, vectors)
    assert source.read_bytes() == original
    session.scroll_to_visible("reindexed-after-explicit-resume", ["1 vectors"], position=(500, 450))


def indexed_identity(session, root):
    session.observe("initial", ["One.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    session.click(993, 118)
    session.key("Escape")
    generation, _, vectors = inventory(index)
    assert vectors == 1
    expected_inode = root.stat().st_ino
    session.click(151, 70)
    session.observe("edit-menu", ["Search settings"])
    click_word(session, "edit-menu", "Search")
    session.observe("pause-settings", ["Pause background indexing"])
    click_word(session, "pause-settings", "Pause")
    click_word(session, "pause-settings", "Save")
    config = session.output / "scratch/config.toml"
    wait_for(lambda: tomllib.loads(config.read_text())["file_manager"]["search"]["paused"], "pause before root replacement")
    click_word(session, "pause-settings", "Close")
    root.rename(session.output / "old-fixture")
    root.mkdir()
    (root / "Replacement.txt").write_text("replacement marker in a different folder lifetime")
    assert root.stat().st_ino != expected_inode
    submit(session, ':content:"replacement marker";scope:indexed')
    session.observe("indexed-root-identity-retained", ["0 matching entries", "1 unsupported/incomplete reads"])
    with sqlite3.connect(index) as db:
        inode, available, current = db.execute("SELECT inode,available,generation FROM roots").fetchone()
        assert inode == expected_inode and available == 1 and current == generation
        assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 1


def search_environment(env, children):
    data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
    data.mkdir(parents=True)
    for directory in ["runtime", "models"]:
        (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager", lambda parser: parser.add_argument("--root-identity", action="store_true"))
    output, root = media_fixture_directory(args)
    (root / "One.txt").write_text("Cats have whiskers and soft fur.")
    return run_application_acceptance(args, output, root, indexed_identity if args.root_identity else exercise,
        "Indexed on-demand searches retain enrolled root identity while reconciliation is paused" if args.root_identity else "Self-contained backup, confirmed clear/persistent pause, retained source/models/roots, search retirement and explicit resume", search_environment)


if __name__ == "__main__":
    raise SystemExit(main())
