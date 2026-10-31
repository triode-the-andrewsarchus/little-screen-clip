#!/usr/bin/env python3
"""Explicitly provision the pinned local embedding runtime; never enroll folders."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import urllib.request

REVISION = "bfcd298762cc34d0357ece5ebdd31791a3a374d8"
LLAMA_REVISION = "4d756bc72bf00a4aacf410ae15a2d315f3db400d"  # b11461
FILES = (
    ("embeddinggemma-2-Q8_0.gguf", 309855456, "2188ac1deca4b77dffefd603c2776a9d76d9d74ec01841392982ebb840b09135"),
    ("mmproj-embeddinggemma-2-Q8_0.gguf", 554821024, "c4a8a52691ecef40618438928bdf9e68379b854e24166f292592353db0aab64f"),
)
HERE = Path(__file__).resolve().parent


def emit(stage, **data):
    print(json.dumps({"stage": stage, **data}), flush=True)


def digest(path):
    with path.open("rb") as stream:
        value = hashlib.sha256()
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b""):
            value.update(block)
        return value.hexdigest()


def download(folder, files=FILES, repository="ggml-org/embeddinggemma-2-GGUF", revision=REVISION):
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    for name, size, checksum in files:
        target = folder / name
        if target.is_file() and target.stat().st_size == size and digest(target) == checksum:
            continue
        # Keep the old final file until an authenticated replacement is complete.
        partial = folder / (name + ".part")
        emit("download", file=name, bytes=size)
        url = f"https://huggingface.co/{repository}/resolve/{revision}/{name}"
        value, total, reported = hashlib.sha256(), 0, time.monotonic()
        try:
            with urllib.request.urlopen(url, timeout=60) as response, partial.open("wb") as output:
                os.chmod(partial, 0o600)
                while block := response.read(1024 * 1024):
                    total += len(block)
                    if total > size:
                        raise ValueError("Model download exceeds pinned size")
                    value.update(block)
                    output.write(block)
                    if time.monotonic() - reported >= 1:
                        emit("download", file=name, received=total, bytes=size)
                        reported = time.monotonic()
                output.flush()
                os.fsync(output.fileno())
            if total != size or value.hexdigest() != checksum:
                raise ValueError(f"Pinned model checksum/size mismatch: {name}")
            partial.replace(target)
        finally:
            partial.unlink(missing_ok=True)
    emit("models_ready", revision=revision)


def download_answer(folder, provider):
    spec = json.loads((HERE / "answer_models.json").read_text())[provider]
    download(folder, spec["files"] + ([spec["vision_file"]] if "vision_file" in spec else []), spec["repository"], spec["revision"])


def run(args):
    subprocess.run([str(arg) for arg in args], check=True, stdout=subprocess.DEVNULL)


def build_backends(prebuilt=None):
    if prebuilt:
        fields = (prebuilt.resolve() / "BUILD").read_text().split()
        if fields[:2] != ["b11461", "33c8c4812a43"]:
            raise ValueError("Prebuilt llama prefix does not match the pinned patched build")
        if "cpu" not in fields[2:]:
            raise ValueError("Prebuilt llama prefix must retain CPU fallback")
        return sorted(set(fields[2:]) & {"cpu", "cuda", "vulkan"})
    backends = ["cpu"]
    if shutil.which("nvcc"):
        backends.append("cuda")
    if shutil.which("glslc") and Path("/usr/include/vulkan/vulkan.h").is_file():
        backends.append("vulkan")
    return backends


def build_prefix(folder, backends):
    source = folder / "llama.cpp"
    if not (source / ".git").is_dir():
        run(["git", "clone", "--depth", "1", "--branch", "b11461", "https://github.com/ggml-org/llama.cpp", source])
    actual = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
    if actual != LLAMA_REVISION:
        raise ValueError("llama.cpp source differs from the pinned commit")
    clean_patch = subprocess.run(["git", "-C", str(source), "apply", "--check", str(HERE / "llama-no-logits.patch")], capture_output=True)
    if clean_patch.returncode == 0:
        run(["git", "-C", source, "apply", HERE / "llama-no-logits.patch"])
    else:
        run(["git", "-C", source, "apply", "--reverse", "--check", HERE / "llama-no-logits.patch"])
    output = folder / "build"
    cuda, vulkan = "cuda" in backends, "vulkan" in backends
    run(["cmake", "-S", source, "-B", output, "-DCMAKE_BUILD_TYPE=Release",
         "-DGGML_NATIVE=OFF", "-DBUILD_SHARED_LIBS=ON", "-DGGML_BACKEND_DL=ON",
         "-DGGML_CPU_ALL_VARIANTS=ON", f"-DGGML_CUDA={'ON' if cuda else 'OFF'}",
         f"-DGGML_VULKAN={'ON' if vulkan else 'OFF'}", "-DLLAMA_BUILD_MTMD=ON",
         "-DLLAMA_BUILD_COMMON=OFF", "-DLLAMA_BUILD_TOOLS=OFF", "-DLLAMA_BUILD_SERVER=OFF",
         "-DLLAMA_BUILD_EXAMPLES=OFF", "-DLLAMA_BUILD_TESTS=OFF", "-DLLAMA_BUILD_APP=OFF",
         "-DLLAMA_BUILD_UI=OFF", "-DLLAMA_OPENSSL=OFF", "-DCMAKE_BUILD_WITH_INSTALL_RPATH=ON",
         "-DCMAKE_INSTALL_RPATH=$ORIGIN"])
    run(["cmake", "--build", output, "-j", "4"])
    prefix = folder / "prefix"
    (prefix / "lib").mkdir(parents=True, exist_ok=True)
    (prefix / "include").mkdir(parents=True, exist_ok=True)
    for path in output.rglob("*.so*"):
        if path.is_file():
            shutil.copy2(path, prefix / "lib" / path.name)
    for header in [* (source / "include").glob("*.h"), * (source / "ggml/include").glob("*.h"),
                   source / "tools/mtmd/mtmd.h", source / "tools/mtmd/mtmd-helper.h"]:
        shutil.copy2(header, prefix / "include" / header.name)
    return prefix


def build(folder, prebuilt):
    backends = build_backends(prebuilt)
    stamp = hashlib.sha256((HERE / "llama_shim.c").read_bytes() + (HERE / "llama-no-logits.patch").read_bytes()).hexdigest()
    manifest = folder / "runtime.json"
    data = {"llama_revision": LLAMA_REVISION, "shim_sha256": stamp, "model_revision": REVISION,
            "backends": backends, "prebuilt": str(prebuilt.resolve()) if prebuilt else None,
            "answer_shim_sha256": digest(HERE / "answer_shim.c")}
    if manifest.is_file() and all((folder / "lib" / name).is_file() for name in ("libclip-search.so", "libclip-answer.so")):
        installed = json.loads(manifest.read_text())
        if all(installed.get(key) == value for key, value in data.items()):
            return
    emit("build", revision=LLAMA_REVISION, backends=backends)
    if prebuilt:
        # An explicit developer-only shortcut for a reviewed, already built b11461 prefix.
        prefix = prebuilt.resolve()
    else:
        prefix = build_prefix(folder, backends)
    with tempfile.TemporaryDirectory(prefix=".runtime-", dir=folder) as temporary:
        staged = Path(temporary)
        lib = staged / "lib"
        lib.mkdir()
        for path in (prefix / "lib").glob("*.so*"):
            backend = next((name for name in ("cuda", "vulkan") if path.name.startswith(f"libggml-{name}.")), None)
            if backend is None or backend in backends:
                shutil.copy2(path, lib / path.name)
        for source, binary in [("llama_shim.c", "libclip-search.so"), ("answer_shim.c", "libclip-answer.so")]:
            run(["cc", "-O2", "-fPIC", "-shared", "-std=c11", HERE / source,
                 "-I", prefix / "include", "-L", lib, "-Wl,-rpath,$ORIGIN",
                 "-lmtmd", "-lllama", "-lggml", "-lggml-base", "-lm", "-o", lib / binary])
        install_runtime(folder, staged, data)
    # Retain source and licenses alongside installed binary artifacts.
    for name in ["llama_shim.c", "answer_shim.c", "llama-no-logits.patch", "LICENSE.recall"]:
        shutil.copy2(HERE / name, folder / name)
    emit("runtime_ready", **data)


def install_runtime(folder, staged, data):
    lib, backup = folder / "lib", folder / "lib.previous"
    if backup.exists():
        if not lib.exists():
            backup.rename(lib)
        else:
            shutil.rmtree(backup)
    pending = staged / "runtime.json"
    pending.write_text(json.dumps(data, indent=2) + "\n")
    if lib.exists():
        lib.rename(backup)
    try:
        (staged / "lib").rename(lib)
        pending.replace(folder / "runtime.json")
    except BaseException:
        if lib.exists():
            shutil.rmtree(lib)
        if backup.exists():
            backup.rename(lib)
        raise
    if backup.exists():
        shutil.rmtree(backup)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--models", type=Path, required=True)
    parser.add_argument("--llama-prefix", type=Path)
    parser.add_argument("--answers", choices=("nuextract3", "qwen3.5", "qwen3.5-9b"))
    args = parser.parse_args()
    args.runtime.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (args.runtime / "setup.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        build(args.runtime, args.llama_prefix)
        for name in ["llama_shim.c", "answer_shim.c", "llama-no-logits.patch", "LICENSE.recall", "LICENSE.llama.cpp", "MODEL_NOTICES.md", "LICENSE.models.apache-2.0", "LICENSE.models.qwen", "worker.py", "inference_errors.py", "answers.py", "applicability.py", "vision_answers.py", "answer_models.json", "documents.py"]:
            shutil.copy2(HERE / name, args.runtime / name)
        download(args.models)
        if args.answers:
            download_answer(args.models, args.answers)
        emit("ready")


if __name__ == "__main__":
    main()
