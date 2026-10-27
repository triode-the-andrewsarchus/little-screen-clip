#!/usr/bin/env python3
"""Native backup restore, persistent pause and explicit resume on private Wayland."""
from pathlib import Path
import hashlib
import sqlite3
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_reuse_acceptance import inventory
from file_manager_maintenance_acceptance import word, scroll, backup_through_settings, search_environment
from file_manager_search_settings_acceptance import click_word
import ui_acceptance_capture as private


def request_restore(session, path, label):
    scroll(session, -800)
    session.scroll_to_visible(label + "-restore-controls", ["Restore source"], position=(500, 450))
    scroll(session, 100)
    session.observe(label + "-restore-address", ["Restore source"])
    anchor = word(session, label + "-restore-address", "source")
    session.click(260, anchor[1] + 28)
    session.key("a", "ctrl")
    private.run_checked(["wtype", str(path)], env=session.env)
    session.click(190, anchor[1] + 64)
    session.scroll_to_visible(label + "-restore-confirmation", ["Configured folders will be kept"], position=(500, 450))
    scroll(session, 100)
    session.observe(label + "-confirm-visible", ["Configured folders will be kept"])
    # The bordered button is omitted by sparse OCR; its verified position is
    # immediately below the final line of the confirmation text.
    anchor = word(session, label + "-confirm-visible", "Configured")
    session.click(190, anchor[1] + 34)


def exercise(session, root):
    session.observe("initial", ["One.txt"])
    source = root / "One.txt"
    original = source.read_bytes()
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    session.click(993, 118)
    session.key("Escape")
    generation, _, vectors = inventory(index)
    assert vectors == 1
    backup = backup_through_settings(session, root)
    backup_hash = hashlib.sha256(backup.read_bytes()).hexdigest()
    config_path = session.output / "scratch/config.toml"
    invalid = session.output / "invalid-backup.sqlite"
    invalid.write_bytes(b"not an index")
    request_restore(session, invalid, "invalid")
    session.scroll_to_visible("invalid-restore-rejected", ["Backup exceeds the index storage limit"], position=(500, 450))
    assert not tomllib.loads(config_path.read_text())["file_manager"]["search"].get("paused", False)
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM segments").fetchone()[0] == 1
    request_restore(session, backup, "valid")
    config_path = session.output / "scratch/config.toml"
    def restored():
        if not tomllib.loads(config_path.read_text())["file_manager"]["search"]["paused"]:
            return False
        with sqlite3.connect(index) as db:
            row = db.execute("SELECT available,generation FROM roots").fetchone()
            count = db.execute("SELECT COUNT(*) FROM segments").fetchone()[0]
            verified = db.execute("SELECT COUNT(*) FROM memberships WHERE verified=1").fetchone()[0]
            return count == 1 and row[1] > generation and not verified
    wait_for(restored, "restore preserves vectors while queuing fresh verification")
    session.scroll_to_visible("restored-outcome", ["Index restored"], position=(500, 450))
    assert source.read_bytes() == original
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    with sqlite3.connect(index) as db:
        assert db.execute("PRAGMA integrity_check").fetchone()[0] == "ok"
        assert db.execute("SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cats'").fetchone()[0] == 1
        assert db.execute("SELECT COUNT(*) FROM roots").fetchone()[0] == 1
        restored_generation = db.execute("SELECT generation FROM roots").fetchone()[0]
        pipeline, vector = db.execute("SELECT pipeline,vector FROM segments").fetchone()
    with sqlite3.connect(backup) as db:
        assert (pipeline, vector) == db.execute("SELECT pipeline,vector FROM segments").fetchone()
    scroll(session, -800)
    session.observe("paused-settings", ["Pause background indexing"])
    click_word(session, "paused-settings", "Pause")
    click_word(session, "paused-settings", "Save")
    resumed, _, vectors = inventory(index, restored_generation - 1)
    assert resumed == restored_generation and vectors == 1
    assert source.read_bytes() == original
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    session.scroll_to_visible("resumed", ["1 vectors"], position=(500, 450))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "One.txt").write_text("Cats have whiskers and soft fur.")
    return run_application_acceptance(args, output, root, exercise,
        "Native restore retains vectors and configured enrollment, persists pause, clears verification and resumes explicitly", search_environment)


if __name__ == "__main__":
    raise SystemExit(main())
