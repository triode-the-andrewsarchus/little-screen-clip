#!/usr/bin/env python3
"""
AI Image Inpainting Worker for little-screen-clip using LaMa (Large Mask Inpainting) ONNX.

Usage:
    python3 inpaint_lama.py <image_path> <mask_path> <output_path>
    python3 inpaint_lama.py --image <image_path> --mask <mask_path> --output <output_path>
"""

import os
import sys
import time

# Auto-reexec inside little-screen-clip venv if onnxruntime is not in current environment
try:
    import onnxruntime  # noqa: F401
    import numpy  # noqa: F401
    import PIL  # noqa: F401
except ImportError:
    user_venv = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
    if os.path.exists(user_venv) and sys.executable != user_venv:
        os.execv(user_venv, [user_venv] + sys.argv)
    else:
        sys.stderr.write(
            "ERROR: Required packages (onnxruntime, numpy, pillow) not found.\n"
            "Install them via:\n"
            "  uv pip install --python ~/.local/share/little-screen-clip/nobg-venv/bin/python onnxruntime numpy pillow\n"
        )
        sys.exit(1)

import argparse
import urllib.request
import numpy as np
from PIL import Image
import onnxruntime as ort

MODEL_CACHE_DIR = os.path.expanduser("~/.cache/little-screen-clip/models")
DEFAULT_MODEL_NAME = "big-lama.onnx"
DEFAULT_MODEL_URL = "https://huggingface.co/smile-forge/big-lama-onnx/resolve/main/inpainter/inpainter.onnx"
FALLBACK_MODEL_URL = "https://huggingface.co/Carve/LaMa-ONNX/resolve/main/lama_fp32.onnx"


def ensure_model_path(model_arg: str | None, model_url: str | None = None) -> str:
    if model_arg and os.path.exists(model_arg):
        return model_arg

    os.makedirs(MODEL_CACHE_DIR, exist_ok=True)
    model_path = os.path.join(MODEL_CACHE_DIR, DEFAULT_MODEL_NAME)
    if os.path.exists(model_path) and os.path.getsize(model_path) > 1_000_000:
        return model_path

    sys.stderr.write(f"Downloading LaMa model to {model_path}...\n")
    sys.stderr.flush()

    env_url = os.environ.get("LAMA_MODEL_URL") or os.environ.get("BIG_LAMA_MODEL_URL")
    target_urls = []
    if model_url and model_url.strip():
        target_urls.append(model_url.strip())
    if env_url and env_url.strip() and env_url.strip() not in target_urls:
        target_urls.append(env_url.strip())
    for default_url in [DEFAULT_MODEL_URL, FALLBACK_MODEL_URL]:
        if default_url not in target_urls:
            target_urls.append(default_url)

    for url in target_urls:
        try:
            req = urllib.request.Request(
                url,
                headers={"User-Agent": "little-screen-clip/1.0"},
            )
            with urllib.request.urlopen(req) as resp, open(model_path + ".tmp", "wb") as out_file:
                total_bytes = int(resp.headers.get("Content-Length", 0))
                downloaded = 0
                last_report = time.time()
                while True:
                    chunk = resp.read(65536)
                    if not chunk:
                        break
                    out_file.write(chunk)
                    downloaded += len(chunk)
                    now = time.time()
                    if total_bytes > 0 and now - last_report > 1.0:
                        pct = (downloaded / total_bytes) * 100.0
                        sys.stderr.write(f"Downloading: {pct:.1f}% ({downloaded // (1024*1024)}MB / {total_bytes // (1024*1024)}MB)\n")
                        sys.stderr.flush()
                        last_report = now

            os.replace(model_path + ".tmp", model_path)
            sys.stderr.write("Model download complete.\n")
            sys.stderr.flush()
            return model_path
        except Exception as err:
            sys.stderr.write(f"Warning: Failed downloading from {url}: {err}\n")
            sys.stderr.flush()
            if os.path.exists(model_path + ".tmp"):
                try:
                    os.remove(model_path + ".tmp")
                except OSError:
                    pass

    if os.path.exists(model_path) and os.path.getsize(model_path) > 1_000_000:
        return model_path

    raise RuntimeError("Failed to download LaMa inpainting model from all available sources.")


def inpaint(
    image_path: str,
    mask_path: str,
    output_path: str,
    model_path: str | None = None,
    model_url: str | None = None,
):
    t0 = time.time()
    model_file = ensure_model_path(model_path, model_url)

    # Available execution providers
    available_providers = ort.get_available_providers()
    providers = []
    if "CUDAExecutionProvider" in available_providers:
        providers.append("CUDAExecutionProvider")
    providers.append("CPUExecutionProvider")

    sys.stderr.write(f"Loading ONNX session ({providers[0]})...\n")
    sys.stderr.flush()

    sess_options = ort.SessionOptions()
    sess_options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    sess_options.intra_op_num_threads = min(8, max(1, os.cpu_count() or 4))
    session = ort.InferenceSession(model_file, sess_options=sess_options, providers=providers)

    # Load source image
    orig_pil = Image.open(image_path)
    orig_w, orig_h = orig_pil.size
    has_alpha = orig_pil.mode in ("RGBA", "LA") or ("transparency" in orig_pil.info)
    orig_rgba = orig_pil.convert("RGBA")
    orig_np = np.array(orig_rgba)

    rgb_np = orig_np[:, :, :3]
    alpha_np = orig_np[:, :, 3] if has_alpha else None

    # Load mask image (8-bit grayscale: >0 is inpaint, 0 is keep)
    mask_pil = Image.open(mask_path).convert("L")
    if mask_pil.size != (orig_w, orig_h):
        mask_pil = mask_pil.resize((orig_w, orig_h), Image.Resampling.NEAREST)
    mask_arr = np.array(mask_pil)

    # Normalize inputs for model:
    # Image: [1, 3, H, W] in [0.0, 1.0]
    img_tensor = (rgb_np.astype(np.float32) / 255.0).transpose((2, 0, 1))[np.newaxis, ...]

    # Mask: [1, 1, H, W] in {0.0, 1.0}
    mask_bool = (mask_arr > 0).astype(np.float32)
    mask_tensor = mask_bool[np.newaxis, np.newaxis, ...]

    # Handle padding to multiple of 8 (required by LaMa's Fourier convolutions)
    pad_h = (8 - orig_h % 8) % 8
    pad_w = (8 - orig_w % 8) % 8
    if pad_h > 0 or pad_w > 0:
        img_tensor = np.pad(img_tensor, ((0, 0), (0, 0), (0, pad_h), (0, pad_w)), mode="reflect")
        mask_tensor = np.pad(mask_tensor, ((0, 0), (0, 0), (0, pad_h), (0, pad_w)), mode="reflect")

    # Match input names
    inputs = session.get_inputs()
    feed_dict = {
        inputs[0].name: img_tensor,
        inputs[1].name: mask_tensor,
    }

    sys.stderr.write(f"Running LaMa inpainting on {orig_w}x{orig_h} image...\n")
    sys.stderr.flush()
    run_t0 = time.time()
    outputs = session.run(None, feed_dict)
    run_t1 = time.time()
    sys.stderr.write(f"Inference complete in {run_t1 - run_t0:.2f}s\n")
    sys.stderr.flush()

    out_tensor = outputs[0]
    # Crop away padding
    out_tensor = out_tensor[:, :, :orig_h, :orig_w]
    out_rgb = np.clip(out_tensor[0].transpose((1, 2, 0)) * 255.0, 0, 255).astype(np.uint8)

    # Composite: keep unmasked pixels exactly as original to avoid any loss/drift
    mask_3d = mask_arr[:, :, np.newaxis] > 0
    final_rgb = np.where(mask_3d, out_rgb, rgb_np)

    if has_alpha and alpha_np is not None:
        final_rgba = np.dstack([final_rgb, alpha_np])
        res_pil = Image.fromarray(final_rgba, mode="RGBA")
    else:
        res_pil = Image.fromarray(final_rgb, mode="RGB")

    os.makedirs(os.path.dirname(os.path.abspath(output_path)), exist_ok=True)
    res_pil.save(output_path, format="PNG")
    sys.stderr.write(f"Result saved to {output_path} (total time {time.time() - t0:.2f}s)\n")
    sys.stderr.flush()


def main():
    parser = argparse.ArgumentParser(description="LaMa Image Inpainting Tool")
    parser.add_argument("pos_image", nargs="?", default=None, help="Input image file path")
    parser.add_argument("pos_mask", nargs="?", default=None, help="Input mask file path")
    parser.add_argument("pos_output", nargs="?", default=None, help="Output image file path")

    parser.add_argument("--image", "-i", default=None, help="Input image file path")
    parser.add_argument("--mask", "-m", default=None, help="Input mask file path")
    parser.add_argument("--output", "-o", default=None, help="Output image file path")
    parser.add_argument("--model", default=None, help="Path to custom ONNX model")
    parser.add_argument(
        "--model-url", default=None, help="Custom download URL for the LaMa ONNX model"
    )

    args = parser.parse_args()

    image_path = args.image or args.pos_image
    mask_path = args.mask or args.pos_mask
    output_path = args.output or args.pos_output

    if not image_path or not mask_path or not output_path:
        parser.print_help(sys.stderr)
        sys.exit(1)

    if not os.path.exists(image_path):
        sys.stderr.write(f"ERROR: Image file not found: {image_path}\n")
        sys.exit(1)

    if not os.path.exists(mask_path):
        sys.stderr.write(f"ERROR: Mask file not found: {mask_path}\n")
        sys.exit(1)

    try:
        inpaint(image_path, mask_path, output_path, args.model, args.model_url)
    except Exception as err:
        sys.stderr.write(f"ERROR: Inpainting failed: {err}\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
