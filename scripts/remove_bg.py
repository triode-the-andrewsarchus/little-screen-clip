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


import argparse

def main():
    parser = argparse.ArgumentParser(description="AI Background Removal & MultiMatte Segmentation")
    parser.add_argument("input", help="Path to input image, or '-' for stdin")
    parser.add_argument("output", help="Path to output image, or '-' for stdout")
    parser.add_argument("--prompt", "-p", default=None, help="Text concept prompt for MultiMatte segmentation (e.g. 'person', 'car', 'flower')")
    parser.add_argument("--model", "-m", default=None, help="Specific model checkpoint (defaults to feyninc/multimatte if prompt given, else feyninc/FeyNobg)")

    args = parser.parse_args()

    input_arg = args.input
    output_arg = args.output
    prompt = args.prompt.strip() if args.prompt and args.prompt.strip() else None

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

    if args.model:
        model_name = args.model
    elif prompt:
        model_name = "feyninc/multimatte"
    else:
        model_name = "feyninc/FeyNobg"

    model = AutoModel.from_pretrained(model_name)
    if use_cuda:
        model = model.to(device)
    model.eval()

    t_load = time.perf_counter() - t_load_start

    # 3. Process image with FeyNoBg or MultiMatte
    t_infer_start = time.perf_counter()
    with torch.inference_mode():
        if prompt:
            cutout = model.process(image, prompt=prompt, return_type="cutout")
        else:
            cutout = model.process(image)
        if use_cuda:
            torch.cuda.synchronize()

    t_infer = time.perf_counter() - t_infer_start
    t_total = time.perf_counter() - t_start

    if isinstance(cutout, list):
        cutout = cutout[0]

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

    tag = f"multimatte('{prompt}')" if prompt else "nobg"
    sys.stderr.write(
        f"[{tag}] Success: {orig_w}x{orig_h} on {device.upper()} (Load: {t_load*1000:.0f}ms, Infer: {t_infer*1000:.0f}ms, Total: {t_total*1000:.0f}ms)\n"
    )


if __name__ == "__main__":
    main()
