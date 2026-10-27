#!/usr/bin/env python3
"""Native Q/A on a synthetic document with its answer beyond 24 KiB."""
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit
from file_manager_answer_acceptance import model_environment


def exercise(session, root):
    session.observe("initial", ["Long.txt"])
    session.click(530, 160)
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    submit(session, ':q:"What numeric rating is explicitly listed?";type:number')
    session.observe("late-answer", ["Long.txt", "18 points", "Session answers"])
    assert not index.exists(), "A session question must not enroll the folder"


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    background = "Field notes: The path passes trees, flowers and a quiet pond.\n" * 500
    assert len(background.encode()) > 24 * 1024
    (root / "Long.txt").write_text(background + "\nFinal assessment. Rating: 18 points.\n")

    return run_application_acceptance(args, output, root, exercise,
        "Native typed answer beyond the former 24 KiB cutoff, with no automatic indexing", model_environment)


if __name__ == "__main__":
    raise SystemExit(main())
