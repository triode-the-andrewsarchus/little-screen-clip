#!/usr/bin/env python3
"""Differentially check the companion against the pristine canonical API.

Usage: python3 vendor/ditaa/probe.py /absolute/output/clip-ditaa.jar
Runs only isolated Java processes; it does not invoke Cargo or install software.
"""
from pathlib import Path
import os
import resource
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET

JVM = ["java", "-Xms16m", "-Xmx128m", "-Xss512k", "-XX:+UseSerialGC",
       "-XX:ReservedCodeCacheSize=32m", "-XX:MaxMetaspaceSize=96m",
       "-XX:CompressedClassSpaceSize=32m", "-XX:ActiveProcessorCount=1",
       "-XX:-UsePerfData", "-Djava.awt.headless=true"]
REFERENCE = """
import java.nio.charset.StandardCharsets;
import java.util.Locale;
import org.stathissideris.ascii2image.core.ConversionOptions;
import org.stathissideris.ascii2image.text.TextGrid;
import org.stathissideris.ascii2image.graphics.Diagram;
import org.stathissideris.ascii2image.graphics.SVGRenderer;
public class DitaaReference {
    public static void main(String[] args) throws Exception {
        Locale.setDefault(Locale.ROOT);
        String source = new String(System.in.readAllBytes(), StandardCharsets.UTF_8);
        ConversionOptions options = new ConversionOptions();
        TextGrid grid = new TextGrid();
        grid.initialiseWithText(source, options.processingOptions);
        String svg = new SVGRenderer().renderToImage(new Diagram(grid, options), options.renderingOptions);
        System.out.write(svg.getBytes(StandardCharsets.UTF_8));
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
    tree = ET.fromstring(svg)
    return (tree.attrib["width"], tree.attrib["height"],
            [dict(node.attrib) for node in tree.iter("{http://www.w3.org/2000/svg}path")])


def main(companion):
    root = Path(__file__).resolve().parent
    repository = root.parent.parent
    actual_command = JVM + ["-jar", str(Path(companion).resolve())]
    with tempfile.TemporaryDirectory(prefix="ditaa-reference-") as scratch:
        source = Path(scratch) / "DitaaReference.java"
        source.write_text(REFERENCE)
        subprocess.run([os.environ.get("LITTLE_CLIP_JAVAC", "javac"), "--release", "11",
                        "-cp", str(root / "ditaa-0.11.0-standalone.jar"), str(source)], check=True)
        reference_command = JVM + ["-cp", os.pathsep.join(
            [scratch, str(root / "ditaa-0.11.0-standalone.jar")]), "DitaaReference"]
        fixtures = sorted((root / "upstream/test-resources/text").glob("*.txt"))
        fixtures += sorted((repository / "docs/examples/diagrams").glob("*.ditaa"))
        checked = 0
        excluded = []
        for path in fixtures:
            data = path.read_bytes()
            # The application intentionally caps the canonical engine's plane.
            lines = data.decode("utf-8").splitlines()
            if len(lines) > 128 or any(len(line.expandtabs(8).encode("utf-16-le")) // 2 > 256
                                       for line in lines):
                excluded.append(path.name)
                continue
            actual = run(actual_command, data)
            reference = run(reference_command, data)
            assert actual.returncode == reference.returncode == 0, (
                path.name, actual.stderr.decode(), reference.stderr.decode())
            assert geometry(actual.stdout) == geometry(reference.stdout), path.name
            checked += 1
        invalid = [
            (b"", "empty"), (b"x" * 257, "columns"), (b"x\n" * 129, "rows"),
            (b"x" + b"\t" * 33, "columns"), (b"@startditaa\nx", "@endditaa"),
            (b"@endditaa", "@startditaa"),
            (b"@startditaa --html\nx\n@endditaa", "option"),
            (b"@startditaa scale=9\nx\n@endditaa", "scale"),
            (b"@startditaa scale=NaN\nx\n@endditaa", "scale"),
            (b"@startditaa scale=4,scale=4\nx\n@endditaa", "scale"),
            (b"x\x00", "control"), (b"\xff", "Input length"),
            (b"x" * (256 * 1024 + 1), "256 KiB"),
        ]
        for data, message in invalid:
            result = run(actual_command, data)
            assert result.returncode != 0 and not result.stdout, (data[:40], result.stdout)
            assert message in result.stderr.decode(), (data[:40], result.stderr.decode())
        for data in [
            b"@startditaa --no-shadows, scale=0.7\n+------+\n|cRED A|\n+------+\n@endditaa",
            b"+------------------------+\n| ]]> <script> & 'quotes' |\n+------------------------+",
            b"\xef\xbb\xbf+---+\r\n| A |\r\n+---+\r\n",
        ]:
            result = run(actual_command, data)
            assert result.returncode == 0, result.stderr.decode()
            tree = ET.fromstring(result.stdout)
            assert not list(tree.iter("{http://www.w3.org/2000/svg}script"))
            assert not list(tree.iter("{http://www.w3.org/2000/svg}image"))
        # An admitted worst-sized plane remains under the same native resource
        # boundary. Success, a JVM allocation failure or a CPU-limit exit are
        # permitted; timeout always kills/reaps the child before failing.
        dense = ((b"+--+" * 64 + b"\n" + b"|  |" * 64 + b"\n"
                  + b"+--+" * 64 + b"\n") * 42)
        started = time.monotonic()
        result = run(actual_command, dense)
        elapsed = time.monotonic() - started
        if result.returncode == 0:
            ET.fromstring(result.stdout)
        else:
            assert not result.stdout, "resource failure leaked a partial SVG"
        print(f"Ditaa: {checked} canonical geometry matches; {len(invalid)} errors and 3 special cases passed")
        print(f"Dense 256 x 126 cell plane retired in {elapsed:.2f}s (status {result.returncode})")
        if excluded:
            print("Excluded by explicit preview plane budget: " + ", ".join(excluded))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: probe.py COMPANION_JAR")
    main(sys.argv[1])
