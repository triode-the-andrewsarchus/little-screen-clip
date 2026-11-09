#!/usr/bin/env python3
"""Persistent search preferences through the private compositor, without indexing."""
from pathlib import Path
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_search_acceptance import submit
from file_manager_search_controls_acceptance import type_text
from file_manager_acceptance import click_search_settings_word as click_word
import ui_acceptance_capture as private




def exercise(session, root):
    session.observe("initial", ["sample.txt"])
    index = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite"
    assert not index.exists()
    session.click(151, 70)
    session.observe("edit-menu", ["Search settings"])
    click_word(session, "edit-menu", "Search")
    session.observe("settings", ["Pause background indexing", "Keep the last-used model loaded", "Excluded search paths"])
    click_word(session, "settings", "Pause")
    click_word(session, "settings", "Keep")
    location = session.word("settings", "Automatic", required=False) or session.word("settings", "Automatic", tree=True)
    field = (location[0] + 60, location[1] + 85)
    session.click(*field)
    type_text(session, "/proc")
    click_word(session, "settings", "Save")
    session.observe("protected-location-rejected", ["System folders cannot be indexed"])
    assert not index.exists()
    session.click(*field)
    type_text(session, str(session.output / "extra-search"))
    click_word(session, "protected-location-rejected", "Save")
    session.observe("saved", ["Search preferences saved", "Excluded search paths"])
    config = session.output / "scratch/config.toml"
    def preferences():
        return tomllib.loads(config.read_text())["file_manager"]["search"]
    wait_for(lambda: preferences()["paused"] and preferences()["keep_model_loaded"], "persist search preferences")
    assert preferences()["always_search"] == [str(session.output / "extra-search")]
    assert not preferences().get("roots")
    assert not index.exists(), "Saving preferences must not create an embedding index"
    click_word(session, "saved", "Close")
    session.click(151, 70)
    session.observe("reopen-edit", ["Search settings"])
    click_word(session, "reopen-edit", "Search")
    session.observe("reopened", ["Pause background indexing", "Excluded search paths"])
    # Reopening uses acknowledged preferences rather than stale initial defaults.
    click_word(session, "reopened", "Pause")
    click_word(session, "reopened", "Save")
    wait_for(lambda: not preferences()["paused"] and preferences()["keep_model_loaded"], "persist resumed indexing")
    session.observe("resumed", ["Search preferences saved", "Excluded search paths"])
    assert not index.exists()
    click_word(session, "resumed", "Close")
    session.observe("browser", ["sample.txt"])
    click_word(session, "browser", "sample.txt")
    private.run_checked(["wtype", "remotebeacon"], env=session.env)
    session.observe("automatic-extra-location", ["automatic scope", "remotebeacon.txt"])
    assert not index.exists(), "Automatic filename fallback must not enroll its extra location"
    submit(session, ':emb:"a document"')
    session.observe("semantic-without-enrollment", ["Filename results only", "no folders enrolled"])
    assert not index.exists(), "An empty semantic scope must not create an index or load a model"


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (output / "extra-search").mkdir()
    (output / "extra-search/remotebeacon.txt").write_text("An ordinary extra search location.")
    (root / "sample.txt").write_text("No indexing without explicit enrollment.")
    return run_application_acceptance(args, output, root, exercise,
        "Edit search preferences, persistent pause/residency, acknowledged reopen and no implicit database creation")


if __name__ == "__main__":
    raise SystemExit(main())
