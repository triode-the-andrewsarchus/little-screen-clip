#!/usr/bin/env python3
"""
AI Background Removal Worker for little-screen-clip using feyninc/nobg (FeyNoBg).

Usage:
    python3 remove_bg.py <input_path> <output_path>
    python3 remove_bg.py - -    (reads PNG from stdin, writes PNG to stdout)
"""

import os
import sys
import time

# Auto-reexec inside little-screen-clip venv if nobg is not in current environment
try:
    import nobg  # noqa: F401
except ImportError:
    user_venv = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
    if os.path.exists(user_venv) and sys.executable != user_venv:
        os.execv(user_venv, [user_venv] + sys.argv)
    else:
        sys.stderr.write(
            "ERROR: 'nobg' package not found.\n"
            "Install it via:\n"
            "  uv venv --python /usr/bin/python3 --system-site-packages ~/.local/share/little-screen-clip/nobg-venv\n"
            "  uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python nobg\n"
        )
        sys.exit(1)

import io
import torch
from PIL import Image
from nobg import AutoModel


def main():
    if len(sys.argv) < 3:
        sys.stderr.write("Usage: remove_bg.py <input_path|-> <output_path|->\n")
        sys.exit(2)

    input_arg = sys.argv[1]
    output_arg = sys.argv[2]

    # 1. Read input image
    t_start = time.perf_counter()
    if input_arg == "-":
        img_bytes = sys.stdin.buffer.read()
        if not img_bytes:
            sys.stderr.write("ERROR: No input image data received on stdin.\n")
            sys.exit(1)
        image = Image.open(io.BytesIO(img_bytes)).convert("RGB")
    else:
        if not os.path.exists(input_arg):
            sys.stderr.write(f"ERROR: Input file not found: {input_arg}\n")
            sys.exit(1)
        image = Image.open(input_arg).convert("RGB")

    orig_w, orig_h = image.size

    # 2. Load model (cached locally via HF)
    t_load_start = time.perf_counter()
    use_cuda = torch.cuda.is_available()
    device = "cuda" if use_cuda else "cpu"

    # Suppress verbose HF hub warnings
    os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")

    model = AutoModel.from_pretrained("feyninc/FeyNobg")
    if use_cuda:
        model = model.to(device)
    model.eval()

    t_load = time.perf_counter() - t_load_start

    # 3. Process image with FeyNoBg
    t_infer_start = time.perf_counter()
    with torch.inference_mode():
        cutout = model.process(image)
        if use_cuda:
            torch.cuda.synchronize()

    t_infer = time.perf_counter() - t_infer_start
    t_total = time.perf_counter() - t_start

    # Ensure cutout matches original resolution
    if cutout.size != (orig_w, orig_h):
        cutout = cutout.resize((orig_w, orig_h), Image.Resampling.LANCZOS)

    # 4. Save output
    if output_arg == "-":
        out_buf = io.BytesIO()
        cutout.save(out_buf, format="PNG")
        sys.stdout.buffer.write(out_buf.getvalue())
        sys.stdout.buffer.flush()
    else:
        out_dir = os.path.dirname(os.path.abspath(output_arg))
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        cutout.save(output_arg, format="PNG")

    sys.stderr.write(
        f"[nobg] Success: {orig_w}x{orig_h} on {device.upper()} (Load: {t_load*1000:.0f}ms, Infer: {t_infer*1000:.0f}ms, Total: {t_total*1000:.0f}ms)\n"
    )


if __name__ == "__main__":
    main()
