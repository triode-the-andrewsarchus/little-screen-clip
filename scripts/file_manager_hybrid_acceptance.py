#!/usr/bin/env python3
"""Real hybrid lexical/semantic search on the private compositor."""
from pathlib import Path
import sqlite3

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["batchneedle_0.rs"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(993, 118)
    session.key("Escape")
    _, paths, vectors = inventory(index)
    assert len(paths) == 105 and vectors == 105
    submit(session, ':emb:"target.rs"')
    session.observe("exact-name-before-body-match", ["target.rs", "Guide.txt", "hybrid filename/keyword/meaning"])
    target = session.word("exact-name-before-body-match", "target.rs", minimum_y=260)
    guide = session.word("exact-name-before-body-match", "Guide.txt", minimum_y=260)
    assert target[1] < guide[1], (target, guide)
    submit(session, ':emb:"ZXQ-9472"')
    session.observe("technical-keyword", ["Technical.txt", "hybrid filename/keyword/meaning"])
    submit(session, ':*.rs;emb:"target.rs"')
    session.observe("hard-type-filter", ["target.rs", "100 of 103 matching entries", "up to 500"])
    submit(session, ':batchneedle*.rs;emb:"batchneedle"')
    session.observe("first-page", ["100 of 102 matching entries", "Load more", "batchneedle_0.rs"])
    session.click(*(session.word("first-page", "Load", required=False) or session.word("first-page", "Load", tree=True)))
    session.observe("second-page", ["102 of 102 matching entries"])
    submit(session, ':batchneedle*.rs;emb:"batchneedle";sort:name:desc')
    session.observe("sorted-first-page", ["100 of 102 matching entries", "batchneedle_101.rs"])
    with sqlite3.connect(index) as db:
        assert db.execute("SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH ?", ('"ZXQ" AND "9472"',)).fetchone()[0] == 1
        assert db.execute("PRAGMA user_version").fetchone()[0] == 5


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "target.rs").write_text('fn main() { println!("Welcome"); }')
    (root / "Guide.txt").write_text("For project setup, open target.rs and review the build guide.")
    (root / "Technical.txt").write_text("The ZXQ-9472 register controls the calibration sequence.")

    for i in range(102):
        (root / f"batchneedle_{i}.rs").write_text("pub fn noop() {}")

    def environment(env, children):
        data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
        data.mkdir(parents=True)
        for directory in ["runtime", "models"]:
            (data / directory).symlink_to(Path.home() / ".local/share/little-screen-clip/search" / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root, exercise,
        "Exact indexed filename priority, technical FTS terms, hard filters and real local semantic retrieval", environment)


if __name__ == "__main__":
    raise SystemExit(main())
