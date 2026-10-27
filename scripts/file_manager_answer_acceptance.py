#!/usr/bin/env python3
"""Real local answer columns in the private Weston/Niri compositor."""
from pathlib import Path

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["Alpha.txt", "Beta.txt", "Gamma.txt"])
    session.click(647, 160)  # Collapse the Shelf so all three tall text rows remain visible.
    session.click(*session.word("initial", "Alpha.txt", minimum_y=250))
    session.observe("selected-before-query", ["Selection: 1 file"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    submit(session, ':q:"What numeric score is explicitly listed?";type:number')
    session.observe("numeric-ready", ["Alpha.txt", "Beta.txt", "10 points", "2 points", "Session answers"])
    header = session.word("numeric-ready", "What", minimum_y=230)
    session.click(*header)
    session.observe("ascending", ["10 points", "2 points", "Alpha.txt", "Beta.txt", "Selection: 1 file"])
    alpha = session.word("ascending", "Alpha.txt", minimum_y=280)
    beta = session.word("ascending", "Beta.txt", minimum_y=280)
    assert beta[1] < alpha[1], (alpha, beta)
    session.click(*header)
    session.observe("descending", ["Alpha.txt", "Beta.txt"])
    assert session.word("descending", "Alpha.txt", minimum_y=280)[1] < session.word("descending", "Beta.txt", minimum_y=280)[1]
    submit(session, ':q:"Does the record say the test passed?";type:boolean')
    session.observe("boolean-ready", ["false", "true", "Alpha.txt", "Gamma.txt"])
    session.key("Escape")
    session.observe("closed-search-keeps-columns", ["Session answers", "false", "true"])
    session.address(root / "Nested")
    text = session.observe("other-folder", ["This folder is empty"])
    assert "session answers" not in text.lower()
    session.address(root)
    session.observe("return-restores-columns", ["Session answers", "false", "true", "10 points", "2 points"])
    assert not index.exists(), "Explicit Q/A must not enroll a folder or create a semantic index"


def model_environment(env, children):
    data = Path(env["XDG_DATA_HOME"]) / "little-screen-clip/search"
    data.mkdir(parents=True)
    actual = Path.home() / ".local/share/little-screen-clip/search"
    for name in ("runtime", "models"):
        (data / name).symlink_to(actual / name, target_is_directory=True)


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Alpha.txt").write_text("Test record. Score: 10 points. Result: failed.")
    (root / "Beta.txt").write_text("Test record. Score: 2 points. Result: passed.")
    (root / "Gamma.txt").write_text("Travel diary: We walked around the lake and photographed the sunset.")
    (root / "Nested").mkdir()

    return run_application_acceptance(args, output, root, exercise,
        "Typed number/boolean answers, header sorting, session-only columns, folder return and no implicit index", model_environment)


if __name__ == "__main__":
    raise SystemExit(main())
