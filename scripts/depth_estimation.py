#!/usr/bin/env python3
"""
AI Depth Estimation Worker for little-screen-clip.
Extracts high-resolution depth maps from images using Depth-Anything / MiDaS via transformers.

Usage:
    python3 depth_estimation.py <input_path|-> <output_path|-> [--invert] [--colormap {gray,inferno,turbo}]
"""

import os
import sys
import time

# Auto-reexec inside little-screen-clip venv if transformers is not in current environment
try:
    import transformers  # noqa: F401
    import torch  # noqa: F401
except ImportError:
    user_venv = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
    if os.path.exists(user_venv) and sys.executable != user_venv:
        os.execv(user_venv, [user_venv] + sys.argv)
    else:
        sys.stderr.write(
            "ERROR: 'transformers' / 'torch' package not found.\n"
            "Install it via:\n"
            "  uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python transformers\n"
        )
        sys.exit(1)

import argparse
import io
import torch
from PIL import Image, ImageOps
from transformers import pipeline


def main():
    parser = argparse.ArgumentParser(description="Estimate depth map from an input image.")
    parser.add_argument("input", help="Path to input image, or '-' for stdin")
    parser.add_argument("output", help="Path to output image, or '-' for stdout")
    parser.add_argument("--invert", action="store_true", help="Invert depth values (near becomes dark, far becomes light)")
    parser.add_argument("--model", default="depth-anything/Depth-Anything-V2-Small-hf", help="Hugging Face depth model")

    args = parser.parse_args()

    t_start = time.perf_counter()

    # 1. Read input image
    if args.input == "-":
        img_bytes = sys.stdin.buffer.read()
        if not img_bytes:
            sys.stderr.write("ERROR: No input image data received on stdin.\n")
            sys.exit(1)
        image = Image.open(io.BytesIO(img_bytes)).convert("RGB")
    else:
        if not os.path.exists(args.input):
            sys.stderr.write(f"ERROR: Input file not found: {args.input}\n")
            sys.exit(1)
        image = Image.open(args.input).convert("RGB")

    orig_w, orig_h = image.size

    # 2. Setup model & device
    use_cuda = torch.cuda.is_available()
    device = "cuda" if use_cuda else "cpu"

    os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")

    t_load_start = time.perf_counter()
    pipe = pipeline(
        task="depth-estimation",
        model=args.model,
        device=device,
    )
    t_load = time.perf_counter() - t_load_start

    # 3. Perform depth inference
    t_infer_start = time.perf_counter()
    result = pipe(image)
    if use_cuda:
        torch.cuda.synchronize()
    t_infer = time.perf_counter() - t_infer_start

    depth_img = result["depth"]

    # Invert if requested
    if args.invert:
        depth_img = ImageOps.invert(depth_img)

    # Resize back to exact original resolution
    if depth_img.size != (orig_w, orig_h):
        depth_img = depth_img.resize((orig_w, orig_h), Image.Resampling.BILINEAR)

    # 4. Save output as grayscale PNG
    if args.output == "-":
        out_buf = io.BytesIO()
        depth_img.save(out_buf, format="PNG")
        sys.stdout.buffer.write(out_buf.getvalue())
        sys.stdout.buffer.flush()
    else:
        out_dir = os.path.dirname(os.path.abspath(args.output))
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        depth_img.save(args.output, format="PNG")

    t_total = time.perf_counter() - t_start
    sys.stderr.write(
        f"[depth] Success: {orig_w}x{orig_h} on {device.upper()} (Load: {t_load*1000:.0f}ms, Infer: {t_infer*1000:.0f}ms, Total: {t_total*1000:.0f}ms)\n"
    )


if __name__ == "__main__":
    main()
