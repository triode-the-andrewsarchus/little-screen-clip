#!/usr/bin/env python3
"""Build an offline, replaceable Ditaa companion from the pinned release JAR.

Usage: python3 vendor/ditaa/build.py /absolute/output/clip-ditaa.jar
Only a local JDK 11+ is needed. No Maven, Leiningen, network or global installs.
"""
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile


def build(destination):
    root = Path(__file__).resolve().parent
    repository = root.parent.parent
    engine = root / "ditaa-0.11.0-standalone.jar"
    destination = Path(destination).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ditaa-", dir=destination.parent) as scratch:
        classes = Path(scratch) / "classes"
        classes.mkdir()
        sources = sorted((root / "bridge").rglob("*.java"))
        subprocess.run(
            [os.environ.get("LITTLE_CLIP_JAVAC", "javac"), "--release", "11",
             "-encoding", "UTF-8", "-sourcepath", "", "-cp", str(engine),
             "-d", str(classes), *map(str, sources)],
            check=True,
        )
        entries = {}
        with zipfile.ZipFile(engine) as original:
            for item in original.infolist():
                # Keep the canonical engine and all notices. CLI/HTML/config
                # dependency packages are unused by the fixed in-memory API.
                # probe.py compares this selection against the pristine JAR.
                if not item.is_dir() and item.filename.startswith(
                        ("org/stathissideris/", "META-INF/", "license/")):
                    entries[item.filename] = original.read(item)
        entries["META-INF/MANIFEST.MF"] = (
            b"Manifest-Version: 1.0\r\nMain-Class: ClipDitaaPreview\r\n\r\n"
        )
        for path in classes.rglob("*.class"):
            entries[path.relative_to(classes).as_posix()] = path.read_bytes()
        fonts = repository / "crates/little-clip-render/assets/fonts/graphviz"
        entries["Cousine-Bold.ttf"] = (fonts / "Cousine-Bold.ttf").read_bytes()
        entries["clip-ditaa/OFL-Cousine.txt"] = (fonts / "OFL-Cousine.txt").read_bytes()
        entries["clip-ditaa/ADAPTATIONS.md"] = (root / "ADAPTATIONS.md").read_bytes()
        # A copied sidecar retains complete engine/adaptation source and build
        # instructions, independently of the source checkout.
        for folder in ["upstream", "bridge", "licenses"]:
            for path in sorted((root / folder).rglob("*")):
                if path.is_file():
                    entries["clip-ditaa/source/" + path.relative_to(root).as_posix()] = path.read_bytes()
        for name in ["build.py", "probe.py", "UPSTREAM.md", "ADAPTATIONS.md"]:
            entries["clip-ditaa/source/" + name] = (root / name).read_bytes()
        staged = Path(scratch) / "companion.jar"
        with zipfile.ZipFile(staged, "w", compression=zipfile.ZIP_DEFLATED) as output:
            for name, data in sorted(entries.items()):
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.compress_type = zipfile.ZIP_DEFLATED
                info.external_attr = 0o644 << 16
                output.writestr(info, data)
        os.replace(staged, destination)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: build.py OUTPUT_JAR")
    build(sys.argv[1])
