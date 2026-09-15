#!/usr/bin/env python3
"""Run HTDemucs v4 locally and report the four standard output stems as JSON."""

import argparse
import json
import os
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description="Separate an audio file with HTDemucs v4.")
    parser.add_argument("input", help="Input audio path")
    parser.add_argument("output", help="Temporary output directory")
    args = parser.parse_args()

    # Match the other local AI workers: prefer the app-managed venv automatically.
    venv_python = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
    if os.path.exists(venv_python) and sys.executable != venv_python:
        os.execv(venv_python, [venv_python] + sys.argv)

    try:
        import demucs  # noqa: F401
    except ImportError:
        sys.stderr.write(
            "ERROR: 'demucs' package not found. Install it via:\n"
            "  uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python demucs\n"
        )
        sys.exit(1)

    result = subprocess.run(
        [sys.executable, "-m", "demucs.separate", "--name", "htdemucs", "--out", args.output, args.input],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    # Keep stdout machine-readable for the Rust UI. Demucs progress belongs on stderr.
    if result.stdout:
        sys.stderr.write(result.stdout)
    if result.stderr:
        sys.stderr.write(result.stderr)
    if result.returncode != 0:
        sys.exit(result.returncode)

    source_stem = os.path.splitext(os.path.basename(args.input))[0]
    stem_dir = os.path.join(args.output, "htdemucs", source_stem)
    stems = {name: os.path.join(stem_dir, f"{name}.wav") for name in ("vocals", "drums", "bass", "other")}
    missing = [path for path in stems.values() if not os.path.isfile(path)]
    if missing:
        sys.stderr.write("ERROR: Demucs finished without all expected stem files.\n")
        sys.exit(1)
    print(json.dumps(stems))


if __name__ == "__main__":
    main()
