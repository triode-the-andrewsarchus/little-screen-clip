#!/usr/bin/env python3
"""Review archive cards, member browsing and source preservation privately."""
from pathlib import Path
import hashlib
import io
import tarfile
import zipfile
from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for


def word(session, shot, text, minimum_y=0, last=False):
    return (session.word(shot, text, minimum_y=minimum_y, last=last, required=False)
            or session.word(shot, text, tree=True, minimum_y=minimum_y, last=last))


def click_word(session, shot, text, minimum_y=0, last=False):
    x, y = word(session, shot, text, minimum_y, last)
    session.click(x + 5, y)


def blue(session, shot, minimum, bounds):
    def rendered():
        session.capture_text(shot, session.output / f"{shot}.png", [])
        with Image.open(session.output / f"{shot}.png") as image:
            return sum(b > 150 and r < 80 and g < 130 for r, g, b in image.convert("RGB").crop(bounds).get_flattened_data()) >= minimum
    wait_for(rendered, f"{shot}: decoded blue member pixels", 20)


def exercise(session, root):
    before = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    session.observe("initial", ["List", "Name"])
    session.click(730, 160)  # Shelf on the observed Columns-enabled toolbar.
    session.observe("inline", ["Archive contents", "blue.png", "hello.txt"])
    blue(session, "inline-artwork", 20, (345, 290, 1170, 350))
    click_word(session, "inline", "blue.png", minimum_y=290)
    session.observe("image-inspector", ["Archive root", "read-only", "art/blue.png"])
    blue(session, "inspector-artwork", 5000, (350, 190, 1170, 700))
    click_word(session, "image-inspector", "Archive", minimum_y=140)
    session.observe("virtual-root", ["Select a member", "notes", "art"])
    click_word(session, "virtual-root", "notes", minimum_y=200)
    session.observe("notes-folder", ["hello.txt", "notes"])
    click_word(session, "notes-folder", "hello.txt", minimum_y=200)
    session.observe("text-inspector", ["Read-only text", "Archive hello", "notes/hello.txt"])
    click_word(session, "text-inspector", "Back")
    session.observe("restored-list", ["Archive contents", "hello.txt", "samples"])
    click_word(session, "restored-list", "hello.txt", minimum_y=300, last=True)
    session.observe("compressed-inspector", ["Read-only text", "Archive hello", "samples"])
    click_word(session, "compressed-inspector", "Back")
    session.observe("final-list", ["Archive contents", "blue.png"])
    assert before == {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    assert not (root / "notes").exists() and not (root / "art").exists()


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    png = io.BytesIO()
    Image.new("RGB", (160, 100), (30, 70, 220)).save(png, "PNG")
    records = [("notes/hello.txt", b"Archive hello from a captured member.\n"), ("art/blue.png", png.getvalue())]
    with zipfile.ZipFile(root / "mixed.zip", "w") as archive:
        for name, payload in records:
            archive.writestr(name, payload)
    with tarfile.open(root / "samples.tgz", "w:gz") as archive:
        for name, payload in records:
            member = tarfile.TarInfo(name)
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Inline archive artwork, member activation, virtual directories, captured text/image previews and unchanged archive sources"))
