#!/usr/bin/env python3
"""
Segment Anything (MobileSAM) Interactive Worker for little-screen-clip.
Provides point-prompted object segmentation via ONNX Runtime.

Usage:
    python3 segment_anything.py --server
    python3 segment_anything.py --image <image_path> --point <x>,<y> --output <mask_path>
"""

import os
import sys
import time
import json

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
DEFAULT_ENCODER_NAME = "mobile_sam_encoder.onnx"
DEFAULT_DECODER_NAME = "mobile_sam_decoder.onnx"

ENCODER_URL = "https://huggingface.co/Acly/MobileSAM/resolve/main/mobile_sam_image_encoder.onnx"
DECODER_URL = "https://huggingface.co/Acly/MobileSAM/resolve/main/sam_mask_decoder_single.onnx"


def ensure_model(model_name: str, url: str) -> str:
    os.makedirs(MODEL_CACHE_DIR, exist_ok=True)
    target_path = os.path.join(MODEL_CACHE_DIR, model_name)
    if os.path.exists(target_path) and os.path.getsize(target_path) > 100_000:
        return target_path

    sys.stderr.write(f"Downloading {model_name} to {target_path}...\n")
    sys.stderr.flush()

    tmp_path = target_path + ".tmp"
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "little-screen-clip/1.0"})
        with urllib.request.urlopen(req) as resp, open(tmp_path, "wb") as f_out:
            total = int(resp.headers.get("Content-Length", 0))
            downloaded = 0
            last_report = time.time()
            while True:
                chunk = resp.read(65536)
                if not chunk:
                    break
                f_out.write(chunk)
                downloaded += len(chunk)
                now = time.time()
                if total > 0 and now - last_report > 1.0:
                    pct = (downloaded / total) * 100.0
                    sys.stderr.write(f"Downloading {model_name}: {pct:.1f}% ({downloaded // (1024*1024)}MB / {total // (1024*1024)}MB)\n")
                    sys.stderr.flush()
                    last_report = now
        os.replace(tmp_path, target_path)
        sys.stderr.write(f"Downloaded {model_name} successfully.\n")
        sys.stderr.flush()
        return target_path
    except Exception as err:
        if os.path.exists(tmp_path):
            try:
                os.remove(tmp_path)
            except OSError:
                pass
        raise RuntimeError(f"Failed to download {model_name} from {url}: {err}")


class SamEngine:
    def __init__(self, encoder_path: str = None, decoder_path: str = None):
        enc_file = encoder_path if (encoder_path and os.path.exists(encoder_path)) else ensure_model(DEFAULT_ENCODER_NAME, ENCODER_URL)
        dec_file = decoder_path if (decoder_path and os.path.exists(decoder_path)) else ensure_model(DEFAULT_DECODER_NAME, DECODER_URL)

        # Prefer CUDA if available, fallback to CPU
        providers = ["CPUExecutionProvider"]
        if "CUDAExecutionProvider" in ort.get_available_providers():
            providers = ["CUDAExecutionProvider", "CPUExecutionProvider"]

        opts = ort.SessionOptions()
        opts.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
        self.encoder = ort.InferenceSession(enc_file, opts, providers=providers)
        self.decoder = ort.InferenceSession(dec_file, opts, providers=providers)

        self.embeddings = None
        self.orig_w = 0
        self.orig_h = 0
        self.new_w = 0
        self.new_h = 0

    def load_image(self, image_path: str):
        t0 = time.time()
        img = Image.open(image_path).convert("RGB")
        self.orig_w, self.orig_h = img.size

        # SAM expects longest side resized to 1024
        scale = 1024.0 / max(self.orig_w, self.orig_h)
        self.new_w = int(self.orig_w * scale + 0.5)
        self.new_h = int(self.orig_h * scale + 0.5)

        resized = img.resize((self.new_w, self.new_h), Image.Resampling.BILINEAR)
        inp_array = np.array(resized, dtype=np.float32)

        # Run encoder
        self.embeddings = self.encoder.run(None, {"input_image": inp_array})[0]
        elapsed_ms = (time.time() - t0) * 1000.0
        sys.stderr.write(f"Image loaded and encoded in {elapsed_ms:.1f}ms ({self.orig_w}x{self.orig_h})\n")
        sys.stderr.flush()

    def predict_point(self, x: float, y: float):
        if self.embeddings is None:
            raise RuntimeError("No image loaded into SAM engine.")

        # Clamp coordinate to image bounds
        x = max(0.0, min(float(self.orig_w - 1), float(x)))
        y = max(0.0, min(float(self.orig_h - 1), float(y)))

        # Scale coordinate to resized coordinate space
        scaled_x = x * (float(self.new_w) / float(self.orig_w))
        scaled_y = y * (float(self.new_h) / float(self.orig_h))

        coords = np.array([[[scaled_x, scaled_y]]], dtype=np.float32)
        labels = np.array([[1.0]], dtype=np.float32)  # 1 = foreground point
        mask_in = np.zeros((1, 1, 256, 256), dtype=np.float32)
        has_mask = np.array([0.0], dtype=np.float32)
        orig_sz = np.array([float(self.orig_h), float(self.orig_w)], dtype=np.float32)

        outputs = self.decoder.run(None, {
            "image_embeddings": self.embeddings,
            "point_coords": coords,
            "point_labels": labels,
            "mask_input": mask_in,
            "has_mask_input": has_mask,
            "orig_im_size": orig_sz,
        })

        mask_logits = outputs[0][0, 0]
        binary_mask = mask_logits > 0.0

        # Run-length encode binary mask for high-speed transmission to Rust
        flat = binary_mask.flatten()
        diff = np.diff(flat)
        idx = np.where(diff)[0] + 1
        runs = np.diff(np.concatenate(([0], idx, [len(flat)]))).tolist()
        if len(flat) > 0 and flat[0]:
            runs = [0] + runs

        return runs


def run_server():
    sys.stderr.write("Starting Segment Anything ONNX server...\n")
    sys.stderr.flush()
    try:
        engine = SamEngine()
    except Exception as err:
        sys.stderr.write(f"Failed to initialize SAM engine: {err}\n")
        sys.stderr.flush()
        # Output error json so client knows
        print(json.dumps({"status": "init_error", "error": str(err)}), flush=True)
        sys.exit(1)

    print(json.dumps({"status": "ready"}), flush=True)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as err:
            print(json.dumps({"status": "error", "error": f"Invalid JSON: {err}"}), flush=True)
            continue

        cmd = req.get("cmd")
        if cmd == "quit":
            break
        elif cmd == "ping":
            print(json.dumps({"status": "pong"}), flush=True)
        elif cmd == "load_image":
            path = req.get("path")
            if not path or not os.path.exists(path):
                print(json.dumps({"status": "error", "error": f"File not found: {path}"}), flush=True)
                continue
            try:
                engine.load_image(path)
                print(json.dumps({"status": "image_loaded", "width": engine.orig_w, "height": engine.orig_h}), flush=True)
            except Exception as err:
                print(json.dumps({"status": "error", "error": f"Failed to encode image: {err}"}), flush=True)
        elif cmd == "predict_point":
            x = req.get("x")
            y = req.get("y")
            if x is None or y is None:
                print(json.dumps({"status": "error", "error": "Missing x or y parameter"}), flush=True)
                continue
            try:
                rle = engine.predict_point(float(x), float(y))
                print(json.dumps({
                    "status": "mask",
                    "x": x,
                    "y": y,
                    "width": engine.orig_w,
                    "height": engine.orig_h,
                    "rle": rle
                }), flush=True)
            except Exception as err:
                print(json.dumps({"status": "error", "error": f"Prediction failed: {err}"}), flush=True)
        else:
            print(json.dumps({"status": "error", "error": f"Unknown command: {cmd}"}), flush=True)


def main():
    parser = argparse.ArgumentParser(description="Segment Anything (MobileSAM) Interactive Worker")
    parser.add_argument("--server", action="store_true", help="Run interactive JSON-line server on stdin/stdout")
    parser.add_argument("--image", default=None, help="Input image path")
    parser.add_argument("--point", default=None, help="Prompt point 'x,y' (pixel coordinates)")
    parser.add_argument("--output", default=None, help="Output mask PNG path")
    parser.add_argument("--encoder", default=None, help="Custom encoder ONNX path")
    parser.add_argument("--decoder", default=None, help="Custom decoder ONNX path")

    args = parser.parse_args()

    if args.server:
        run_server()
        return

    if not args.image or not args.point or not args.output:
        parser.print_help()
        sys.exit(1)

    coords = [float(p.strip()) for p in args.point.split(",")]
    engine = SamEngine(args.encoder, args.decoder)
    engine.load_image(args.image)
    rle = engine.predict_point(coords[0], coords[1])

    # Reconstruct binary mask for saving
    total_pixels = engine.orig_w * engine.orig_h
    recon = np.zeros(total_pixels, dtype=np.uint8)
    val = 0
    offset = 0
    for r in rle:
        if val == 1:
            recon[offset : offset + r] = 255
        val = 1 - val
        offset += r
    mask_2d = recon.reshape((engine.orig_h, engine.orig_w))
    Image.fromarray(mask_2d).save(args.output)
    print(f"Mask saved to {args.output}")


if __name__ == "__main__":
    main()
