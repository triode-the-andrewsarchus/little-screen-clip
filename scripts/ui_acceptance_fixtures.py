#!/usr/bin/env python3
"""Generate deterministic local fixtures for the Little Screen Clip UI acceptance runs.

Usage:
    python3 scripts/ui_acceptance_fixtures.py
    python3 scripts/ui_acceptance_fixtures.py --output-dir /tmp/clip-ui-fixtures

Without --output-dir, files go to a newly created system temporary directory. The
generator refuses to overwrite any of its output files.
"""

from __future__ import annotations

import argparse
import csv
import math
import struct
import tempfile
import wave
import zlib
from pathlib import Path


TEXT_LARGE_BYTES = 1_200_000
CSV_TALL_ROWS = 20_000
WAV_LARGE_SECONDS = 30
SAMPLE_RATE = 44_100


def text_fixtures(out: Path) -> None:
    small = "Little Screen Clip acceptance fixture\nUnicode: café — 東京 — 🖥️\n" * 8
    (out / "text-small.txt").write_text(small, encoding="utf-8")

    line = "Line with Unicode: café — 東京 — 🖥️; deterministic search token: CLIP-042\n"
    line_bytes = line.encode("utf-8")
    repeated = line_bytes * (TEXT_LARGE_BYTES // len(line_bytes) + 1)
    (out / "text-large-1m2.txt").write_bytes(repeated)

    (out / "markdown-long.md").write_text(
        "# Acceptance Markdown\n\n"
        "Search for `CLIP-042`, inspect the heading, code, list, and Unicode text below.\n\n"
        + "## Repeated section\n\n"
        + ("A paragraph with **bold**, _italic_, and 東京 characters.\n\n" * 1200),
        encoding="utf-8",
    )
    (out / "ansi-color.txt").write_text(
        "\x1b[1;32mANSI green and bold\x1b[0m\n"
        "\x1b[31mRed error sample\x1b[0m\n"
        "Plain text after reset\n",
        encoding="utf-8",
    )


def csv_fixtures(out: Path) -> None:
    with (out / "sheet-wide.csv").open("w", newline="", encoding="utf-8") as stream:
        writer = csv.writer(stream)
        writer.writerow([f"column_{index:04}" for index in range(1000)])
        writer.writerow([f"value_{index:04}" for index in range(1000)])

    with (out / "sheet-tall.csv").open("w", newline="", encoding="utf-8") as stream:
        writer = csv.writer(stream)
        writer.writerow(["row", "label", "value"])
        writer.writerows((index, f"record_{index:05}", index * 3) for index in range(1, CSV_TALL_ROWS + 1))


def binary_fixtures(out: Path) -> None:
    left = bytearray((index * 37 + 11) % 256 for index in range(8192))
    right = bytearray(left)
    right[17:17] = b"INSERTED"
    del right[2048:2080]
    right.extend(bytes((index * 19) % 256 for index in range(513)))
    (out / "binary-left.bin").write_bytes(left)
    (out / "binary-right.bin").write_bytes(right)


def png_chunk(kind: bytes, data: bytes) -> bytes:
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def image_fixture(out: Path) -> None:
    width, height = 96, 64
    rows = []
    for y in range(height):
        row = bytearray([0])
        for x in range(width):
            row.extend((x * 255 // (width - 1), y * 255 // (height - 1), 160, 255 if x > 8 else 0))
        rows.append(row)
    image = b"\x89PNG\r\n\x1a\n"
    image += png_chunk(b"IHDR", struct.pack(">2I5B", width, height, 8, 6, 0, 0, 0))
    image += png_chunk(b"IDAT", zlib.compress(b"".join(rows), level=9))
    image += png_chunk(b"IEND", b"")
    (out / "image-transparent.png").write_bytes(image)


def wav_fixture(path: Path, seconds: int, mode: str) -> None:
    frames = bytearray()
    for index in range(SAMPLE_RATE * seconds):
        if mode == "silence":
            left = right = 0
        else:
            if mode in {"large", "stereo"}:
                envelope = 1.0
            else:
                envelope = 1.0 if SAMPLE_RATE <= index < SAMPLE_RATE + 220 else 0.0
            left = int(12_000 * envelope * math.sin(2 * math.pi * 440 * index / SAMPLE_RATE))
            right = int(9_000 * envelope * math.sin(2 * math.pi * 660 * index / SAMPLE_RATE))
        frames.extend(struct.pack("<hh", left, right))
    with wave.open(str(path), "wb") as stream:
        stream.setnchannels(2)
        stream.setsampwidth(2)
        stream.setframerate(SAMPLE_RATE)
        stream.writeframes(frames)


def audio_fixtures(out: Path, large_audio_seconds: int) -> None:
    wav_fixture(out / "audio-stereo.wav", 3, "stereo")
    wav_fixture(out / f"audio-large-{large_audio_seconds}s.wav", large_audio_seconds, "large")
    wav_fixture(out / "audio-silence-transient.wav", 3, "transient")


def stl_fixture(out: Path) -> None:
    (out / "model-simple.stl").write_text(
        "solid acceptance_plane\n"
        "facet normal 0 0 1\n outer loop\n"
        " vertex 0 0 0\n vertex 1 0 0\n vertex 1 1 0\n"
        " endloop\nendfacet\n"
        "facet normal 0 0 1\n outer loop\n"
        " vertex 0 0 0\n vertex 1 1 0\n vertex 0 1 0\n"
        " endloop\nendfacet\nendsolid acceptance_plane\n",
        encoding="ascii",
    )


def generate(out: Path, large_audio_seconds: int = WAV_LARGE_SECONDS) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    names = [
        "text-small.txt", "text-large-1m2.txt", "markdown-long.md", "ansi-color.txt",
        "sheet-wide.csv", "sheet-tall.csv", "binary-left.bin", "binary-right.bin",
        "image-transparent.png", "audio-stereo.wav", f"audio-large-{large_audio_seconds}s.wav",
        "audio-silence-transient.wav", "model-simple.stl",
    ]
    conflicts = [name for name in names if (out / name).exists()]
    if conflicts:
        raise FileExistsError(f"Refusing to overwrite existing fixture(s): {', '.join(conflicts)}")
    text_fixtures(out)
    csv_fixtures(out)
    binary_fixtures(out)
    image_fixture(out)
    audio_fixtures(out, large_audio_seconds)
    stl_fixture(out)
    return [out / name for name in names]


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Create deterministic small/large UI acceptance inputs. Files: text-small.txt, "
            "text-large-1m2.txt, markdown-long.md, ansi-color.txt, sheet-wide.csv, "
            "sheet-tall.csv, binary-left.bin, binary-right.bin, image-transparent.png, "
            "audio-stereo.wav, audio-large-<seconds>s.wav, audio-silence-transient.wav, and "
            "model-simple.stl. The large WAV defaults to 30 seconds (~5.3 MB); use "
            "--large-audio-seconds 300 for 300 seconds (~53 MB). PDF and MIDI fixtures "
            "are deferred.\n\n"
            "Exact command: python3 scripts/ui_acceptance_fixtures.py --output-dir /tmp/clip-ui-fixtures"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--output-dir", type=Path, help="destination directory (default: new system temp directory)")
    parser.add_argument(
        "--large-audio-seconds",
        type=int,
        default=WAV_LARGE_SECONDS,
        metavar="SECONDS",
        help=f"duration of large stereo WAV (default: {WAV_LARGE_SECONDS}; 300 is about 53 MB)",
    )
    args = parser.parse_args()
    if args.large_audio_seconds < 1:
        parser.error("--large-audio-seconds must be at least 1")
    out = args.output_dir if args.output_dir else Path(tempfile.mkdtemp(prefix="clip-ui-fixtures-"))
    files = generate(out, args.large_audio_seconds)
    print(f"Generated {len(files)} fixtures in {out.resolve()}")
    for path in files:
        print(f"{path.name}\t{path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
