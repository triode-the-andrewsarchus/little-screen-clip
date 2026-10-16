#!/usr/bin/env python3
"""Exercise PDF sibling collisions, rotation, OCR and reuse on private Weston/Niri."""
import hashlib
import json
import os
import shlex
import shutil
from pathlib import Path
import ui_acceptance_fixtures as fixtures
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for, private
from file_manager_phase4_acceptance import open_file, click_word, back


def properties(session, path):
    python = Path.home() / ".local/share/little-screen-clip/nobg-venv/bin/python"
    script = ("import pymupdf as pdf,json,sys; d=pdf.open(sys.argv[1]); "
              "print(json.dumps(dict(pages=len(d),rotations=[p.rotation for p in d],"
              "text=[p.get_text() for p in d],source_hash=d.xref_get_key(-1,'Info/LittleClipOCRSourceSHA256')[1])))")
    return json.loads(private.run_checked([str(python), "-c", script, str(path)], env=session.env))


def exercise(session, root):
    source = root / "document.pdf"
    rotated = root / "document.rotate.edited.pdf"
    searchable = root / "document.rotate.edited.ocr.edited.pdf"
    original_bytes = source.read_bytes()
    previous_output = rotated.read_bytes()
    session.observe("01-folder", ["document.pdf", "Folders"])
    session.click(848, 201)
    open_file(session, root, source.name, "02-source-row")
    session.observe("03-source", ["3 pages", "Page 1 of 3"])
    session.click(600, 400)  # Focus the PDF canvas before its keyboard shortcut.
    session.key("r")
    status = Path(session.env["HOME"]) / "qpdf-status"
    wait_for(status.exists, "rotation tool completion")
    assert status.read_text().strip() == "0", "rotation producer failed before publication"
    wait_for(lambda: not any(path.is_dir() for path in root.iterdir()), "collision staging cleanup")
    # Verify persistent state rather than depending on a four-second toast.
    session.observe("04-collision", ["document.pdf", "3 pages", "Page 1 of 3"])
    assert rotated.read_bytes() == previous_output
    assert source.read_bytes() == original_bytes
    assert not any(path.is_dir() for path in root.iterdir()), "failed publication leaked staging"
    rotated.unlink()  # Remove only the owned collision fixture, then repeat the normal action.
    status.unlink()
    session.click(600, 400)
    session.key("r")
    wait_for(rotated.exists, "rotated PDF sibling")
    session.observe("05-rotated", [rotated.name, "3 pages", "Page 1 of 3"])
    rotation = properties(session, rotated)
    assert rotation["pages"] == 3 and rotation["rotations"] == [90, 0, 0], rotation
    rotated_bytes = rotated.read_bytes()
    click_word(session, "05-rotated", "OCR", minimum_y=150, maximum_y=270)
    session.observe("06-ocr-controls", ["Tesseract", "Add text layer to PDF"])
    click_word(session, "06-ocr-controls", "Add")
    wait_for(searchable.exists, "searchable PDF sibling")
    session.observe("07-searchable", ["Searchable PDF saved", "3 pages"])
    ocr = properties(session, searchable)
    assert ocr["pages"] == 3 and ocr["rotations"] == [90, 0, 0], ocr
    assert ocr["source_hash"] == hashlib.sha256(rotated_bytes).hexdigest(), ocr
    assert "Acceptance page 01" in ocr["text"][0], ocr
    assert rotated.read_bytes() == rotated_bytes and source.read_bytes() == original_bytes
    back(session, "07-searchable")
    session.observe("08-return", ["Folders", "3 items"])
    open_file(session, root, rotated.name, "09-reopen-row")
    session.observe("10-verified-reuse", [searchable.name, "3 pages"])
    back(session, "10-verified-reuse")
    session.observe("11-final-folder", ["Folders", "3 items"])
    assert sorted(path.name for path in root.iterdir()) == sorted([source.name, rotated.name, searchable.name])
    (session.output / "pdf-properties.json").write_text(json.dumps(dict(rotation=rotation, ocr=ocr), indent=2))


def main():
    args = application_arguments(__doc__, Path("target/release/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures.pdf_fixture(root, 3, "document.pdf")
    fixtures.pdf_fixture(root, 1, "document.rotate.edited.pdf")
    def prepare(env, _):
        installed = Path.home() / ".local/share/little-screen-clip/nobg-venv"
        assert (installed / "bin/python").is_file(), "PDF acceptance requires the installed Python environment"
        private_venv = Path(env["HOME"]) / ".local/share/little-screen-clip/nobg-venv"
        private_venv.parent.mkdir(parents=True, exist_ok=True)
        private_venv.symlink_to(installed, target_is_directory=True)
        qpdf = shutil.which("qpdf")
        assert qpdf, "PDF acceptance requires qpdf"
        tools = Path(env["HOME"]) / "acceptance-bin"
        tools.mkdir()
        wrapper = tools / "qpdf"
        wrapper.write_text("#!/bin/sh\n" + shlex.quote(qpdf) + " \"$@\"\nresult=$?\nprintf '%s\\n' \"$result\" > \"$HOME/qpdf-status\"\nexit \"$result\"\n")
        wrapper.chmod(0o700)
        env["PATH"] = str(tools) + os.pathsep + env["PATH"]
    return run_application_acceptance(args, output, root, exercise, "PDF staged output and OCR lifecycle", prepare)


if __name__ == "__main__":
    raise SystemExit(main())
