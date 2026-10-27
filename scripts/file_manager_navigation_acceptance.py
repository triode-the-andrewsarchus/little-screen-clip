#!/usr/bin/env python3
"""Progressive recursive filename fallback on the private Weston/Niri runner."""
import json
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["report.txt", "Nested"])
    private.run_checked(["wtype", "landmark"], env=session.env)
    session.observe("automatic-subtree", ["automatic scope", "landmark.txt", "1 matching entries", "Selection: 1 file"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "reprot"], env=session.env)
    session.observe("local-again", ["reprot", "report.txt", "Current folder", "1 matching entries"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "outside"], env=session.env)
    session.observe("configured-location", ["outside.txt", "1 matching entries", "unsupported/incomplete reads"])
    session.key("Escape")
    session.observe("restored", ["report.txt", "Nested"])
    submit(session, ":*.txt landmark")
    session.observe("explicit-stays-here", ["0 matching entries", "Current folder"])
    session.key("Escape")
    submit(session, ":*.txt outside;scope:recursive")
    session.observe("explicit-excludes-extra-roots", ["0 matching entries", "Explicit search scope"])
    assert not (Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite").exists()


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "report.txt").write_text("local")
    (root / "Nested").mkdir()
    (root / "Nested/landmark.txt").write_text("nested")
    (root / ".git").mkdir()
    (root / ".git/landmark-secret.txt").write_text("excluded")
    (root / "loop").symlink_to(root, target_is_directory=True)
    extra = output / "extra"
    extra.mkdir()
    (extra / "outside.txt").write_text("extra root")

    def environment(env, children):
        settings = '\n[file_manager.search]\nalways_search = ' + json.dumps([
            str(extra), str(root / "Nested"), str(output / "missing"), "/proc"
        ]) + '\n'
        for config in [output / "scratch/config.toml", Path(env["XDG_CONFIG_HOME"]) / "little-screen-clip/config.toml"]:
            with config.open("a") as stream:
                stream.write(settings)

    return run_application_acceptance(args, output, root, exercise,
        "Debounced subtree fallback, live query edits, extra roots, protected paths, loop exclusion and explicit scopes without an index", environment)


if __name__ == "__main__":
    raise SystemExit(main())
