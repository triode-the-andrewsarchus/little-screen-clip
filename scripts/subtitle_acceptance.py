#!/usr/bin/env python3
"""Inspect and edit subtitle cues on the private Weston/Niri compositor."""
from pathlib import Path
import shutil
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root):
    session.observe("01-timeline-strip", ["sample.srt", "4 cues", "peak 2"])
    _, y = session.word("01-timeline-strip", "sample.srt", minimum_y=240)
    # The second cue alone covers 4.5 seconds on the ten-second full-file strip.
    session.click(735, y + 45)
    session.observe("01b-caption-hover", ["Two voices overlap"], preserve_pointer=True)
    session.click(735, y + 45, delay=0.05)
    session.click(735, y + 45, delay=0.05)
    session.observe("02-selected-cue", ["Cue 2", "Two voices overlap", "Counter: 7"])
    click_word(session, "02-selected-cue", "Copy", minimum_y=280)
    copied = subprocess.run(["wl-paste", "--no-newline"], env=session.env, capture_output=True, check=True).stdout
    assert copied == b"Two voices overlap.", copied
    session.click(600, 180)  # Raw / Code tab in the verified shared header.
    session.observe("03-source", ["00:00:03,000", "Two voices overlap", "Edit text"])
    click_word(session, "03-source", "Edit", minimum_y=90, maximum_y=140)
    session.observe("04-editor", ["Save", "Welcome to the garden"])
    x, y = session.word("04-editor", "Welcome", tree=True, minimum_y=200)
    session.click(x + 5, y)
    session.key("a", "ctrl")
    replacement = "1\n00:00:02,000 --> 00:00:06,000\nAn edited subtitle.\n"
    private.run_checked(["wtype", replacement], env=session.env)
    session.key("s", "ctrl")
    wait_for(lambda: (root / "sample.srt").read_text() == replacement, "exact subtitle save")
    session.observe("05-saved", ["An edited subtitle"])
    session.click(140, 117)  # Preview tab above the verified source editor.
    session.observe("06-refreshed", ["1 cue", "00:00:06.000", "An edited subtitle"])
    x, y = session.word("06-refreshed", "sample.srt", tree=True, minimum_y=90)
    session.click(x + 10, y)
    session.observe("07-rename", ["Filename"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", "renamed.srt"], env=session.env)
    session.key("Return")
    wait_for(lambda: (root / "renamed.srt").exists(), "subtitle rename")
    session.observe("08-renamed", ["renamed.srt", "An edited subtitle"])
    session.click(1110, 70)
    session.observe("09-folder", ["renamed.srt", "1 cue", "notes.txt"])
    open_file(session, root, "renamed.srt", "10-reopen")
    session.observe("11-reopened", ["An edited subtitle", "00:00:06.000"])
    assert (root / "renamed.srt").read_text() == replacement


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    shutil.copyfile(repo / "crates/clip-workbench/tests/fixtures/subtitles/sample.srt", root / "sample.srt")
    (root / "notes.txt").write_text("A neighboring text file.\n")
    return run_application_acceptance(args, output, root, exercise, "subtitle cue strip, clicked cue, copy, raw source, atomic save, refreshed timeline, rename and reopen")


if __name__ == "__main__":
    raise SystemExit(main())
