#!/usr/bin/env python3
"""Phase 4 file studios, guarded saves and previews on private Weston/Niri."""
import csv
import io
import json
import subprocess
import struct
import time
from pathlib import Path
from PIL import Image
import ui_acceptance_capture as private
import ui_acceptance_fixtures as fixture
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for


def click_word(session, shot, word, last=False, minimum_y=85, maximum_y=None):
    data = private.run_checked(["tesseract", str(session.output / f"{shot}-ocr.png"), "stdout", "--psm", "11", "-c", "tessedit_create_tsv=1", "-c", "tessedit_create_txt=0"], env=session.env)
    words = [item for item in csv.DictReader(io.StringIO(data), delimiter="\t", quoting=csv.QUOTE_NONE) if item["text"].lower().strip("[]()|:.,\"'“”‘’") == word.lower() and minimum_y < int(item["top"]) // 3 < (maximum_y if maximum_y is not None else float("inf"))]
    if not words:
        raise RuntimeError(f"{shot}: no {word}")
    item = words[-1] if last else words[0]
    session.click((int(item["left"]) + int(item["width"]) // 2) // 3, (int(item["top"]) + int(item["height"]) // 2) // 3)


def open_file(session, root, filename, shot):
    if filename == "external.unknown":
        session.click(500, 310)
        for _ in range(12):
            session.key("Up")
        session.key("Down")
        session.observe(shot, ["external"])
        session.key("Return")
        return
    private.run_checked([str(session.pointer), "scroll", "1280", "800", "800", "400", "-600"], env=session.env)
    time.sleep(0.3)
    for _ in range(4):
        session.observe(shot, [])
        if session.word(shot, filename, required=False):
            break
        private.run_checked([str(session.pointer), "scroll", "1280", "800", "800", "400", "200"], env=session.env)
        time.sleep(0.3)
    session.observe(shot, [filename])
    x, y = session.word(shot, filename)
    session.click(x + 15, y)
    session.key("Return")


def type_text(session, text):
    private.run_checked(["wtype", text], env=session.env)


def back(session, shot):
    # The fixed private 1280x800 fixture places the Files back button here.
    session.click(150, 118)


def exercise(session, root):
    session.observe("01-browser", ["Files", "Folders", "notes.txt", "picture.png"])
    open_file(session, root, "notes.txt", "02-text-row")
    session.observe("03-text-preview", ["Back to folder", "ORIGINALNOTE", "Edit text"])
    click_word(session, "03-text-preview", "Edit")
    session.observe("04-text-editor", ["Save", "ORIGINALNOTE"])
    # The code editor follows the two toolbars.
    x, y = session.word("04-text-editor", "ORIGINALNOTE", tree=True, required=False) or session.word("04-text-editor", "ORIGINALNOTE")
    session.click(x + 10, y)
    session.key("a", "ctrl")
    type_text(session, "EDITEDNOTE\nsecond line\n")
    session.observe("05-text-dirty", ["Unsaved changes", "EDITEDNOTE"])
    back(session, "05-text-dirty")
    session.observe("06-text-close-prompt", ["Unsaved edits", "Keep editing", "Discard edits"])
    click_word(session, "06-text-close-prompt", "Keep")
    session.key("s", "ctrl")
    wait_for(lambda: (root / "notes.txt").read_text() == "EDITEDNOTE\nsecond line\n", "exact text save")
    session.observe("07-text-saved", ["EDITEDNOTE"])
    click_word(session, "07-text-saved", "Preview")
    session.observe("08-refreshed-preview", ["EDITEDNOTE"])
    back(session, "08-refreshed-preview")
    session.observe("09-return", ["notes.txt", "Folders"])

    open_file(session, root, "notes.txt", "10-reopen")
    session.observe("11-reopened", ["EDITEDNOTE", "Edit text"])
    click_word(session, "11-reopened", "Edit")
    session.observe("12-editor-reopen", ["EDITEDNOTE", "Save"])
    (root / "notes.txt").write_text("EXTERNALNOTE")
    x, y = session.word("12-editor-reopen", "EDITEDNOTE", tree=True, required=False) or session.word("12-editor-reopen", "EDITEDNOTE")
    session.click(x + 10, y)
    session.key("a", "ctrl")
    type_text(session, "REFUSEDNOTE")
    session.key("s", "ctrl")
    session.observe("13-text-conflict", ["Source changed outside Files", "REFUSEDNOTE"])
    assert (root / "notes.txt").read_text() == "EXTERNALNOTE"
    back(session, "13-text-conflict")
    session.observe("14-discard", ["Discard edits"])
    click_word(session, "14-discard", "Discard")
    session.observe("15-browser", ["Folders", "picture.png"])

    open_file(session, root, "picture.png", "16-image-row")
    session.observe("17-image-studio", ["picture.png", "PNG", "Nomacs"])
    session.key("f")
    session.observe("18-transform", ["Mirror"])
    # Shared Image Studio's horizontal flip button in the fixed fixture.
    session.click(160, 538)
    session.key("s", "ctrl")
    session.observe("19-image-save", ["Save", "picture_edit.png"])
    session.click(510, 498)
    wait_for(lambda: (root / "picture_edit.png").exists(), "image sibling save")
    with Image.open(root / "picture_edit.png") as image:
        assert image.size == (120, 80)
        assert image.getpixel((0, 0))[:3] == (0, 0, 255)
    session.observe("20-image-saved", ["picture_edit.png"])
    back(session, "20-image-saved")
    session.observe("21-image-return", ["picture.png", "Folders"])

    open_file(session, root, "document.pdf", "22-pdf-row")
    session.observe("23-pdf-studio", ["Pages", "Find", "OCR"])
    back(session, "23-pdf-studio")
    session.observe("24-pdf-return", ["Folders"])
    open_file(session, root, "music.wav", "25-audio-row")
    session.observe("26-audio-studio", ["AUDIO STUDIO", "Selection info"])
    back(session, "26-audio-studio")
    session.observe("27-audio-return", ["Folders"])
    open_file(session, root, "table.csv", "28-sheet-row")
    session.observe("29-sheet-studio", ["table.csv", "Copy Table", "alpha", "beta"])
    back(session, "29-sheet-studio")
    session.observe("30-sheet-return", ["Folders"])

    open_file(session, root, "music-three-track.mid", "31-midi-row")
    session.observe("32-midi-studio", ["music-three-track.mid", "BPM", "tracks"])
    back(session, "32-midi-studio")
    session.observe("33-midi-return", ["Folders"])
    open_file(session, root, "model-simple.stl", "34-model-row")
    session.observe("35-model-studio", ["Back to folder", "STL", "triangles"])
    back(session, "35-model-studio")
    session.observe("36-model-return", ["Folders"])

    open_file(session, root, "scene.splat", "37-scene-row")
    session.observe("38-scene-studio", ["Gaussians", "Reset view"])
    back(session, "38-scene-studio")
    session.observe("39-scene-return", ["Folders"])
    open_file(session, root, "external.unknown", "40-external-row")
    wait_for(lambda: (session.output / "external-open.json").exists(), "external application handoff")
    assert json.loads((session.output / "external-open.json").read_text()) == [str(root / "external.unknown")]
    session.observe("41-external-return", ["Folders"])
    for dimension, value in [("width", "640"), ("height", "560")]:
        private.run_checked([session.niri, "msg", "action", f"set-window-{dimension}", value], env=session.env)
    time.sleep(0.5)
    session.observe("42-narrow-browser", ["Files", "Folders"])


def main():
    args = application_arguments(__doc__, Path(__file__).resolve().parent.parent / "target/release/clip-file-manager")
    output, root = media_fixture_directory(args)
    (root / "notes.txt").write_text("ORIGINALNOTE\n")
    image = Image.new("RGB", (120, 80), "red")
    for x in range(60, 120):
        for y in range(80): image.putpixel((x, y), (0, 0, 255))
    image.save(root / "picture.png")
    fixture.pdf_fixture(root, 3, "document.pdf")
    fixture.wav_fixture(root / "music.wav", 4, "tone")
    (root / "table.csv").write_text("name,value\nalpha,1\nbeta,2\n")
    fixture.midi_fixture(root)
    fixture.stl_fixture(root)
    (root / "scene.splat").write_bytes(struct.pack("<6f8B", 0, 0, 0, .1, .1, .1, 220, 80, 40, 255, 255, 128, 128, 128))
    (root / "external.unknown").write_bytes(bytes([0, 255, 0]))
    def prepare_environment(env, children):
        # A synthetic desktop opener keeps this check inside the private run.
        bin_dir = output / "bin"
        bin_dir.mkdir()
        opener = bin_dir / "xdg-open"
        opener.write_text("#!/usr/bin/env python3\nimport json, sys\nfrom pathlib import Path\nPath(" + repr(str(output / "external-open.json")) + ").write_text(json.dumps(sys.argv[1:]))\n")
        opener.chmod(0o700)
        env["PATH"] = str(bin_dir) + ":" + env["PATH"]
    return run_application_acceptance(args, output, root, exercise, "studios", prepare_environment)


if __name__ == "__main__":
    raise SystemExit(main())
