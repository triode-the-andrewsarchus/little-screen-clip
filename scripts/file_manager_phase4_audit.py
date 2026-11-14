#!/usr/bin/env python3
"""Audit current studio ownership, guarded saves and browser return on a private desktop."""
import json
from pathlib import Path
from PIL import Image
import ui_acceptance_capture as private
import ui_acceptance_fixtures as fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_phase4_acceptance import click_word, open_file, type_text


def back(session, image=False):
    session.click(1010 if image else 1120, 72)


def replace_draft(session, shot, word, text):
    x, y = session.word(shot, word, tree=True, required=False) or session.word(shot, word)
    session.click(x + 5, y)
    session.key("a", "ctrl")
    type_text(session, text)


def exercise(session, root):
    session.observe("00-browser", ["Folders"])
    session.click(645, 160)
    open_file(session, root, "notes.txt", "01-text-row")
    session.observe("02-text", ["ORIGINALNOTE", "Edit text", "Back to folder"])
    click_word(session, "02-text", "Edit")
    session.observe("03-editor", ["ORIGINALNOTE", "Edit text"])
    replace_draft(session, "03-editor", "ORIGINALNOTE", "EDITEDNOTE\nsecond line\n")
    session.observe("04-draft", ["EDITEDNOTE", "Unsaved changes"])
    back(session)
    session.observe("05-close-prompt", ["Unsaved edits", "Save your changes"])
    session.click(540, 478)  # Keep editing in the observed centered modal.
    session.key("s", "ctrl")
    wait_for(lambda: (root / "notes.txt").read_text() == "EDITEDNOTE\nsecond line\n", "atomic saved draft")
    session.observe("06-saved", ["EDITEDNOTE"])
    click_word(session, "06-saved", "Preview")
    session.observe("07-refreshed", ["EDITEDNOTE"])
    back(session)
    session.observe("08-return", ["Folders"])
    open_file(session, root, "notes.txt", "09-reopen-row")
    session.observe("10-reopened", ["EDITEDNOTE", "Edit text"])
    click_word(session, "10-reopened", "Edit")
    session.observe("11-reedit", ["EDITEDNOTE"])
    (root / "notes.txt").write_text("EXTERNALNOTE")
    replace_draft(session, "11-reedit", "EDITEDNOTE", "REFUSEDNOTE")
    session.key("s", "ctrl")
    session.observe("12-conflict", ["Source changed outside Files", "REFUSEDNOTE"])
    assert (root / "notes.txt").read_text() == "EXTERNALNOTE"
    back(session)
    session.observe("13-discard", ["Unsaved edits"])
    session.click(660, 478)  # Discard the refused draft, preserving external bytes.
    session.observe("14-browser", ["Folders"])

    for filename, expected in [
        ("picture.png", ["picture.png"]),
        ("document.pdf", ["Find", "OCR"]),
        ("music.wav", ["music.wav"]),
        ("table.csv", ["alpha", "beta"]),
        ("music-three-track.mid", ["BPM"]),
        ("model-simple.stl", ["triangles"]),
    ]:
        open_file(session, root, filename, filename + "-row")
        session.observe(filename + "-studio", expected)
        if filename == "picture.png":
            with Image.open(session.output / (filename + "-studio.png")).convert("RGB") as screen:
                assert sum(count for count, rgb in screen.getcolors(screen.width * screen.height) if rgb == (57, 124, 148)) > 20000
        back(session, image=filename == "picture.png")
        session.observe(filename + "-return", ["Folders"])
    open_file(session, root, "handoff.odd", "external-row")
    wait_for(lambda: (session.output / "external-open.json").exists(), "inert external handoff")
    assert json.loads((session.output / "external-open.json").read_text()) == [str(root / "handoff.odd")]
    session.observe("external-return", ["Folders"])
    for dimension, value in [("width", "640"), ("height", "560")]:
        private.run_checked([session.niri, "msg", "action", f"set-window-{dimension}", value], env=session.env)
    session.observe("narrow-browser", ["Folders"])
    assert (root / "notes.txt").read_text() == "EXTERNALNOTE"


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    (root / "notes.txt").write_text("ORIGINALNOTE\n")
    Image.new("RGB", (120, 80), "#397c94").save(root / "picture.png")
    fixture.pdf_fixture(root, 3, "document.pdf")
    fixture.wav_fixture(root / "music.wav", 4, "tone")
    (root / "table.csv").write_text("name,value\nalpha,1\nbeta,2\n")
    fixture.midi_fixture(root)
    fixture.stl_fixture(root)
    (root / "handoff.odd").write_bytes(bytes([0, 255, 0]))

    def prepare_environment(env, children):
        bin_dir = output / "bin"
        bin_dir.mkdir()
        opener = bin_dir / "xdg-open"
        opener.write_text("#!/usr/bin/env python3\nimport json,sys\nfrom pathlib import Path\nPath(" + repr(str(output / "external-open.json")) + ").write_text(json.dumps(sys.argv[1:]))\n")
        opener.chmod(0o700)
        env["PATH"] = str(bin_dir) + ":" + env["PATH"]

    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Phase 4 current studios, unsaved close, exact save/refresh, external conflict preservation, browser return and inert external handoff", prepare_environment))
