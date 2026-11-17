#!/usr/bin/env python3
"""Regenerate tiny upstream reference pixels; numpy is needed only for this script.

Pass a checkout of https://github.com/dada-bots/dadaspec at the revision recorded
in docs/dadaspec.md. No music or other upstream example assets are copied.
"""
import argparse
import importlib.util
from pathlib import Path

import numpy as np


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("upstream", type=Path, help="Path to upstream dadaspec.py")
    args = parser.parse_args()
    spec = importlib.util.spec_from_file_location("upstream_dadaspec", args.upstream)
    upstream = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(upstream)
    destination = Path(__file__).resolve().parents[1] / "crates/little-clip-core/src/audio/fixtures"
    for rate, frames in [(32000, 14336), (44100, 44100)]:
        index = np.arange(frames, dtype=np.int64)
        left = (((index * 97) % 1024) - 512).astype(np.float32) / 2048
        right = (((index * 31) % 2048) - 1024).astype(np.float32) / 4096
        left[(index // 2048) % 3 == 1] = 0
        right[(index // 2048) % 4 == 2] = 0
        samples = upstream.resample_linear(np.stack([left, right]), rate, 32000)
        left_mel, left_rgb = upstream._mel_channel(samples[0], 32000)
        right_mel, right_rgb = upstream._mel_channel(samples[1], 32000)
        rgb = np.concatenate([left_mel[::-1, :, None] * left_rgb[None, :, :],
                              right_mel[:, :, None] * right_rgb[None, :, :]], axis=0)
        rgb = (np.clip(rgb, 0, 1) * 255).astype(np.uint8)
        rgba = np.concatenate([rgb, np.full((*rgb.shape[:2], 1), 255, dtype=np.uint8)], axis=2)
        (destination / f"dadaspec_{rate}.rgba").write_bytes(rgba.tobytes())


if __name__ == "__main__":
    main()
