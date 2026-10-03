#!/usr/bin/env python3
"""
Multi-Model AI Document OCR Worker for little-screen-clip.

Supports:
- GLM-OCR (default): zai-org/GLM-OCR (0.9B VLM)
- Tesseract (quick local baseline via /usr/bin/tesseract)

Usage:
    python3 ocr.py <input_path|-> <output_path|-> [--engine {glm-ocr, tesseract}]
"""

import argparse
import io
import os
import subprocess
import sys
import time
import types
import importlib.machinery

# Auto-reexec inside little-screen-clip venv if available
user_venv = os.path.expanduser("~/.local/share/little-screen-clip/nobg-venv/bin/python")
if os.path.exists(user_venv) and sys.executable != user_venv:
    os.execv(user_venv, [user_venv] + sys.argv)

from PIL import Image

def log_progress(msg: str):
    sys.stderr.write(f"[ocr:progress] {msg}\n")
    sys.stderr.flush()

def ensure_tessdata():
    tess_user_dir = os.path.expanduser("~/.local/share/tessdata")
    eng_path = os.path.join(tess_user_dir, "eng.traineddata")
    if os.path.exists(eng_path):
        os.environ["TESSDATA_PREFIX"] = tess_user_dir
    elif os.path.exists("/usr/share/tessdata"):
        os.environ["TESSDATA_PREFIX"] = "/usr/share/tessdata"

def run_tesseract(image: Image.Image) -> str:
    log_progress("Executing local Tesseract OCR engine...")
    ensure_tessdata()
    import tempfile

    # Preprocess image: upscale if low resolution so Tesseract text recognition is clean
    w, h = image.size
    if h < 400 or w < 600:
        scale = max(2, min(4, int(800 / max(h, 1))))
        proc_img = image.resize((w * scale, h * scale), Image.Resampling.LANCZOS)
    else:
        proc_img = image

    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as tmp_in:
        tmp_in_path = tmp_in.name
        proc_img.save(tmp_in_path, format="PNG")

    try:
        # Use English language pack only (do NOT use osd which corrupts characters)
        cmd = ["tesseract", tmp_in_path, "stdout", "-l", "eng", "--psm", "6"]
        res = subprocess.run(cmd, capture_output=True, text=True, env=os.environ)
        if res.returncode == 0 and res.stdout.strip():
            return res.stdout.strip()

        # Fallback to automatic PSM
        cmd2 = ["tesseract", tmp_in_path, "stdout", "-l", "eng"]
        res2 = subprocess.run(cmd2, capture_output=True, text=True, env=os.environ)
        if res2.returncode == 0:
            return res2.stdout.strip()

        raise RuntimeError(f"Tesseract failed: {res2.stderr or res.stderr}")
    finally:
        if os.path.exists(tmp_in_path):
            try:
                os.remove(tmp_in_path)
            except Exception:
                pass

def is_cached_model(model_id: str) -> bool:
    cache_dir = os.path.expanduser(f"~/.cache/huggingface/hub/models--{model_id.replace('/', '--')}")
    return os.path.isdir(cache_dir)

def run_glm_ocr(image: Image.Image, prompt: str | None, device: str) -> str:
    log_progress("Importing PyTorch and Transformers for GLM-OCR...")
    import torch
    from transformers import AutoModelForImageTextToText, AutoProcessor

    model_id = "zai-org/GLM-OCR"
    log_progress(f"Loading GLM-OCR weights ({model_id}) on {device.upper()}...")

    t0 = time.perf_counter()
    dtype = torch.bfloat16 if device == "cuda" and torch.cuda.is_bf16_supported() else torch.float16 if device == "cuda" else torch.float32
    cached = is_cached_model(model_id)

    processor = AutoProcessor.from_pretrained(model_id, local_files_only=cached, trust_remote_code=True)
    try:
        model = AutoModelForImageTextToText.from_pretrained(
            model_id,
            dtype=dtype,
            local_files_only=cached,
            trust_remote_code=True,
        ).to(device)
    except (torch.cuda.OutOfMemoryError, RuntimeError) as err:
        if "out of memory" in str(err).lower() and device == "cuda":
            log_progress("CUDA VRAM full for GLM-OCR, falling back to CPU execution...")
            torch.cuda.empty_cache()
            device = "cpu"
            dtype = torch.float32
            model = AutoModelForImageTextToText.from_pretrained(
                model_id,
                dtype=dtype,
                local_files_only=cached,
                trust_remote_code=True,
            ).to("cpu")
        else:
            raise
    model.eval()
    log_progress(f"GLM-OCR model loaded in {time.perf_counter() - t0:.1f}s. Running inference...")

    task_prompt = prompt if prompt else "Text Recognition:"
    messages = [
        {
            "role": "user",
            "content": [
                {"type": "image", "image": image},
                {"type": "text", "text": task_prompt},
            ],
        }
    ]

    text = processor.apply_chat_template(messages, add_generation_prompt=True, tokenize=False)
    inputs = processor(text=[text], images=[image], return_tensors="pt").to(device)

    with torch.inference_mode():
        outputs = model.generate(**inputs, max_new_tokens=4096)

    # Trim prompt tokens
    generated_ids = [
        output_ids[len(input_ids):]
        for input_ids, output_ids in zip(inputs.input_ids, outputs)
    ]
    response = processor.batch_decode(generated_ids, skip_special_tokens=True)[0]
    return response.strip()

def main():
    parser = argparse.ArgumentParser(description="Multi-Model Document OCR Worker")
    parser.add_argument("input", help="Path to input image, or '-' for stdin")
    parser.add_argument("output", help="Path to output text file, or '-' for stdout")
    parser.add_argument(
        "--engine",
        "-e",
        choices=["glm-ocr", "tesseract"],
        default="glm-ocr",
        help="OCR engine / model to use (default: glm-ocr)",
    )
    parser.add_argument("--prompt", "-p", default=None, help="Custom prompt or format schema")
    parser.add_argument("--device", "-d", choices=["auto", "cuda", "cpu"], default="auto", help="Compute device")

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

    # 2. Select device
    if args.engine == "tesseract":
        device = "cpu"
    elif args.device == "auto":
        import torch
        device = "cuda" if torch.cuda.is_available() else "cpu"
    else:
        device = args.device

    engine = args.engine.lower().replace("-", "").replace(".", "")
    log_progress(f"Starting OCR with engine '{args.engine}' on {orig_w}x{orig_h} image ({device.upper()})...")

    extracted_text = ""
    try:
        if "tesseract" in engine:
            extracted_text = run_tesseract(image)
        elif "glm" in engine:
            extracted_text = run_glm_ocr(image, args.prompt, device)
        else:
            sys.stderr.write(f"ERROR: Unknown engine '{args.engine}'\n")
            sys.exit(1)
    except Exception as e:
        import traceback
        tb_str = traceback.format_exc()
        sys.stderr.write(f"[ocr:error] Engine '{args.engine}' failed: {e}\n{tb_str}\n")
        sys.stderr.flush()
        sys.exit(1)

    t_total = time.perf_counter() - t_start

    # 3. Write output
    if args.output == "-":
        sys.stdout.write(extracted_text)
        sys.stdout.flush()
    else:
        out_dir = os.path.dirname(os.path.abspath(args.output))
        if out_dir:
            os.makedirs(out_dir, exist_ok=True)
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(extracted_text)

    char_count = len(extracted_text)
    line_count = len(extracted_text.splitlines())
    sys.stderr.write(
        f"[ocr:done] Engine: {args.engine} | {char_count} chars, {line_count} lines in {t_total:.2f}s on {device.upper()}\n"
    )

if __name__ == "__main__":
    main()
