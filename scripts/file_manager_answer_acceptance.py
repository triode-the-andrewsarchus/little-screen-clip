#!/usr/bin/env python3
"""Real local answer columns in the private Weston/Niri compositor."""
from pathlib import Path
import hashlib
import json
import subprocess
import ui_acceptance_capture as private

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_search_acceptance import submit
from file_manager_search_controls_acceptance import type_text


def column_menu(session, shot, title):
    x, y = session.word(shot, title, minimum_y=230)
    private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x), str(y)], env=session.env)
    session.observe(shot + "-menu", ["Column title", "Retry", "missing/failed", "Refresh all answers", "Remove column"])


def exercise_controls(session, root):
    original = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob("*.txt")}
    session.observe("initial", ["Alpha.txt", "Beta.txt", "Gamma.txt"])
    session.click(647, 160)
    session.click(*session.word("initial", "Photos", tree=True, minimum_y=150))
    session.observe("photos-before-question", ["Photos", "Select"])
    command = ':Alpha*;q:"What numeric score is explicitly listed?";type:number'
    submit(session, command)
    session.observe("selector-retains-listing", ["Alpha.txt", "Beta.txt", "Gamma.txt", "10 points", "Excluded"])
    session.key("Escape")
    session.observe("selector-search-closed", ["10 points", "Excluded", "Session answers"])
    column_menu(session, "selector-search-closed", "What")
    _, label_y = session.word("selector-search-closed-menu", "Column", minimum_y=230)
    x, _ = session.word("selector-search-closed-menu", "Column", minimum_y=230)
    session.click(x + 35, label_y + 25)
    type_text(session, "Score")
    session.click(1140, 150)  # Commit the edit and dismiss the menu outside its field.
    session.observe("renamed-header", ["Score", "10 points", "Excluded"])
    submit(session, command)
    session.key("Escape")
    session.observe("duplicate-keeps-renamed-column", ["Score", "10 points", "Excluded"])
    # Zero eligible files make these UI/capacity checks independent of model prose.
    for title in ("One", "Two", "Three"):
        submit(session, f':Missing*;q:"{title}";type:text')
        session.key("Escape")
        session.observe("added-" + title.lower(), ["Score", title, "Excluded", "Session answers"])
    submit(session, command)
    session.key("Escape")
    session.observe("duplicate-focuses-first-of-four", ["Score", "One", "Two", "Three"])
    submit(session, ':Missing*;q:"Fifth";type:text')
    session.observe("fifth-rejected", ["at most four temporary answer columns", "Score", "One", "Two", "Three"])
    session.key("Escape")
    for scope in ("recursive", "indexed"):
        submit(session, f':q:"Outside";scope:{scope}')
        session.observe(scope + "-rejected", ["Answer columns belong to the current folder only", "Score"])
        session.key("Escape")
    session.observe("before-header-actions", ["Score", "One", "Two", "Three", "10 points"])
    shot = "before-header-actions"
    for action, word in (("retry", "Retry"), ("refresh", "Refresh")):
        column_menu(session, shot, "Score")
        session.click(*session.word(shot + "-menu", word, minimum_y=230))
        shot = action + "-completed"
        session.observe(shot, ["Score", "One", "Two", "Three", "10 points"], timeout_seconds=60)
    column_menu(session, shot, "One")
    session.click(*session.word(shot + "-menu", "Remove", minimum_y=230))
    session.observe("column-removed", ["Score", "Two", "Three", "10 points"])
    assert session.word("column-removed", "One", minimum_y=230, required=False) is None
    submit(session, ':Missing*;q:"Fifth";type:text')
    session.key("Escape")
    session.observe("capacity-released", ["Score", "Two", "Three", "Fifth", "10 points"])
    session.address(root / "Nested")
    text = session.observe("other-folder", ["This folder is empty"])
    assert "session answers" not in text.lower()
    session.address(root)
    session.observe("return-restores-four", ["Score", "Two", "Three", "Fifth", "10 points"])
    submit(session, ":*.txt Alpha")
    session.observe("name-filter-before-question", ["Alpha.txt", "1 matching entries"])
    submit(session, command)
    session.observe("question-retains-name-filter", ["Alpha.txt", "1 matching entries", "Score", "10 points"])
    for excluded in ("Beta.txt", "Gamma.txt"):
        assert session.word("question-retains-name-filter", excluded, minimum_y=280, required=False) is None
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    assert original == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.glob("*.txt")}
    (session.output / "column-controls.json").write_text(json.dumps(dict(
        selectors_keep_all_rows=True, existing_name_filter_preserved=True,
        photos_switches_to_list_for_columns=True, duplicate_does_not_consume_slot=True,
        four_column_limit=True, recursive_and_indexed_rejected=True,
        rename_retry_refresh_remove=True, removal_releases_slot=True,
        folder_return_preserves_columns=True, implicit_index=False, source_bytes_unchanged=True,
        scope="Native listing, parser, capacity and header interactions; late-work cancellation is tested separately."), indent=2))


def exercise(session, root):
    session.observe("initial", ["Alpha.txt", "Beta.txt", "Gamma.txt"])
    session.click(647, 160)  # Collapse the Shelf so all three tall text rows remain visible.
    session.click(*session.word("initial", "Alpha.txt", minimum_y=250))
    session.observe("selected-before-query", ["Selection: 1 file"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    submit(session, ':q:"What numeric score is explicitly listed?";type:number')
    session.observe("numeric-ready", ["Alpha.txt", "Beta.txt", "10 points", "2 points", "Session answers"])
    session.key("Escape")
    session.observe("numeric-search-closed", ["Session answers", "10 points", "2 points", "Selection: 1 file"])
    header = session.word("numeric-search-closed", "What", minimum_y=230)
    for phase, order in [("ascending", ["Beta.txt", "Alpha.txt"]),
                         ("descending", ["Alpha.txt", "Beta.txt"])]:
        session.click(*header)
        session.key("c", "ctrl")
        command = ["wl-paste", "--no-newline", "--type", "text/uri-list"]
        wait_for(lambda: subprocess.run(command, env=session.env, capture_output=True, text=True,
                                       timeout=2).stdout.strip() == (root / "Alpha.txt").as_uri(),
                 f"{phase} copy of original selected identity", 5)
        payload = private.run_checked(command, env=session.env)
        assert payload.strip() == (root / "Alpha.txt").as_uri(), payload
        (session.output / f"{phase}-selection.txt").write_text(payload)
        session.observe(phase, ["10 points", "2 points", "Alpha.txt", "Beta.txt", "Selection: 1 file"])
        assert session.word(phase, order[0], minimum_y=280)[1] < session.word(phase, order[1], minimum_y=280)[1]
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
    def configure(parser):
        parser.add_argument("--column-controls", action="store_true", help="Exercise selectors, duplicate/capacity rules and column header actions")
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager", configure)
    output, root = media_fixture_directory(args)
    (root / "Alpha.txt").write_text("Test record. Score: 10 points. Result: failed.")
    (root / "Beta.txt").write_text("Test record. Score: 2 points. Result: passed.")
    (root / "Gamma.txt").write_text("Travel diary: We walked around the lake and photographed the sunset.")
    (root / "Nested").mkdir()

    scenario = exercise_controls if args.column_controls else exercise
    scope = ("Answer-column selectors, duplicate/capacity rules, scope rejection and header actions" if args.column_controls else
             "Typed number/boolean answers, header sorting, session-only columns, folder return and no implicit index")
    return run_application_acceptance(args, output, root, scenario, scope, model_environment)


if __name__ == "__main__":
    raise SystemExit(main())
