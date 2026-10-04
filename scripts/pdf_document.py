#!/usr/bin/env python3
"""Positioned PDF text, image extraction, and lossless searchable OCR layers.

Run with the application's nobg-venv interpreter. Requires PyMuPDF and Pillow;
OCR also requires Tesseract and the chosen language's traineddata.
"""
import argparse
import hashlib
import io
import json
import re
from pathlib import Path
import tempfile

try:
    import pymupdf as pdf
except ImportError as error:
    raise SystemExit(
        "PDF text selection needs PyMuPDF. Install with: uv pip install --python "
        "~/.local/share/little-screen-clip/nobg-venv/bin/python 'PyMuPDF>=1.26,<1.27'"
    ) from error
from PIL import Image, ImageDraw
from ocr import run_tesseract_pdf

OCR_SOURCE_HASH_KEY = "LittleClipOCRSourceSHA256"


def ocr_source_hash(document):
    kind, value = document.xref_get_key(-1, "Info/" + OCR_SOURCE_HASH_KEY)
    return value if kind == "string" and re.fullmatch(r"[0-9a-f]{64}", value) else None


def normalized_rect(page, bounds):
    rect = pdf.Rect(bounds) * page.rotation_matrix
    return [rect.x0 / page.rect.width, rect.y0 / page.rect.height,
            rect.x1 / page.rect.width, rect.y1 / page.rect.height]


def page_layout(page):
    """Keep character offsets in UTF-8 source bytes and boxes in visible page space."""
    pieces, glyphs, offset = [], [], 0
    raw = page.get_text("rawdict", flags=pdf.TEXTFLAGS_RAWDICT & ~pdf.TEXT_PRESERVE_IMAGES, sort=True)
    for block in raw["blocks"]:
        for line in block.get("lines", []):
            for span in line["spans"]:
                for char in span["chars"]:
                    text = char["c"]
                    end = offset + len(text.encode("utf-8"))
                    glyphs.append({"start": offset, "end": end,
                                   "rect": normalized_rect(page, char["bbox"])})
                    pieces.append(text)
                    offset = end
            pieces.append("\n")
            offset += 1
    images = [{"index": index, "rect": normalized_rect(page, image["bbox"])}
              for index, image in enumerate(page.get_image_info())]
    return {"text": "".join(pieces), "glyphs": glyphs, "images": images,
            "ocr_complete": ocr_source_hash(page.parent) is not None}


def image_png(document, page, index):
    """Extract one image object, preserving its alpha mask without page text."""
    images = page.get_image_info(xrefs=True)
    item = images[index]
    xref = item["xref"]
    if xref:
        data = document.extract_image(xref)
        image = Image.open(io.BytesIO(data["image"])).convert("RGBA")
        if data.get("smask"):
            mask = document.extract_image(data["smask"])["image"]
            image.putalpha(Image.open(io.BytesIO(mask)).convert("L"))
        output = io.BytesIO()
        image.save(output, format="PNG")
        return output.getvalue()
    blocks = page.get_text("dict", flags=pdf.TEXT_PRESERVE_IMAGES)["blocks"]
    block = next(block for block in blocks if block["number"] == item["number"])
    image = Image.open(io.BytesIO(block["image"])).convert("RGBA")
    if block.get("mask"):
        image.putalpha(Image.open(io.BytesIO(block["mask"])).convert("L"))
    output = io.BytesIO()
    image.save(output, format="PNG")
    return output.getvalue()


def add_ocr_layer(page, workspace, language):
    """OCR only pixels lacking native text; preserve the original image and objects."""
    rotation = page.rotation
    page.set_rotation(0)
    try:
        scale = min(300 / 72, 4096 / max(page.rect.width, page.rect.height))
        pixmap = page.get_pixmap(matrix=pdf.Matrix(scale, scale), alpha=False, colorspace=pdf.csRGB)
        image = Image.frombytes("RGB", [pixmap.width, pixmap.height], pixmap.samples)
        # Existing text (including a previous OCR layer) must not be duplicated.
        draw = ImageDraw.Draw(image)
        for word in page.get_text("words"):
            draw.rectangle(tuple(coordinate * scale for coordinate in word[:4]), fill="white")
        base = str(workspace / "layer")
        run_tesseract_pdf(image, base, language)
        with pdf.open(base + ".pdf") as layer:
            if layer[0].get_text().strip():
                page.show_pdf_page(page.rect, layer, 0, overlay=False)
    finally:
        page.set_rotation(rotation)


def source_sha256(source):
    with open(source, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def searchable_pdf(source, output, language):
    import sys
    original_hash = source_sha256(source)
    with pdf.open(source) as document, tempfile.TemporaryDirectory(prefix="pdf-ocr-", dir=Path(output).parent) as directory:
        for index, page in enumerate(document):
            print(f"[ocr:progress] Adding text layer: page {index + 1} of {len(document)}", file=sys.stderr, flush=True)
            add_ocr_layer(page, Path(directory), language)
        kind, info = document.xref_get_key(-1, "Info")
        if kind == "xref":
            info_xref = int(info.split()[0])
        else:
            info_xref = document.get_new_xref()
            document.update_object(info_xref, info if kind == "dict" else "<< >>")
            document.xref_set_key(-1, "Info", f"{info_xref} 0 R")
        document.xref_set_key(info_xref, OCR_SOURCE_HASH_KEY, pdf.get_pdf_str(original_hash))
        document.save(output, garbage=3, deflate=True)


def matching_ocr_copy(source, candidate):
    """Reuse only OCR copies stamped with the SHA-256 of the current source bytes."""
    try:
        with pdf.open(candidate) as searchable:
            stored_hash = ocr_source_hash(searchable)
            return stored_hash is not None and stored_hash == source_sha256(source)
    except (RuntimeError, ValueError, OSError):
        return False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["layout", "image", "ocr", "compare"])
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--page", type=int, default=1)
    parser.add_argument("--image", type=int, default=0)
    parser.add_argument("--language", default="eng")
    parser.add_argument("--candidate", type=Path)
    args = parser.parse_args()
    if args.operation == "compare":
        if args.candidate is None:
            parser.error("compare requires --candidate")
        args.output.write_text(json.dumps(matching_ocr_copy(args.source, args.candidate)))
    elif args.operation == "ocr":
        searchable_pdf(args.source, args.output, args.language)
    else:
        with pdf.open(args.source) as document:
            page = document[args.page - 1]
            if args.operation == "layout":
                args.output.write_text(json.dumps(page_layout(page), ensure_ascii=False), encoding="utf-8")
            else:
                args.output.write_bytes(image_png(document, page, args.image))


if __name__ == "__main__":
    main()
