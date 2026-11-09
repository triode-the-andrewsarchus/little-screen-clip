#!/usr/bin/env python3
"""Guarded external launches and saved revisions in the private compositor."""
import json
import os
from pathlib import Path

import ui_acceptance_capture as private
from file_manager_acceptance import (
    application_arguments, media_fixture_directory, run_application_acceptance, wait_for,
)
from file_manager_search_acceptance import submit
from file_manager_search_actions_acceptance import open_result, back


def external_menu(session, shot, label):
    x, y = session.word(shot, "report.txt", minimum_y=190)
    private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x + 10), str(y)], env=session.env)
    session.observe(label, ["Open externally"])
    session.click(*session.word(label, "externally"))


def launches(session):
    path = session.output / "external.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def exercise(session, root):
    session.observe("initial", ["Nested"])
    submit(session, ":*.txt report;scope:recursive")
    session.observe("results", ["report.txt", "1 matching entries"])
    external_menu(session, "results", "valid-context-menu")
    wait_for(lambda: len(launches(session)) == 1, "valid external launch")
    target = root / "Nested/report.txt"
    assert launches(session) == [[str(target)]]
    target.write_text("SECONDNOTE")
    session.observe("before-stale-launch", ["report.txt"])
    external_menu(session, "before-stale-launch", "stale-context-menu")
    session.observe("stale-context-rejected", ["Search result changed"])
    assert len(launches(session)) == 1
    session.key("F5")
    text = session.observe("refreshed", ["report.txt", "10 B"])
    assert "search result changed" not in text.lower()
    open_result(session, "refreshed")
    session.observe("studio", ["SECONDNOTE", "Edit text"])
    session.click(212, 170)
    session.observe("editor", ["SECONDNOTE", "Save"])
    session.click(*session.word("editor", "SECONDNOTE", minimum_y=190, tree=True))
    session.key("a", "ctrl")
    private.run_checked(["wtype", "SAVEDNOTE"], env=session.env)
    session.key("s", "ctrl")
    wait_for(lambda: target.read_text() == "SAVEDNOTE", "owned atomic save")
    session.observe("saved", ["SAVEDNOTE", "Save"])
    session.click(285, 118)
    wait_for(lambda: len(launches(session)) == 2, "external launch of saved revision")
    target.write_text("THIRDNOTE")
    session.click(285, 118)
    session.observe("studio-replacement-rejected", ["Source changed outside Files"])
    assert len(launches(session)) == 2
    back(session, "studio-replacement-rejected")
    session.key("F5")
    session.observe("before-launch-failure", ["report.txt", "9 B"])
    (session.output / "external-fail").touch()
    external_menu(session, "before-launch-failure", "failing-context-menu")
    session.observe("launch-failure-visible", ["External application launcher failed"])
    assert len(launches(session)) == 3
    (session.output / "external-fail").unlink()
    session.key("Escape")
    session.address(root / "Nested")
    session.observe("ordinary-folder", ["report.txt", "mystery.zzz"])
    session.click(*session.word("ordinary-folder", "mystery.zzz", minimum_y=190))
    session.key("Return")
    wait_for(lambda: len(launches(session)) == 4, "unsupported-format external fallback")
    assert launches(session)[-1] == [str(root / "Nested/mystery.zzz")]
    session.observe("returned-after-fallback", ["Folders", "mystery.zzz", "report.txt"])
    assert not (Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search/index.sqlite").exists()


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "Nested").mkdir()
    (root / "Nested/report.txt").write_text("ORIGINALNOTE")
    (root / "Nested/mystery.zzz").write_bytes(bytes([0, 255, 0, 254]) * 32)

    def environment(env, children):
        directory = output / "mock-bin"
        directory.mkdir()
        opener = directory / "xdg-open"
        opener.write_text("#!/usr/bin/python3\nimport json, os, sys\nfrom pathlib import Path\np = Path(os.environ['CLIP_EXTERNAL_TEST_OUTPUT'])\nwith (p / 'external.jsonl').open('a') as log:\n    log.write(json.dumps(sys.argv[1:]) + '\\n')\nsys.exit(7 if (p / 'external-fail').exists() else 0)\n")
        opener.chmod(0o700)
        env["PATH"] = str(directory) + os.pathsep + env["PATH"]
        env["CLIP_EXTERNAL_TEST_OUTPUT"] = str(output)

    return run_application_acceptance(args, output, root, exercise,
        "External context/studio validation, owned-save revision handoff and surfaced launcher failure without implicit indexing", environment)


if __name__ == "__main__":
    raise SystemExit(main())
