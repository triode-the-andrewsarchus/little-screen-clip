#!/usr/bin/env python3
"""Qualify the fixed companion against the canonical desktop SVG API.

Usage: python3 vendor/umlet/probe.py /absolute/output/clip-umlet.jar
Reference inputs are only known upstream/local fixtures, never untrusted code.
"""
import os
from pathlib import Path
import resource
import subprocess
import sys
import tempfile
import time
import zipfile
import xml.etree.ElementTree as ET

JVM = ["java", "-Xms16m", "-Xmx128m", "-Xss512k", "-XX:+UseSerialGC",
       "-XX:ReservedCodeCacheSize=32m", "-XX:MaxMetaspaceSize=96m",
       "-XX:CompressedClassSpaceSize=32m", "-XX:ActiveProcessorCount=1",
       "-XX:-UsePerfData", "-Djava.awt.headless=true"]
REFERENCE = """
import com.baselet.control.config.Config;
import com.baselet.control.enums.Program;
import com.baselet.control.enums.RuntimeType;
import com.baselet.diagram.DiagramHandler;
import com.baselet.diagram.io.InputHandler;
import com.baselet.diagram.io.OutputHandler;
import javax.xml.parsers.SAXParserFactory;
public class UmletReference {
 public static void main(String[] args) throws Exception {
  java.io.PrintStream output = System.out;
  System.setOut(System.err);
  java.util.Locale.setDefault(java.util.Locale.ROOT);
  Program.init("15.1", RuntimeType.BATCH);
  Config.getInstance().setEnable_custom_elements(false);
  DiagramHandler handler = DiagramHandler.forExport(null);
  SAXParserFactory.newInstance().newSAXParser().parse(System.in, new InputHandler(handler));
  java.io.ByteArrayOutputStream result = new java.io.ByteArrayOutputStream();
  OutputHandler.createToStream("svg", result, handler);
  output.write(result.toByteArray()); output.flush(); System.exit(0);
 }
}
"""


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (2 * 1024**3, 2 * 1024**3))
    resource.setrlimit(resource.RLIMIT_CPU, (4, 4))


def run(command, source):
    result = subprocess.run(command, input=source, capture_output=True, timeout=6,
                            preexec_fn=limits)
    assert len(result.stdout) <= 2 * 1024**2, "unbounded output"
    return result


def geometry(svg):
    root = ET.fromstring(svg)
    tags = {"path", "rect", "ellipse", "circle", "polygon", "polyline", "line"}
    return (root.attrib.get("width"), root.attrib.get("height"), root.attrib.get("viewBox"),
            [(node.tag, sorted(node.attrib.items())) for node in root.iter()
             if node.tag.rsplit("}", 1)[-1] in tags])


def element(panel="Example", kind="UMLClass", extra=""):
    return (f"<element><id>{kind}</id><coordinates><x>10</x><y>20</y><w>160</w><h>80</h>"
            f"</coordinates><panel_attributes>{panel}</panel_attributes>"
            f"<additional_attributes/>{extra}</element>").encode()


def main(companion):
    companion = Path(companion).resolve()
    root = Path(__file__).resolve().parent
    repository = root.parent.parent
    command = JVM + ["-jar", str(companion)]
    with zipfile.ZipFile(companion) as jar:
        names = jar.namelist()
        assert not any(n.startswith(("org/mozilla/", "org/eclipse/jdt/")) for n in names)
        assert not any(n.startswith("com/baselet/element/old/custom/CustomElementCompiler") for n in names)
    with tempfile.TemporaryDirectory(prefix="umlet-reference-") as scratch:
        scratch = Path(scratch)
        libraries = []
        with zipfile.ZipFile(root / "umlet-standalone-15.1.zip") as archive:
            for name in archive.namelist():
                if name.endswith(".jar"):
                    path = scratch / Path(name).name
                    path.write_bytes(archive.read(name))
                    libraries.append(str(path))
        (scratch / "UmletReference.java").write_text(REFERENCE)
        classes = scratch / "classes"
        classes.mkdir()
        # Both sides use identical portable font metrics; geometry/parser/export
        # algorithms on the reference side are the pristine canonical classes.
        subprocess.run([os.environ.get("LITTLE_CLIP_JAVAC", "javac"), "--release", "11",
                        "-cp", os.pathsep.join(libraries), "-d", str(classes),
                        str(scratch / "UmletReference.java"),
                        str(root / "bridge/com/baselet/diagram/FontHandler.java")], check=True)
        reference = JVM + ["-cp", os.pathsep.join([str(classes), *libraries, str(companion)]), "UmletReference"]
        fixtures = sorted((root / "upstream/umlet-res/src/main/resources/palettes").glob("*.uxf"))
        fixtures += sorted((repository / "docs/examples/diagrams/umlet").glob("*.uxf"))
        checked = 0
        for path in fixtures:
            data = path.read_bytes()
            actual = run(command, data)
            if b"_umletcode=" in data:
                assert actual.returncode != 0 and not actual.stdout
                assert "JavaScript" in actual.stderr.decode()
                continue
            expected = run(reference, data)
            assert actual.returncode == expected.returncode == 0, (
                path.name, actual.stderr.decode(), expected.stderr.decode())
            assert geometry(actual.stdout) == geometry(expected.stdout), path.name
            checked += 1
        invalid = [
            (b"", "empty"), (b"<diagram>", "XML"), (b"<svg/>", "root"),
            (b"<!DOCTYPE diagram [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><diagram>&x;</diagram>", "DOCTYPE"),
            (b"<diagram>" + element(extra="<custom_code>System.exit(0);</custom_code>") + b"</diagram>", "Custom Java"),
            (b"<diagram>" + element("_umletcode=\njava.lang.System.exit(0);") + b"</diagram>", "JavaScript"),
            (b"<diagram>" + element(kind="CustomElement") + b"</diagram>", "Unsupported"),
            (b"<diagram>" + element(kind="DoesNotExist") + b"</diagram>", "Unsupported"),
            (b"<diagram>" + element().replace(b"<w>160</w>", b"<w>999999</w>") + b"</diagram>", "limits"),
            (b"<diagram>" + element("bg=invalid-color") + b"</diagram>", "color"),
            (b"<diagram>" + element() * 513 + b"</diagram>", "512"),
            (b"x" * (256 * 1024 + 1), "256 KiB"),
            (b"<diagram>" + b"<group>" * 40 + element() + b"</group>" * 40 + b"</diagram>", "depth"),
        ]
        for data, message in invalid:
            result = run(command, data)
            assert result.returncode != 0 and not result.stdout, (data[:50], result.stdout[:50])
            assert message.lower() in result.stderr.decode().lower(), result.stderr.decode()
        dense = b"<diagram>" + element("Bounded\n--\nA\nB\nC") * 512 + b"</diagram>"
        started = time.monotonic()
        result = run(command, dense)
        elapsed = time.monotonic() - started
        if result.returncode == 0:
            ET.fromstring(result.stdout)
        else:
            assert not result.stdout
        print(f"UMLet: {checked} canonical geometry matches; forbidden advanced palette rejected; {len(invalid)} errors passed")
        print(f"512-element plane retired in {elapsed:.2f}s (status {result.returncode})")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: probe.py COMPANION_JAR")
    main(sys.argv[1])
