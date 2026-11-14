#!/usr/bin/env python3
"""Check empty and deliberately partial archive inventories in the native List."""
from pathlib import Path
import hashlib
import tarfile
import zipfile
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    with zipfile.ZipFile(root / "a-empty.zip", "w"):
        pass
    with tarfile.open(root / "b-links.tar", "w") as archive:
        for index in range(2):
            member = tarfile.TarInfo(f"alias-{index}")
            member.type = tarfile.SYMTYPE
            member.linkname = "../outside"
            archive.addfile(member)
    with tarfile.open(root / "c-duplicate.tar", "w") as archive:
        for _ in range(2):
            archive.addfile(tarfile.TarInfo("same.txt"))


def exercise(session, root):
    original = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}
    session.observe("initial", ["a-empty.zip", "b-links.tar", "c-duplicate.tar"])
    session.click(645, 160)  # Collapse Shelf in the observed List toolbar.
    session.observe("empty-and-duplicate", ["Empty", "Duplicate archive member path"])
    point = session.word("empty-and-duplicate", "b-links.tar", required=False)
    # Dark-theme OCR can drop the archive basename despite observing it on the
    # initial frame. The preceding empty inventory has a fixed 108px row.
    y = point[1] if point else session.word("empty-and-duplicate", "Empty")[1] + 58
    session.click(360, y)  # Incomplete inventories require an explicit expansion.
    session.observe("omitted-links", ["No previewable entries", "2 entries omitted"])
    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()} == original
    assert not (root.parent / "outside").exists()


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Empty ZIP, explicit omitted-link disclosure and duplicate-member refusal without archive extraction"))
