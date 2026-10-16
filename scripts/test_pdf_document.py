"""Integration checks for native objects and independently searchable PDF OCR."""
import hashlib
import io
import os
import shutil
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import pymupdf as pdf
from PIL import Image, ImageDraw, ImageFont
from pdf_document import image_png, page_layout, searchable_pdf, matching_ocr_copy, OCR_SOURCE_HASH_KEY


class PdfDocumentTests(unittest.TestCase):
    def test_ocr_passes_an_inherited_staging_directory_to_tesseract(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source.pdf"
            with pdf.open() as document:
                page = document.new_page(width=400, height=200)
                page.insert_text((30, 60), "Native words remain", fontsize=24)
                document.save(source)
            descriptor = os.open(root, os.O_PATH | os.O_DIRECTORY)
            source_descriptor = os.open(source, os.O_RDONLY)
            try:
                output = Path(f"/proc/self/fd/{descriptor}/searchable.pdf")
                result = subprocess.run(
                    [sys.executable, str(Path(__file__).with_name("pdf_document.py")),
                     "ocr", f"/proc/self/fd/{source_descriptor}", str(output), "--language", "eng"],
                    pass_fds=(descriptor, source_descriptor), capture_output=True, text=True, timeout=30,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
            finally:
                os.close(source_descriptor)
                os.close(descriptor)
            with pdf.open(root / "searchable.pdf") as result:
                self.assertIn("Native words remain", result[0].get_text())
                self.assertTrue(page_layout(result[0])["ocr_complete"])
            self.assertEqual(sorted(path.name for path in root.iterdir()), ["searchable.pdf", "source.pdf"])

    def test_layout_preserves_unicode_byte_offsets_and_rotated_crop_coordinates(self):
        with pdf.open() as document:
            page = document.new_page(width=400, height=300)
            page.insert_text((40, 60), "Hello 日本語", fontname="china-s", fontsize=22)
            page.set_cropbox(pdf.Rect(10, 10, 390, 290))
            for rotation in (0, 90, 180, 270):
                page.set_rotation(rotation)
                layout = page_layout(page)
                encoded = layout["text"].encode("utf-8")
                self.assertIn("日本語", layout["text"])
                self.assertEqual("".join(encoded[glyph["start"]:glyph["end"]].decode("utf-8")
                                         for glyph in layout["glyphs"]), "Hello 日本語")
                raster = page.get_pixmap(alpha=False)
                for glyph in layout["glyphs"]:
                    x0, y0, x1, y1 = glyph["rect"]
                    self.assertTrue(0 <= x0 < x1 <= 1 and 0 <= y0 < y1 <= 1)
                first = layout["glyphs"][0]["rect"]
                pixels = Image.frombytes("RGB", [raster.width, raster.height], raster.samples)
                region = pixels.crop(tuple(int(v * (raster.width if i % 2 == 0 else raster.height))
                                           for i, v in enumerate(first)))
                self.assertLess(min(sum(pixel) for pixel in region.getdata()), 600)

    def test_copy_extracts_only_the_chosen_image_with_alpha_and_occurrence_bounds(self):
        image = Image.new("RGBA", (80, 60), (200, 30, 40, 128))
        buffer = io.BytesIO()
        image.save(buffer, format="PNG")
        with pdf.open() as document:
            page = document.new_page(width=400, height=300)
            page.insert_image(pdf.Rect(20, 20, 100, 80), stream=buffer.getvalue())
            page.insert_image(pdf.Rect(200, 160, 280, 220), stream=buffer.getvalue())
            page.insert_text((25, 50), "Must not be copied")
            layout = page_layout(page)
            self.assertEqual(len(layout["images"]), 2)
            self.assertNotEqual(layout["images"][0]["rect"], layout["images"][1]["rect"])
            for index in (0, 1):
                copied = Image.open(io.BytesIO(image_png(document, page, index))).convert("RGBA")
                self.assertEqual(copied.size, image.size)
                self.assertEqual(copied.getpixel((0, 0))[3], 128)
                self.assertTrue(all(abs(actual - expected) <= 1
                                    for actual, expected in zip(copied.tobytes(), image.tobytes())))

    def test_ocr_adds_real_text_preserves_pixels_bookmarks_rotation_and_native_text(self):
        font = ImageFont.truetype("/usr/share/fonts/TTF/DejaVuSans.ttf", 44)
        scan = Image.new("RGB", (1000, 500), "white")
        draw = ImageDraw.Draw(scan)
        draw.text((60, 140), "Amber lighthouse", fill="black", font=font)
        buffer = io.BytesIO()
        scan.save(buffer, format="PNG")
        with tempfile.TemporaryDirectory() as temporary:
            source, output, repeated = [Path(temporary) / name for name in ("source.pdf", "ocr.pdf", "again.pdf")]
            with pdf.open() as document:
                page = document.new_page(width=500, height=250)
                page.insert_image(page.rect, stream=buffer.getvalue())
                page.insert_text((25, 25), "Native caption", fontsize=14)
                page.set_rotation(90)
                document.new_page(width=300, height=400)  # Blank pages must not stop OCR.
                document.set_toc([[1, "Chapter One", 1]])
                document.set_metadata({"title": "Preserve this title"})
                _, info = document.xref_get_key(-1, "Info")
                document.xref_set_key(int(info.split()[0]), "ExistingCustomField", pdf.get_pdf_str("Keep this value"))
                document.save(source)
            searchable_pdf(source, output, "eng")
            self.assertTrue(matching_ocr_copy(source, output))
            with pdf.open(source) as original, pdf.open(output) as searchable:
                self.assertFalse(page_layout(original[0])["ocr_complete"])
                self.assertTrue(page_layout(searchable[0])["ocr_complete"])
                self.assertEqual(searchable.xref_get_key(-1, "Info/" + OCR_SOURCE_HASH_KEY),
                                 ("string", hashlib.sha256(source.read_bytes()).hexdigest()))
                self.assertEqual(len(searchable), 2)
                self.assertEqual(searchable.get_toc(), original.get_toc())
                self.assertEqual(searchable.metadata["title"], original.metadata["title"])
                self.assertEqual(searchable.xref_get_key(-1, "Info/ExistingCustomField"), ("string", "Keep this value"))
                self.assertEqual(searchable[0].rotation, 90)
                text = searchable[0].get_text()
                self.assertIn("Amber lighthouse", text)
                self.assertEqual(text.count("Native caption"), 1)
                self.assertTrue(searchable[0].search_for("lighthouse"))
                self.assertEqual(original[0].get_pixmap().samples, searchable[0].get_pixmap().samples)
                self.assertEqual(Image.open(io.BytesIO(image_png(searchable, searchable[0], 0))).convert("RGB").tobytes(), scan.tobytes())
            searchable_pdf(output, repeated, "eng")
            self.assertTrue(matching_ocr_copy(output, repeated))
            self.assertFalse(matching_ocr_copy(source, repeated))
            with pdf.open(repeated) as searchable:
                self.assertEqual(searchable[0].get_text().count("Amber lighthouse"), 1)

    def test_hash_reuse_handles_sources_without_metadata_and_checks_exact_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, candidate, legacy = [Path(temporary) / name for name in ("source.pdf", "ocr.pdf", "legacy.pdf")]
            with pdf.open() as document:
                document.new_page(width=300, height=400)
                document.save(source)
            shutil.copyfile(source, legacy)
            self.assertFalse(matching_ocr_copy(source, legacy))
            searchable_pdf(source, candidate, "eng")
            with pdf.open(candidate) as document:
                self.assertEqual(document.xref_get_key(-1, "Info/" + OCR_SOURCE_HASH_KEY),
                                 ("string", hashlib.sha256(source.read_bytes()).hexdigest()))
            with patch.object(pdf.Page, "get_pixmap", side_effect=AssertionError("reuse must not render pages")):
                self.assertTrue(matching_ocr_copy(source, candidate))
            renamed = Path(temporary) / "renamed.pdf"
            shutil.copyfile(source, renamed)
            self.assertTrue(matching_ocr_copy(renamed, candidate))
            source.write_bytes(source.read_bytes() + b"\n% source bytes changed\n")
            self.assertFalse(matching_ocr_copy(source, candidate))
            self.assertFalse(matching_ocr_copy(source, Path(temporary) / "missing.pdf"))
            legacy.write_bytes(b"not a PDF")
            self.assertFalse(matching_ocr_copy(source, legacy))

    def test_reuse_rejects_invalid_hash_metadata_and_missing_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            source, candidate = [Path(temporary) / name for name in ("source.pdf", "ocr.pdf")]
            with pdf.open() as document:
                document.new_page(width=300, height=400)
                document.set_metadata({"title": "Preserve metadata"})
                document.save(source)
            for value in ("null", "123", pdf.get_pdf_str("invalid hash")):
                with pdf.open(source) as document:
                    _, info = document.xref_get_key(-1, "Info")
                    document.xref_set_key(int(info.split()[0]), OCR_SOURCE_HASH_KEY, value)
                    document.save(candidate)
                self.assertFalse(matching_ocr_copy(source, candidate))
                with pdf.open(candidate) as document:
                    self.assertFalse(page_layout(document[0])["ocr_complete"])
            source.unlink()
            self.assertFalse(matching_ocr_copy(source, candidate))


if __name__ == "__main__":
    unittest.main()
