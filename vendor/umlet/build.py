#!/usr/bin/env python3
"""Build a separate offline UMLet SVG companion; requires local JDK 11+.

Usage: python3 vendor/umlet/build.py /absolute/output/clip-umlet.jar
The official distribution/source snapshot remain unchanged.
"""
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import zipfile

# SVG rendering requires neither the desktop launchers/editors nor PDF output,
# Java compilation, mail, bytecode tools, Rhino or a logging implementation.
LIBRARIES = [
    "umlet-swing-15.1.jar", "umlet-elements-15.1.jar", "slf4j-api-2.0.6.jar",
    "batik-awt-util-1.8.jar", "batik-util-1.8.jar", "batik-dom-1.8.jar",
    "batik-ext-1.8.jar", "batik-xml-1.8.jar", "batik-svggen-1.8.jar",
    "commons-io-2.4.jar", "jlibeps-0.1.jar",
]
EXCLUDED_CLASSES = (
    "com/baselet/element/old/custom/CustomElementCompiler",
    "com/baselet/element/old/custom/CustomElementCompileTask",
    "com/baselet/element/old/custom/FileClassLoader",
    "com/baselet/element/old/custom/CustomElementHandler",
    "com/baselet/diagram/draw/swing/javascriptparser/DrawerScriptable",
    "com/baselet/diagram/draw/swing/javascriptparser/CustomFunctionObject",
)


def build(destination):
    root = Path(__file__).resolve().parent
    repository = root.parent.parent
    destination = Path(destination).resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="umlet-", dir=destination.parent) as scratch:
        scratch = Path(scratch)
        libraries = []
        entries = {}
        with zipfile.ZipFile(root / "umlet-standalone-15.1.zip") as distribution:
            for name in LIBRARIES:
                data = distribution.read("Umlet/lib/" + name)
                path = scratch / name
                path.write_bytes(data)
                libraries.append(str(path))
                with zipfile.ZipFile(io.BytesIO(data)) as library:
                    for item in library.infolist():
                        if item.is_dir() or item.filename.startswith(EXCLUDED_CLASSES):
                            continue
                        if item.filename.startswith("META-INF/"):
                            # Artifact-specific paths preserve notices that an
                            # ordinary fat-JAR merge would overwrite.
                            entries["clip-umlet/dependencies/" + name + "/" + item.filename] = library.read(item)
                        else:
                            entries[item.filename] = library.read(item)
        classes = scratch / "classes"
        classes.mkdir()
        subprocess.run(
            [os.environ.get("LITTLE_CLIP_JAVAC", "javac"), "--release", "11",
             "-encoding", "UTF-8", "-sourcepath", "", "-cp", os.pathsep.join(libraries),
             "-d", str(classes), *map(str, sorted((root / "bridge").rglob("*.java")))], check=True)
        for path in classes.rglob("*.class"):
            entries[path.relative_to(classes).as_posix()] = path.read_bytes()
        fonts = repository / "crates/little-clip-render/assets/fonts/graphviz"
        for resource, canonical in [("Arimo.ttf", "Arimo[wght].ttf"),
                                    ("Tinos-Regular.ttf", "Tinos-Regular.ttf"),
                                    ("Cousine-Regular.ttf", "Cousine-Regular.ttf")]:
            entries[resource] = (fonts / canonical).read_bytes()
        for name in ["OFL-Arimo.txt", "OFL-Tinos.txt", "OFL-Cousine.txt"]:
            entries["clip-umlet/" + name] = (fonts / name).read_bytes()
        for folder in ["upstream", "bridge", "licenses", "jlibeps-pristine"]:
            for path in sorted((root / folder).rglob("*")):
                if path.is_file():
                    entries["clip-umlet/source/" + path.relative_to(root).as_posix()] = path.read_bytes()
        for name in ["build.py", "probe.py", "ADAPTATIONS.md", "UPSTREAM.md"]:
            entries["clip-umlet/source/" + name] = (root / name).read_bytes()
        entries["META-INF/MANIFEST.MF"] = b"Manifest-Version: 1.0\r\nMain-Class: ClipUmletPreview\r\n\r\n"
        staged = scratch / "companion.jar"
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
