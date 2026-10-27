#!/usr/bin/env python3
"""Search result opening and guarded reveal on the private compositor."""
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance,
)
from file_manager_search_acceptance import submit


def open_result(session, shot):
    session.click(*session.word(shot, "report.txt", minimum_y=190))
    session.key("Return")


def back(session, shot):
    # The header guide repeats "Back"; the verified studio button is below it.
    session.click(155, 118)


def reveal_menu(session, shot, label):
    x, y = session.word(shot, "report.txt", minimum_y=190)
    private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x + 10), str(y)], env=session.env)
    session.observe(label, ["Reveal in folder"])
    session.click(*session.word(label, "Reveal"))


def exercise(session, root):
    session.observe("initial", ["Nested", "ordinary.txt"])
    query = ":*.txt report;scope:recursive"
    submit(session, query)
    session.observe("result", ["report.txt", "1 matching entries"])
    open_result(session, "result")
    session.observe("valid-open", ["Back to folder", "original content"])
    back(session, "valid-open")
    session.observe("return-retains-search", ["report.txt", "1 matching entries", "Explicit search scope"])
    # Remote search results retain their observed revision while this source changes.
    target = root / "Nested/report.txt"
    target.write_text("replacement content must not open from stale results")
    reveal_menu(session, "return-retains-search", "stale-reveal-menu")
    session.observe("reject-stale-reveal", ["Cannot open", "Search result changed", "1 matching entries", "Retry"])
    open_result(session, "reject-stale-reveal")
    text = session.observe("reject-changed-file", ["Back to folder", "Search result changed"])
    assert "replacement content" not in text.lower()
    back(session, "reject-changed-file")
    session.key("F5")
    session.observe("refreshed-search", ["report.txt", "1 matching entries"])
    reveal_menu(session, "refreshed-search", "valid-reveal-menu")
    session.observe("revealed-folder", ["report.txt", "Selection: 1 file"])
    # Same-folder reveal must also verify the result rather than becoming a no-op.
    submit(session, query)
    session.observe("same-folder-result", ["report.txt", "1 matching entries"])
    reveal_menu(session, "same-folder-result", "same-folder-menu")
    session.observe("same-folder-revealed", ["report.txt", "Selection: 1 file"])
    session.address(root)
    session.observe("root-again", ["Nested", "ordinary.txt"])
    submit(session, query)
    session.observe("before-parent-move", ["report.txt", "1 matching entries"])
    moved = session.output / "outside"
    (root / "Nested").rename(moved)
    (root / "Nested").symlink_to(moved, target_is_directory=True)
    open_result(session, "before-parent-move")
    session.observe("reject-parent-alias", ["Back to folder", "Search result changed"])
    back(session, "reject-parent-alias")
    # Return refresh may retire the stale result. Re-querying cannot follow the alias.
    session.key("F5")
    session.observe("alias-excluded", ["0 matching entries", "Explicit search scope"])
    assert not (Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite").exists()


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Nested").mkdir()
    (root / "Nested/report.txt").write_text("original content")
    (root / "ordinary.txt").write_text("ordinary")
    return run_application_acceptance(args, output, root, exercise,
        "Valid studio open, search-preserving return, stale source rejection, guarded reveal and moved-parent alias rejection without enrollment")


if __name__ == "__main__":
    raise SystemExit(main())
