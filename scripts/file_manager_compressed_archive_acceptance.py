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
    session.click(600, 160)
    session.observe("compressed-inventories", ["Archive contents", "hello.txt"])
    for name in ["bzip.tbz2", "gzip.tgz", "xzdata.txz", "zstd.tzst"]:
        session.address(root)
        session.key(name[0])
        session.key("Return")
        session.observe(f"{name}-root", ["Browsing compressed archive", "docs", name])
        session.key("Down")
        session.key("Return")
        session.observe(f"{name}-members", ["Browsing compressed archive", "hello.txt"])
        if name == "gzip.tgz":
            session.key("Down")
            session.key("Return")
            session.observe("compressed-text", ["Back to folder", "hello.txt", "hello"])
            session.key("Escape")
            session.observe("compressed-text-return", ["Browsing compressed archive", "hello.txt"])
    session.address(root)
    session.key("l")
    session.key("Return")
    text = session.observe("bounded-browse-rejected", ["limited.tgz"], expected_any=["budget", "limit"])
    assert "Browsing compressed archive" not in text, "A partial large archive must not publish a browseable snapshot"
    assert not (root / "docs").exists()
    assert not (root / "payload.bin").exists()
    assert before == {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in root.iterdir()}


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Gzip/bzip2/xz/zstd inventories and snapshot navigation, member text, bounded large-archive rejection and unchanged sources"))
