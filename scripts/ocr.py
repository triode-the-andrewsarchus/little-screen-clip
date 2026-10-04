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

def ensure_tessdata(language="eng"):
    candidates = [os.environ.get("TESSDATA_PREFIX", ""),
                  os.path.expanduser("~/.local/share/tessdata"),
                  "/usr/share/tessdata", "/usr/share/tesseract-ocr/5/tessdata"]
    for directory in candidates:
        if directory and all(os.path.isfile(os.path.join(directory, code + ".traineddata"))
                             for code in language.split("+")):
            os.environ["TESSDATA_PREFIX"] = directory
            return


def tesseract_output(image: Image.Image, output: str, arguments: list[str], language: str = "eng", temporary_directory=None):
    """Shared local OCR boundary for text and positioned PDF output."""
    import tempfile
    ensure_tessdata(language)
    w, h = image.size
    if h < 400 or w < 600:
        scale = max(2, min(4, int(800 / max(h, 1))))
        image = image.resize((w * scale, h * scale), Image.Resampling.LANCZOS)
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False, dir=temporary_directory) as source:
        image.save(source.name, format="PNG")
    try:
        return subprocess.run(
            ["tesseract", source.name, output, "-l", language, *arguments],
            capture_output=True, text=True, env=os.environ, timeout=300,
        )
    finally:
        os.unlink(source.name)


def run_tesseract(image: Image.Image) -> str:
    log_progress("Executing local Tesseract OCR engine...")
    result = tesseract_output(image, "stdout", ["--psm", "6"])
    if result.returncode == 0 and result.stdout.strip():
        return result.stdout.strip()
    fallback = tesseract_output(image, "stdout", [])
    if fallback.returncode:
        raise RuntimeError(f"Tesseract failed: {fallback.stderr or result.stderr}")
    return fallback.stdout.strip()


def run_tesseract_pdf(image: Image.Image, output_base: str, language: str = "eng"):
    """Write only the invisible, positioned OCR layer, without duplicating pixels."""
    result = tesseract_output(
        image, output_base, ["--dpi", "300", "-c", "textonly_pdf=1", "-c", "tessedit_create_pdf=1"], language,
        os.path.dirname(output_base),
    )
    if result.returncode:
        raise RuntimeError(f"Tesseract PDF OCR failed: {result.stderr}")


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
