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


def pdf_fixture(out: Path, page_count: int, filename: str, *, page_streams: list[bytes] | None = None) -> None:
    # Small hand-built PDF keeps this generator dependency-free and byte-stable.
    if page_streams is not None and len(page_streams) != page_count:
        raise ValueError("Each PDF page must have one content stream")
    def text_stream(content: bytes) -> bytes:
        return b"<< /Length " + str(len(content)).encode("ascii") + b" >>\nstream\n" + content + b"\nendstream"

    kids = b" ".join(f"{3 + 2 * index} 0 R".encode("ascii") for index in range(page_count))
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>"]
    objects.append(b"<< /Type /Pages /Kids [" + kids + f"] /Count {page_count} >>".encode("ascii"))
    for index in range(page_count):
        page_num = index + 1
        objects.append(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Resources << /Font << /F1 {3 + page_count * 2} 0 R >> >> /Contents {4 + index * 2} 0 R >>".encode("ascii")
        )
        content = (page_streams[index] if page_streams is not None else
                   b"BT /F1 18 Tf 40 140 Td (" + f"Acceptance page {page_num:02}".encode("ascii") + b") Tj ET")
        objects.append(text_stream(content))
    objects.append(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
    data = bytearray(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0]
    for number, body in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{number} 0 obj\n".encode("ascii") + body + b"\nendobj\n")
    xref = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode("ascii"))
    for offset in offsets[1:]:
        data.extend(f"{offset:010} 00000 n \n".encode("ascii"))
    data.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode("ascii"))
    (out / filename).write_bytes(data)


def dense_midi_fixture(out: Path, track_count: int = 24) -> None:
    def vlq(value: int) -> bytes:
        encoded = [value & 0x7F]
        value >>= 7
        while value:
            encoded.insert(0, (value & 0x7F) | 0x80)
            value >>= 7
        return bytes(encoded)

    tracks = []
    for index in range(track_count):
        name = f"Track {index + 1:02}".encode("ascii")
        events = b"\x00\xff\x03" + vlq(len(name)) + name + bytes([0, 0xC0 | (index % 16), index % 8])
        pitch = 48 + index % 36
        for beat in range(16):
            events += bytes([0, 0x90 | (index % 16), pitch + beat % 5, 48 + (index * 7 + beat * 3) % 80])
            events += vlq(120) + bytes([0x80 | (index % 16), pitch + beat % 5, 0])
        events += b"\x00\xff\x2f\x00"
        tracks.append(b"MTrk" + struct.pack(">I", len(events)) + events)
    payload = b"MThd" + struct.pack(">IHHH", 6, 1, track_count, 480) + b"".join(tracks)
    (out / "music-24-track.mid").write_bytes(payload)


def midi_fixture(out: Path) -> None:
    def vlq(value: int) -> bytes:
        encoded = [value & 0x7F]
        value >>= 7
        while value:
            encoded.insert(0, (value & 0x7F) | 0x80)
            value >>= 7
        return bytes(encoded)

    def track(events: bytes) -> bytes:
        return b"MTrk" + struct.pack(">I", len(events)) + events

    tempo = track(b"\x00\xff\x51\x03\x07\xa1\x20\x00\xff\x2f\x00")
    melody_events = b"\x00\xc0\x00" + b"\x00\x90\x3c\x64" + vlq(480) + b"\x80\x3c\x00\x00\xff\x2f\x00"
    bass_events = b"\x00\xc1\x21" + b"\x00\x91\x30\x50" + vlq(960) + b"\x81\x30\x00\x00\xff\x2f\x00"
    payload = b"MThd" + struct.pack(">IHHH", 6, 1, 3, 480) + tempo + track(melody_events) + track(bass_events)
    (out / "music-three-track.mid").write_bytes(payload)


def exif_tiff_fixture(out: Path) -> None:
    # Minimal little-endian RGB TIFF with Orientation and DateTime EXIF tags.
    width, height = 48, 32
    pixels = bytes(channel for y in range(height) for x in range(width) for channel in (x * 255 // (width - 1), y * 255 // (height - 1), 96))
    entries = [
        (256, 4, 1, width), (257, 4, 1, height), (258, 3, 3, None),
        (259, 3, 1, 1), (262, 3, 1, 2), (273, 4, 1, None),
        (274, 3, 1, 6), (277, 3, 1, 3), (278, 4, 1, height),
        (279, 4, 1, len(pixels)), (284, 3, 1, 1), (306, 2, 20, None),
    ]
    ifd_offset = 8
    extra_offset = ifd_offset + 2 + len(entries) * 12 + 4
    bits_offset = extra_offset
    datetime_offset = bits_offset + 6
    pixel_offset = datetime_offset + 20
    values = {258: bits_offset, 273: pixel_offset, 306: datetime_offset}
    data = bytearray(b"II*\x00" + struct.pack("<I", ifd_offset))
    data.extend(struct.pack("<H", len(entries)))
    for tag, kind, count, value in entries:
        value = values.get(tag, value)
        if kind == 3 and count == 1:
            packed_value = struct.pack("<H", value) + b"\x00\x00"
        elif kind == 4:
            packed_value = struct.pack("<I", value)
        else:
            packed_value = struct.pack("<I", value)
        data.extend(struct.pack("<HHI", tag, kind, count) + packed_value)
    data.extend(b"\x00\x00\x00\x00")
    data.extend(struct.pack("<HHH", 8, 8, 8))
    data.extend(b"2026:01:02 03:04:05\x00")
    data.extend(pixels)
    (out / "image-exif.tiff").write_bytes(data)


def svg_fixture(out: Path) -> None:
    (out / "image-simple.svg").write_text(
        '<svg xmlns="http://www.w3.org/2000/svg" width="160" height="90" viewBox="0 0 160 90">'
        '<rect width="160" height="90" fill="#243b53"/><circle cx="80" cy="45" r="28" fill="#f0b429"/>'
        '<text x="80" y="84" text-anchor="middle" fill="white">CLIP SVG</text></svg>\n', encoding="utf-8")


def malformed_fixtures(out: Path) -> None:
    # Deliberately truncated/corrupt synthetic inputs exercise decoder failures.
    (out / "document-malformed.pdf").write_bytes(
        b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\n"
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n"
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>\n"
        b"xref\nthis is not a cross-reference table\n%%EOF\n"
    )
    (out / "music-malformed.mid").write_bytes(b"MThd\x00\x00\x00\x06\x00\x01")
    (out / "image-malformed.png").write_bytes(
        b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", struct.pack(">2I5B", 8, 8, 8, 6, 0, 0, 0))
        + b"\x00\x00\x00\x10IDAT\x78\x9c\x00"
    )
    (out / "text-invalid-utf8.txt").write_bytes(
        b"Synthetic text preview decoding failure fixture\nInvalid UTF-8 byte: \xff\xfe\n"
    )


def validate_added_fixtures(out: Path) -> None:
    pdf = (out / "document-two-page.pdf").read_bytes()
    dense_pdf = (out / "document-80-page.pdf").read_bytes()
    midi = (out / "music-three-track.mid").read_bytes()
    dense_midi = (out / "music-24-track.mid").read_bytes()
    tiff = (out / "image-exif.tiff").read_bytes()
    svg = (out / "image-simple.svg").read_text(encoding="utf-8")
    malformed_pdf = (out / "document-malformed.pdf").read_bytes()
    malformed_midi = (out / "music-malformed.mid").read_bytes()
    malformed_png = (out / "image-malformed.png").read_bytes()
    malformed_text = (out / "text-invalid-utf8.txt").read_bytes()
    assert pdf.startswith(b"%PDF-1.4") and b"/Count 2" in pdf
    assert dense_pdf.startswith(b"%PDF-1.4") and b"/Count 80" in dense_pdf
    assert midi[:4] == b"MThd" and struct.unpack_from(">H", midi, 10)[0] == 3
    assert dense_midi[:4] == b"MThd" and struct.unpack_from(">H", dense_midi, 10)[0] == 24
    assert tiff[:4] == b"II*\x00" and struct.pack("<H", 274) in tiff and b"2026:01:02 03:04:05" in tiff
    assert svg.startswith("<svg ") and "<circle" in svg
    assert malformed_pdf.startswith(b"%PDF-1.4") and b"not a cross-reference table" in malformed_pdf
    assert malformed_midi.startswith(b"MThd") and len(malformed_midi) < 14
    assert malformed_png.startswith(b"\x89PNG\r\n\x1a\n") and b"IDAT" in malformed_png and not malformed_png.endswith(b"IEND")
    assert b"\xff\xfe" in malformed_text


def generate(out: Path, large_audio_seconds: int = WAV_LARGE_SECONDS) -> list[Path]:
    out.mkdir(parents=True, exist_ok=True)
    names = [
        "text-small.txt", "text-large-1m2.txt", "markdown-long.md", "ansi-color.txt",
        "sheet-wide.csv", "sheet-tall.csv", "binary-left.bin", "binary-right.bin",
        "image-transparent.png", "audio-stereo.wav", f"audio-large-{large_audio_seconds}s.wav",
        "audio-silence-transient.wav", "model-simple.stl",
        "document-two-page.pdf", "music-three-track.mid", "image-exif.tiff", "image-simple.svg",
        "document-80-page.pdf", "music-24-track.mid",
        "document-malformed.pdf", "music-malformed.mid", "image-malformed.png", "text-invalid-utf8.txt",
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
    pdf_fixture(out, 2, "document-two-page.pdf")
    midi_fixture(out)
    pdf_fixture(out, 80, "document-80-page.pdf")
    dense_midi_fixture(out)
    exif_tiff_fixture(out)
    svg_fixture(out)
    malformed_fixtures(out)
    validate_added_fixtures(out)
    return [out / name for name in names]


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Create deterministic small/large UI acceptance inputs. Files: text-small.txt, "
            "text-large-1m2.txt, markdown-long.md, ansi-color.txt, sheet-wide.csv, "
            "sheet-tall.csv, binary-left.bin, binary-right.bin, image-transparent.png, "
            "audio-stereo.wav, audio-large-<seconds>s.wav, audio-silence-transient.wav, and "
            "model-simple.stl, document-two-page.pdf, document-80-page.pdf, music-three-track.mid, "
            "music-24-track.mid, image-exif.tiff, image-simple.svg, document-malformed.pdf, "
            "music-malformed.mid, image-malformed.png, "
            "and text-invalid-utf8.txt. The large WAV defaults to 30 seconds (~5.3 MB); use "
            "--large-audio-seconds 300 for 300 seconds (~53 MB).\n\n"
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
