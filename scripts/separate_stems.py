#!/usr/bin/env python3
"""Separate a prepared, inherited WAV input using Demucs' tensor API."""
import argparse
import contextlib
import json
import os
import sys


def separate(input_path, output_directory):
    # Rust has normalized the admitted source to a pinned WAV. Decode it here,
    # rather than allowing a generic CLI to reopen the original or lose its FD
    # in another Python/FFmpeg subprocess.
    import sphn
    import torch
    from demucs.api import Separator
    from demucs.audio import convert_audio, save_audio

    audio, sample_rate = sphn.read(input_path)
    with contextlib.redirect_stdout(sys.stderr):
        separator = Separator(model="htdemucs", progress=True)
        tensor = convert_audio(torch.from_numpy(audio), int(sample_rate),
                               separator.samplerate, separator.audio_channels)
        _, separated = separator.separate_tensor(tensor, separator.samplerate)
        for name in ("vocals", "drums", "bass", "other"):
            # The output directory is an inherited pinned private job directory.
            # Only these fixed relative names can appear in the manifest.
            save_audio(separated[name].cpu(), os.path.join(output_directory, name + ".wav"),
                       separator.samplerate)
    return {name:name + ".wav" for name in ("vocals", "drums", "bass", "other")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", help="Inherited prepared WAV descriptor path")
    parser.add_argument("output", help="Inherited output-directory descriptor path")
    args = parser.parse_args()
    venv_python = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
    if os.path.exists(venv_python) and sys.executable != venv_python:
        # CommandRequest inherits these descriptors without close-on-exec. Keep
        # them across interpreter replacement and retain isolated Python mode.
        os.execv(venv_python, [venv_python, "-I"] + sys.argv)
    try:
        manifest = separate(args.input, args.output)
    except ImportError as error:
        sys.stderr.write("ERROR: Demucs tensor API dependencies unavailable: " + str(error) +
                         "\nInstall demucs>=4.1 with its dependencies in the app-managed venv.\n")
        return 1
    print(json.dumps(manifest))
    return 0


if __name__ == "__main__":
    sys.exit(main())
