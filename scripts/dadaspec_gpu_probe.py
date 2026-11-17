#!/usr/bin/env python3
"""Compare three CPU waveform previews with a native CUDA Dadaspec experiment.

Requires nvcc, CUDA/cuFFT and an NVIDIA GPU. Builds a development-only shared
library and an explicitly ignored Rust profile; neither desktop app loads it.
Each clip length runs in a fresh process to include native CUDA cold startup.
Uses three owned stereo WAV fixtures, guarded Symphonia decoding, and the actual
Rust normalization/compositor. GPU output must match the CPU pixels.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, nargs="+", default=[1, 10, 60, 300])
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--jobs", type=int, default=4, help="Cargo build jobs")
    parser.add_argument("--cpus", help="Optional taskset CPU list; allow three physical cores")
    args = parser.parse_args()
    if args.jobs < 1 or not all(1 <= seconds <= 300 for seconds in args.seconds):
        parser.error("clip lengths must be between one and 300 seconds")
    root = Path(__file__).resolve().parent.parent
    output = args.output_dir or Path(tempfile.mkdtemp(prefix="dadaspec-native-gpu-"))
    output.mkdir(parents=True, exist_ok=True)
    output = output.resolve()
    nvcc = shutil.which("nvcc") or "/opt/cuda/bin/nvcc"
    library = output / "libdadaspec_cuda_probe.so"
    subprocess.run([
        nvcc, "-O3", "--fmad=false", "--gpu-architecture=native", "--shared",
        "-Xcompiler", "-fPIC", str(root / "scripts/dadaspec_cuda_probe.cu"),
        "-lcufft", "-o", str(library),
    ], check=True)
    build = subprocess.Popen([
        "cargo", "test", "--release", "-p", "little-clip-core", "--lib",
        "native_cuda_three_file_budget", "--no-run", "--message-format=json", "-j", str(args.jobs),
    ], cwd=root, stdout=subprocess.PIPE, text=True)
    executable = None
    for line in build.stdout:
        record = json.loads(line)
        if record.get("reason") == "compiler-artifact" and record.get("executable"):
            executable = record["executable"]
    if build.wait() or not executable:
        raise RuntimeError("could not build the ignored native GPU profile")
    observations = []
    for seconds in args.seconds:
        env = dict(os.environ, DADASPEC_CUDA_PROBE=str(library), DADASPEC_PROFILE_SECONDS=str(seconds))
        command = [executable, "audio::dadaspec::profile::native_cuda_three_file_budget", "--exact", "--ignored", "--nocapture"]
        if args.cpus:
            command = ["taskset", "-c", args.cpus, *command]
        result = subprocess.run(command, cwd=root, env=env, text=True, capture_output=True)
        (output / f"native-{seconds}.log").write_text(result.stdout + result.stderr)
        result.check_returncode()
        for line in result.stdout.splitlines():
            if line.startswith("{"):
                observation = json.loads(line)
                observations.append(observation)
                print(json.dumps(observation), flush=True)
    (output / "results.json").write_text(json.dumps(observations, indent=2) + "\n")
    print(f"Artifacts: {output}")


if __name__ == "__main__":
    main()
