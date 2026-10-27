#!/usr/bin/env python3
"""Exercise real text/image/audio embeddings and protocol bounds without indexing files."""
import argparse
import base64
import io
import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import time
import wave
import zlib


def image(edge=96):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    row = b"\0" + bytes([210, 30, 30]) * edge
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", edge, edge, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(row * edge)) + chunk(b"IEND", b"")


def audio(seconds=1):
    stream = io.BytesIO()
    with wave.open(stream, "wb") as output:
        output.setnchannels(1)
        output.setsampwidth(2)
        output.setframerate(16000)
        output.writeframes(b"".join(struct.pack("<h", int(12000 * math.sin(2 * math.pi * 440 * i / 16000))) for i in range(16000 * seconds)))
    return stream.getvalue()


def vectors(response):
    assert response["ok"], response
    assert response["dimension"] == 768
    raw = base64.b64decode(response["vectors"], validate=True)
    values = struct.unpack("<" + "f" * (len(raw) // 4), raw)
    assert len(values) == response["count"] * 768
    rows = [values[i:i + 768] for i in range(0, len(values), 768)]
    for row in rows:
        assert all(math.isfinite(v) for v in row)
        assert abs(sum(v * v for v in row) - 1) < 1e-4
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--device", default="auto")
    args = parser.parse_args()
    requests = [
        {"id": 1, "op": "text", "inputs": [
            "task: search result | query: a document about cats",
            "title: none | text: Cats are small domesticated animals with whiskers and soft fur.",
            "title: none | text: A bank statement reports balances and financial transactions.",
        ]},
        {"id": 2, "op": "image", "data": base64.b64encode(image()).decode()},
        {"id": 3, "op": "audio", "data": base64.b64encode(audio()).decode()},
        {"id": 4, "op": "text", "inputs": ["word " * 1000], "token_limit": 512},
        {"id": 5, "op": "passage", "text": "A café. किताब पढ़ें。 音楽 🎵\n\n" * 120},
        {"id": 6, "op": "quit"},
    ]
    started = time.monotonic()
    result = subprocess.run([sys.executable, str(Path(__file__).with_name("worker.py")),
        "--runtime", str(args.runtime), "--models", str(args.models), "--device", args.device],
        input="".join(json.dumps(request) + "\n" for request in requests), text=True,
        capture_output=True, timeout=180)
    rows = [json.loads(line) for line in result.stdout.splitlines()]
    if result.returncode:
        raise RuntimeError(f"Helper failed: {rows}; {result.stderr[-2000:]}")
    assert rows[0]["ready"], rows[0]
    responses = {row["id"]: row for row in rows[1:]}
    text = vectors(responses[1])
    relevant = sum(a * b for a, b in zip(text[0], text[1]))
    irrelevant = sum(a * b for a, b in zip(text[0], text[2]))
    assert relevant > irrelevant, (relevant, irrelevant)
    vectors(responses[2])
    vectors(responses[3])
    assert not responses[4]["ok"] and "token" in responses[4]["error"]
    passage = responses[5]
    assert passage["ok"] and 0 < passage["next"] < passage["end"], passage
    assert passage["tokens"] <= 768 and passage["overlap_tokens"] <= 128, passage
    print(json.dumps({"device": rows[0]["device"], "seconds": round(time.monotonic() - started, 3),
        "text_relevant_cosine": relevant, "text_irrelevant_cosine": irrelevant,
        "modalities": ["text", "image", "audio"], "dimension": 768,
        "finite_normalized": True, "oversize_rejected": True, "tokenizer_passage": passage}, indent=2))


if __name__ == "__main__":
    main()
