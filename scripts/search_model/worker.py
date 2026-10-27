#!/usr/bin/env python3
"""Local JSON-lines embedding helper. Only accepts text or prepared media bytes."""
import argparse
import base64
import ctypes as c
import fcntl
import json
import math
import os
import signal
import runpy
from pathlib import Path
import struct
import io
import wave
import sys
import tempfile

DIMENSION = 768
REQUEST_LIMIT = 16 * 1024 * 1024


def emit(value):
    print(json.dumps(value, allow_nan=False), flush=True)


def configure(library):
    library.clip_search_backend_init.argtypes = [c.c_char_p, c.c_char_p]
    library.clip_search_device_name.argtypes = [c.c_char_p, c.c_size_t]
    library.clip_search_open.argtypes = [c.c_char_p, c.c_char_p, c.c_int, c.c_char_p, c.c_size_t]
    library.clip_search_open.restype = c.c_void_p
    library.clip_search_close.argtypes = [c.c_void_p]
    library.clip_search_embedding_size.argtypes = [c.c_void_p]
    library.clip_search_embedding_size.restype = c.c_int
    library.clip_search_token_count.argtypes = [c.c_void_p, c.c_char_p]
    library.clip_search_token_count.restype = c.c_int
    library.clip_search_embed_texts.argtypes = [c.c_void_p, c.POINTER(c.c_char_p), c.c_int, c.POINTER(c.c_float), c.c_char_p, c.c_size_t]
    library.clip_search_embed_media.argtypes = [c.c_void_p, c.c_char_p, c.c_int, c.c_int, c.POINTER(c.c_float), c.c_char_p, c.c_size_t]


def text_embedding(library, model, request, error):
    inputs = request.get("inputs")
    if not isinstance(inputs, list) or not 1 <= len(inputs) <= 4:
        raise ValueError("Submit one to four text inputs")
    limit = request.get("token_limit", 2048)
    if limit not in (512, 2048):
        raise ValueError("Invalid text token limit")
    encoded = []
    for text in inputs:
        if not isinstance(text, str) or "\0" in text or len(text.encode()) > 32768:
            raise ValueError("Invalid or overlong text input")
        text = text.encode()
        tokens = library.clip_search_token_count(model, text)
        if tokens > limit:
            raise ValueError(f"Input exceeds {limit}-token limit ({tokens} tokens)")
        encoded.append(text)
    output = (c.c_float * (DIMENSION * len(encoded)))()
    pointers = (c.c_char_p * len(encoded))(*encoded)
    result = library.clip_search_embed_texts(model, pointers, len(encoded), output, error, len(error))
    return output, result


def validate_media(op, data):
    if op == "image":
        if data[:8] != b"\x89PNG\r\n\x1a\n" or len(data) < 24 or data[12:16] != b"IHDR":
            raise ValueError("Submit a prepared PNG image")
        width, height = struct.unpack(">II", data[16:24])
        if not width or not height or max(width, height) > 4096 or width * height > 4_194_304:
            raise ValueError("Prepared image exceeds pixel budget")
    else:
        with wave.open(io.BytesIO(data), "rb") as audio:
            if audio.getnchannels() != 1 or audio.getframerate() != 16000 or audio.getsampwidth() != 2:
                raise ValueError("Submit prepared 16 kHz mono PCM16 WAV")
            if not 0 < audio.getnframes() <= 480000:
                raise ValueError("Submit a nonempty audio window of at most 30 seconds")


def media_embedding(library, model, request, error):
    op = request["op"]
    data = base64.b64decode(request.get("data", ""), validate=True)
    if not data or len(data) > 8 * 1024 * 1024:
        raise ValueError("Prepared media is empty or exceeds 8 MiB")
    budget = request.get("budget", 280)
    if budget not in (140, 280):
        raise ValueError("Unsupported visual token budget")
    validate_media(op, data)
    output = (c.c_float * DIMENSION)()
    with tempfile.TemporaryDirectory(prefix="clip-search-media-") as folder:
        path = Path(folder) / ("input.png" if op == "image" else "input.wav")
        path.write_bytes(data)
        os.chmod(path, 0o600)
        result = library.clip_search_embed_media(model, os.fsencode(path), op == "image", budget, output, error, len(error))
    return output, result


def normalized_output(output):
    rows = []
    for start in range(0, len(output), DIMENSION):
        values = list(output[start:start + DIMENSION])
        if not all(math.isfinite(value) for value in values):
            raise ValueError("Model returned nonfinite values")
        norm = math.sqrt(sum(value * value for value in values))
        if norm < 1e-12:
            raise ValueError("Model returned a zero embedding")
        rows.append([value / norm for value in values])
    packed = b"".join(struct.pack("<768f", *row) for row in rows)
    return {"dimension": DIMENSION, "count": len(rows), "vectors": base64.b64encode(packed).decode()}


def passage(library, model, request):
    """One exact-source passage; offsets are UTF-8 bytes, never decoded tokens."""
    text = request.get("text")
    if not isinstance(text, str) or not text or "\0" in text or len(text.encode()) > 8192:
        raise ValueError("Passage input must contain 1..8192 UTF-8 bytes without NUL")

    def count(value, framed=True):
        value = ("title: none | text: " + value) if framed else value
        result = library.clip_search_token_count(model, value.encode())
        if result <= 0:
            raise ValueError("Tokenizer returned an invalid count")
        return result

    # BPE token counts need not be strictly monotone. Every accepted boundary is
    # checked directly; binary search may choose a conservative shorter passage.
    low, high = 0, len(text)
    while low < high:
        middle = (low + high + 1) // 2
        if count(text[:middle]) <= 768:
            low = middle
        else:
            high = middle - 1
    end = low
    if not end:
        raise ValueError("A source character exceeds the passage token budget")
    if end < len(text):
        import re
        boundaries = list(re.finditer(r"\n[ \t]*\n|[.!?。！？](?:[ \t]+|\n)", text[:end]))
        preferred = next((match.end() for match in reversed(boundaries)
                          if match.end() >= end // 2 and count(text[:match.end()]) <= 768), None)
        if preferred is not None:
            end = preferred
    # Keep at most half the passage and 128 tokens as overlap. Always advance.
    low, high = 0, end // 2
    while low < high:
        middle = (low + high + 1) // 2
        if count(text[end - middle:end], framed=False) <= 128:
            low = middle
        else:
            high = middle - 1
    return {"end": len(text[:end].encode()), "next": len(text[:end - low].encode()),
            "tokens": count(text[:end]), "overlap_tokens": count(text[end - low:end], framed=False)}


def encode(library, model, request):
    if request.get("op") == "passage":
        return passage(library, model, request)
    error = c.create_string_buffer(1024)
    if request.get("op") == "text":
        output, result = text_embedding(library, model, request, error)
    elif request.get("op") in ("image", "audio"):
        output, result = media_embedding(library, model, request, error)
    else:
        raise ValueError("Unknown embedding operation")
    if result:
        raise ValueError(error.value.decode(errors="replace"))
    return normalized_output(output)


def attach_parent(parent):
    if parent <= 1 or os.getppid() != parent:
        raise RuntimeError("Model helper has no live owning parent")
    if sys.platform == "linux":
        libc = c.CDLL(None, use_errno=True)
        if libc.prctl(1, signal.SIGKILL, 0, 0, 0) != 0:
            raise OSError(c.get_errno(), "Cannot attach model helper lifetime to parent")
        if os.getppid() != parent:
            raise RuntimeError("Parent exited during model helper startup")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--device", choices=("auto", "cpu", "cuda", "vulkan"), default="auto")
    parser.add_argument("--text-only", action="store_true")
    parser.add_argument("--role", choices=("embedding", "answer"), default="embedding")
    parser.add_argument("--answer-provider", choices=("nuextract3", "qwen3.5", "qwen3.5-9b"), default="qwen3.5-9b")
    parser.add_argument("--parent", type=int, default=os.getppid())
    args = parser.parse_args()
    attach_parent(args.parent)
    args.runtime = args.runtime.resolve()
    args.models = args.models.resolve()
    with (args.runtime / "setup.lock").open("a") as ownership:
        fcntl.flock(ownership, fcntl.LOCK_SH | fcntl.LOCK_NB)
        serve(args)


def selected_device(library, requested):
    device = c.create_string_buffer(256)
    library.clip_search_device_name(device, len(device))
    name = device.value.decode()
    backend = next((kind for kind in ("cuda", "vulkan", "cpu") if name.lower().startswith(kind)), None)
    if backend is None or (requested != "auto" and backend != requested):
        raise RuntimeError(f"Requested {requested} backend is unavailable (selected {name})")
    return backend, name


def serve(args):
    model = None
    library = c.CDLL(str(args.runtime / "lib/libclip-search.so"))
    configure(library)
    # All model acquisition belongs to explicit setup.py, never this worker.
    library.clip_search_backend_init(os.fsencode(args.runtime / "lib"), b"" if args.device == "auto" else args.device.encode())
    backend, device = selected_device(library, args.device)
    if args.role == "answer":
        runpy.run_path(str(Path(__file__).with_name("answers.py")))["serve"](args, backend, device, emit, validate_media)
        return
    error = c.create_string_buffer(1024)
    text = args.models / "embeddinggemma-2-Q8_0.gguf"
    media = None if args.text_only else os.fsencode(args.models / "mmproj-embeddinggemma-2-Q8_0.gguf")
    model = library.clip_search_open(os.fsencode(text), media, 2048, error, len(error))
    if not model:
        raise RuntimeError(error.value.decode(errors="replace"))
    try:
        if library.clip_search_embedding_size(model) != DIMENSION:
            raise ValueError("Incompatible model dimension")
        manifest = json.loads((args.runtime / "runtime.json").read_text())
        pipeline = {key: manifest[key] for key in ("model_revision", "llama_revision", "shim_sha256")}
        emit({"ready": True, "dimension": DIMENSION, "device": device, "backend": backend, "text_only": args.text_only, "pipeline": pipeline})
        while line := sys.stdin.buffer.readline(REQUEST_LIMIT + 1):
            if len(line) > REQUEST_LIMIT:
                raise ValueError("Embedding request exceeds protocol budget")
            request_id = None
            try:
                request = json.loads(line)
                request_id = request.get("id")
                if request.get("op") == "quit":
                    emit({"id": request_id, "ok": True})
                    break
                emit({"id": request_id, "ok": True, **encode(library, model, request)})
            except (ValueError, TypeError, KeyError) as error:
                emit({"id": request_id, "ok": False, "error": str(error)[:500]})
    finally:
        library.clip_search_close(model)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        emit({"ready": False, "error": str(error)[:500]})
        sys.exit(2)
