#!/usr/bin/env python3
"""Search and real local embedding indexing on the private Weston/Niri runner."""
import sqlite3
import time
import tomllib
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_reuse_acceptance import inventory
from file_manager_acceptance import click_search_settings_word as click_word
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)


def submit(session, command):
    session.key("f", "ctrl")
    session.key("a", "ctrl")
    private.run_checked(["wtype", command], env=session.env)
    session.key("Return")


def pause_indexing(session, paused, label):
    session.click(151, 70)
    session.observe(label + "-edit", ["Search settings"])
    click_word(session, label + "-edit", "Search")
    session.observe(label + "-settings", ["Pause background indexing"])
    click_word(session, label + "-settings", "Pause")
    click_word(session, label + "-settings", "Save")
    config = session.output / "scratch/config.toml"
    from file_manager_acceptance import wait_for
    wait_for(lambda: tomllib.loads(config.read_text())["file_manager"]["search"]["paused"] == paused, "persist pause/resume")
    click_word(session, label + "-settings", "Close")


def exercise(session, root):
    session.observe("initial", ["cats.txt", "bank.txt", "Nested"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists(), "No default root may create an index"
    submit(session, ':content:"domesticated animals"')
    session.observe("content-match", ["cats.txt", "1 matching entries"])
    session.key("Escape")
    session.observe("restored", ["cats.txt", "bank.txt", "Nested"])
    submit(session, ':*.txt landmark;scope:recursive')
    session.observe("recursive-match", ["landmark.txt", "1 matching entries"])
    session.key("Escape")
    session.observe("before-enrollment", ["cats.txt", "bank.txt"])
    session.click(993, 118)  # Index button in the runner's fixed 1280×800 output.
    session.key("Escape")  # Close status popup; accepted enrollment continues.
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        if index.exists():
            with sqlite3.connect(index) as db:
                roots = db.execute("SELECT state,error FROM roots").fetchall()
                if roots and roots[0][0] == "ready":
                    files = db.execute("SELECT COUNT(*) FROM files").fetchone()[0]
                    vectors = db.execute("SELECT COUNT(*) FROM segments WHERE length(vector)=1536").fetchone()[0]
                    assert files == 3, (files, roots)
                    assert vectors >= 3, vectors
                    break
                if roots and roots[0][0] == "error":
                    raise AssertionError(roots)
        time.sleep(0.1)
    else:
        raise AssertionError("Folder did not finish indexing within 90 seconds")
    submit(session, ':emb:"small domesticated animals with whiskers"')
    session.observe("semantic-match", ["cats.txt", "Semantic candidates"])
    cats = session.word("semantic-match", "cats.txt", minimum_y=190)
    bank = session.word("semantic-match", "bank.txt", minimum_y=190)
    assert cats[1] < bank[1], (cats, bank)
    pause_indexing(session, True, "pause-before-change")
    with sqlite3.connect(index) as db:
        generation = db.execute("SELECT generation FROM roots").fetchone()[0]
    (root / "cats.txt").write_text("This source has changed since indexing.")
    submit(session, ':emb:"small domesticated animals with whiskers"')
    text = session.observe("stale-source-omitted", ["Semantic candidates"], expected_any=("0 matching entries", "1 matching entries"))
    assert "cats.txt" not in text.lower(), "An old indexed revision must not survive source verification"
    # Event invalidation may omit the whole parent before the query, or source
    # verification may omit just cats.txt. Both correctly reject stale content.
    session.key("Escape")
    session.key("F5")
    session.observe("final", ["cats.txt", "bank.txt", "Nested", "39 B"])
    submit(session, ':q:"Does this document contain the phrase financial transactions?";type:boolean')
    session.observe("answer-role", ["Session answers", "true", "false", "cats.txt", "bank.txt"])
    pause_indexing(session, False, "resume-after-answers")
    inventory(index, generation)
    submit(session, ':emb:"financial transactions"')
    session.observe("return-to-embedding-role", ["bank.txt", "Semantic candidates"])


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "cats.txt").write_text("Cats are small domesticated animals with whiskers and soft fur.")
    (root / "bank.txt").write_text("A bank statement reports balances and financial transactions.")
    (root / "Nested").mkdir()
    (root / "Nested/landmark.txt").write_text("A mountain trail reaches a rocky summit.")
    (root / ".git").mkdir()
    (root / ".git/secret.txt").write_text("must not enter the index")
    (root / "credentials.json").write_text("must not enter the index")

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        (data / "runtime").symlink_to(Path.home() / ".local/share/little-screen-clip/search/runtime", target_is_directory=True)
        (data / "models").symlink_to(Path.home() / ".local/share/little-screen-clip/search/models", target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "Literal content, recursive scope, guarded enrollment, persisted vectors and real semantic ranking", environment)


if __name__ == "__main__":
    raise SystemExit(main())
