#!/usr/bin/env python3
"""Inspect and edit subtitle cues on the private Weston/Niri compositor."""
from pathlib import Path
import shutil
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word


def exercise(session, root, extension):
    filename, renamed = f"sample.{extension}", f"renamed.{extension}"
    original = (root / filename).read_text()
    identifier = "7" if extension == "srt" else "overlap"
    source_time = "00:00:03,000" if extension == "srt" else "00:03.000" if extension == "vtt" else "0:00:03.00"
    session.observe("01-timeline-strip", [filename, "4 cues", "peak 2"])
    _, y = session.word("01-timeline-strip", filename, minimum_y=240)
    # The second cue alone covers 4.5 seconds on the ten-second full-file strip.
    session.click(735, y + 45)
    session.observe("01b-caption-hover", ["Two voices overlap"], preserve_pointer=True)
    session.click(735, y + 45, delay=0.05)
    session.click(735, y + 45, delay=0.05)
    session.observe("02-selected-cue", ["Cue 2", "Two voices overlap", f"Identifier: {identifier}"])
    click_word(session, "02-selected-cue", "Copy", minimum_y=280)
    copied = subprocess.run(["wl-paste", "--no-newline"], env=session.env, capture_output=True, check=True).stdout
    assert copied == b"Two voices overlap.", copied
    session.click(600, 180)  # Raw / Code tab in the verified shared header.
    session.observe("03-source", [source_time, "Two voices overlap", "Edit text"])
    click_word(session, "03-source", "Edit", minimum_y=90, maximum_y=140)
    if extension in {"ass", "ssa"}:
        session.observe("04-editor", ["Save", "Script Info"])
        session.click(240,300)  # Verified multiline source field below Save.
    else:
        session.observe("04-editor", ["Save", "Welcome to the garden"])
        x, y = session.word("04-editor", "Welcome", tree=True, minimum_y=200)
        session.click(x + 5, y)
    session.key("a", "ctrl")
    replacement = "1\n00:00:02,000 --> 00:00:06,000\nAn edited subtitle.\n"
    if extension == "vtt":
        # Keep the header, notes, style and region source verbatim while changing cues.
        replacement = original.split("opening\n", 1)[0] + "edited\n00:02.000 --> 00:06.000 align:start\nAn edited subtitle.\n"
    if extension in {"ass", "ssa"}:
        prefix = original.split("Dialogue:", 1)[0]
        comment = next(line for line in original.splitlines() if line.startswith("Comment:"))
        first_field = "Marked=0" if extension == "ssa" else "0"
        replacement = prefix + comment + f"\nDialogue: {first_field},0:00:02.00,0:00:06.00,Default,edited,0,0,0,,An edited subtitle.\n"
    subprocess.run(["wl-copy", "--type", "text/plain;charset=utf-8"], input=replacement.encode(),
        env=session.env, check=True, timeout=5)
    session.key("v", "ctrl")
    session.key("s", "ctrl")
    wait_for(lambda: (root / filename).read_text() == replacement, "exact subtitle save")
    session.observe("05-saved", ["An edited subtitle"])
    session.click(140, 117)  # Preview tab above the verified source editor.
    session.observe("06-refreshed", ["1 cue", "00:00:06.000", "An edited subtitle"])
    x, y = session.word("06-refreshed", filename, tree=True, minimum_y=90)
    session.click(x + 10, y)
    session.observe("07-rename", ["Filename"])
    session.key("a", "ctrl")
    private.run_checked(["wtype", renamed], env=session.env)
    session.key("Return")
    wait_for(lambda: (root / renamed).exists(), "subtitle rename")
    session.observe("08-renamed", [renamed, "An edited subtitle"])
    session.click(1110, 70)
    session.observe("09-folder", [renamed, "1 cue", "notes.txt"])
    open_file(session, root, renamed, "10-reopen")
    session.observe("11-reopened", ["An edited subtitle", "00:00:06.000"])
    assert (root / renamed).read_text() == replacement


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / "target/release/clip-file-manager",
        lambda parser: parser.add_argument("--format", choices=["srt", "vtt", "ass", "ssa"], default="srt"))
    output, root = media_fixture_directory(args)
    filename = f"sample.{args.format}"
    shutil.copyfile(repo / "crates/clip-workbench/tests/fixtures/subtitles" / filename, root / filename)
    (root / "notes.txt").write_text("A neighboring text file.\n")
    return run_application_acceptance(args, output, root, lambda session, root: exercise(session, root, args.format), "subtitle cue strip, clicked cue, copy, raw source, atomic save, refreshed timeline, rename and reopen")


if __name__ == "__main__":
    raise SystemExit(main())
