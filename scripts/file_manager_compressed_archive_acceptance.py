#!/usr/bin/env python3
"""Review bounded compressed archive cards on the private native compositor."""
from pathlib import Path
import bz2
import gzip
import hashlib
import io
import lzma
import subprocess
import tarfile
import re
from PIL import Image
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w") as archive:
        header = tarfile.TarInfo("docs/hello.txt")
        header.size = 5
        archive.addfile(header, io.BytesIO(b"hello"))
    payload = raw.getvalue()
    (root / "gzip.tgz").write_bytes(gzip.compress(payload))
    (root / "bzip.tbz2").write_bytes(bz2.compress(payload))
    (root / "xzdata.txz").write_bytes(lzma.compress(payload))
    encoded = subprocess.run(["zstd", "-q", "-c"], input=payload, capture_output=True, check=True, timeout=10)
    (root / "zstd.tzst").write_bytes(encoded.stdout)
    class Zeros:
        def read(self, size):
            return bytes(size)
    # A real large member, streamed directly into a small compressed fixture.
    with tarfile.open(root / "limited.tgz", mode="w:gz", compresslevel=1) as archive:
        header = tarfile.TarInfo("payload.bin")
        header.size = 128 * 1024 * 1024
        archive.addfile(header, Zeros())


def exercise(session, root):
    before = {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}
    session.observe("initial", ["List", "Name"])
    session.click(730, 160)  # Collapse Shelf on the fixed private output.
    session.observe("compressed-inventories", ["Archive contents", "hello.txt"])
    normal = lambda text: re.sub(r"[^a-z0-9]", "", text.lower())
    with Image.open(session.output / "compressed-inventories.png") as screen:
        for name, top in [("bzip.tbz2", 250), ("gzip.tgz", 358), ("limited.tgz", 466), ("xzdata.txz", 494), ("zstd.tzst", 602)]:
            crop = session.output / f"title-{name}.png"
            region = screen.crop((398, top, 750, top + 28))
            region.resize((region.width * 4, region.height * 4)).save(crop)
            text = private.run_checked(["tesseract", str(crop), "stdout", "--psm", "7"], env=session.env)
            assert normal(name.split(".")[0]) in normal(text), (name, text)
            if not name.startswith("limited"):
                crop = session.output / f"members-{name}.png"
                region = screen.crop((365, top + 28, 900, top + 82))
                region.resize((region.width * 4, region.height * 4)).save(crop)
                text = private.run_checked(["tesseract", str(crop), "stdout", "--psm", "6"], env=session.env)
                assert "archivecontents" in normal(text) and "hellotxt" in normal(text), (name, text)
    session.click(355, 480)  # Separate disclosure on the observed limited archive row.
    session.observe("limited-preview", ["Preview limit", "Archive contents"])
    assert not (root / "docs").exists()
    assert not (root / "payload.bin").exists()
    assert before == {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Gzip/bzip2/xz/zstd inline archive inventories, explicit bounded partial previews and source preservation"))
