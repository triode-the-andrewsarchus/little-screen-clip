#!/usr/bin/env python3
"""Review image, video and document metadata on the private native compositor."""
from pathlib import Path
import shutil
import subprocess
import re
import ui_acceptance_capture as private
from PIL import Image
from reportlab.pdfgen import canvas
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


def fixtures(root):
    Image.new("RGB", (400, 100), (80, 120, 160)).save(root / "wide.png")
    Image.new("RGB", (120, 300), (120, 80, 160)).save(root / "tall.png")
    exif = Image.Exif()
    exif[274] = 6
    Image.new("RGB", (60, 160), (160, 120, 80)).save(root / "turned.jpg", exif=exif)
    for name, size, duration, codec in [
        ("landscape.mp4", "320x180", 2, "mpeg4"),
        ("portrait.mkv", "120x240", 1, "ffv1"),
    ]:
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", f"color=red:size={size}:duration={duration}:rate=2",
            "-an", "-c:v", codec, "-pix_fmt", "yuv420p", str(root / name)], check=True, timeout=10)
    for name, title, author, count in [("short.pdf", "Rainbow", "Zelda", 1), ("long.pdf", "Atlas", "Ada", 3)]:
        document = canvas.Canvas(str(root / name))
        document.setTitle(title)
        document.setAuthor(author)
        for page in range(count):
            document.drawString(72, 720, f"{title} page {page + 1}")
            document.showPage()
        document.save()
    source = Path(__file__).resolve().parent.parent / "crates/little-clip-core/tests/fixtures/ebooks/journey.epub"
    shutil.copyfile(source, root / "journey.epub")


def ordered(session, shot, headers, names):
    # Active arrows and filename dots are unreliable in full-window sparse OCR.
    # Check owned filenames in their actual, observed fixed-output row positions.
    session.observe(shot, [headers[0], headers[-1]])
    normal = lambda text: re.sub(r"[^a-z0-9]", "", text.lower())
    with Image.open(session.output / f"{shot}.png") as screen:
        for index, name in enumerate(names):
            top = 372 + index * 108
            crop = session.output / f"{shot}-filename-{index}.png"
            region = screen.crop((346, top, 600, top + 25))
            region.resize((region.width * 4, region.height * 4)).save(crop)
            text = private.run_checked(["tesseract", str(crop), "stdout", "--psm", "7"], env=session.env)
            assert normal(name) in normal(text), (shot, index, name, text)


def exercise(session, root):
    session.observe("initial", ["List", "Photos"])
    session.click(118, 160)
    text = session.observe("list-start", ["Type", "Name", "Size"])
    if "Hands" in text:
        session.click(645, 160)
    submit(session, ':image;sort:type;scope:here')
    session.click(118, 227)  # Keep the test in List after existing automatic view selection.
    image_headers = ["Images", "Width", "Height", "Megapixels", "3/3"]
    session.observe("image-ready", [*image_headers, "400", "300", "160", "60"])
    positions = {field: session.word("image-ready", field, minimum_y=260) for field in ["Width", "Height", "Megapixels"]}
    session.click(positions["Width"][0] + 6, positions["Width"][1])
    ordered(session, "width-ascending", image_headers, ["tall.png", "turned.jpg", "wide.png"])
    session.click(positions["Width"][0] + 6, positions["Width"][1])
    ordered(session, "width-descending", image_headers, ["wide.png", "turned.jpg", "tall.png"])
    session.click(positions["Height"][0] + 6, positions["Height"][1])
    ordered(session, "height-ascending", image_headers, ["turned.jpg", "wide.png", "tall.png"])
    session.click(positions["Megapixels"][0] + 6, positions["Megapixels"][1])
    ordered(session, "pixels-ascending", image_headers, ["turned.jpg", "tall.png", "wide.png"])
    submit(session, ':video;sort:type;scope:here')
    session.click(118, 227)  # Keep the test in List after existing automatic view selection.
    video_headers = ["Video", "Duration", "Width", "Height", "Codec", "2/2"]
    session.observe("video-ready", video_headers)
    positions = {field: session.word("video-ready", field, minimum_y=260) for field in ["Duration", "Height", "Codec"]}
    session.click(positions["Duration"][0] + 6, positions["Duration"][1])
    ordered(session, "video-duration", video_headers, ["portrait.mkv", "landscape.mp4"])
    session.click(positions["Height"][0] + 6, positions["Height"][1])
    ordered(session, "video-height", video_headers, ["landscape.mp4", "portrait.mkv"])
    session.click(positions["Codec"][0] + 6, positions["Codec"][1])
    ordered(session, "video-codec", video_headers, ["portrait.mkv", "landscape.mp4"])
    submit(session, ':file;regex:".*[.](pdf|epub)$";sort:type;scope:here')
    session.click(118, 227)  # Keep the test in List after existing automatic view selection.
    document_headers = ["Documents", "Title", "Author", "Pages", "3/3"]
    session.observe("document-ready", [*document_headers, "Rainbow", "Atlas", "Example Author"])
    positions = {field: session.word("document-ready", field, minimum_y=260) for field in ["Title", "Author", "Pages"]}
    session.click(positions["Title"][0] + 6, positions["Title"][1])
    ordered(session, "document-title", document_headers, ["journey.epub", "long.pdf", "short.pdf"])
    session.click(positions["Author"][0] + 6, positions["Author"][1])
    ordered(session, "document-author", document_headers, ["long.pdf", "journey.epub", "short.pdf"])
    session.click(positions["Pages"][0] + 6, positions["Pages"][1])
    ordered(session, "pages-ascending", document_headers, ["short.pdf", "long.pdf", "journey.epub"])
    session.click(positions["Pages"][0] + 6, positions["Pages"][1])
    ordered(session, "pages-descending", document_headers, ["long.pdf", "short.pdf", "journey.epub"])
    x, y = positions["Pages"]
    private.run_checked([str(session.pointer), "right-click", "1280", "800", str(x + 5), str(y)], env=session.env)
    session.observe("family-reset-menu", ["Filename order"])
    session.click(*session.word("family-reset-menu", "Filename", minimum_y=300))
    ordered(session, "document-filename-order", document_headers, ["journey.epub", "long.pdf", "short.pdf"])
    submit(session, ':image;sort:type;scope:here')
    session.click(118, 227)  # Keep the test in List after existing automatic view selection.
    session.observe("image-sort-restored", image_headers)
    ordered(session, "other-family-sort-retained", image_headers, ["turned.jpg", "tall.png", "wide.png"])


if __name__ == "__main__":
    args = application_arguments(__doc__, Path("target/debug/clip-file-manager"))
    output, root = media_fixture_directory(args)
    fixtures(root)
    raise SystemExit(run_application_acceptance(args, output, root, exercise,
        "Family-specific image/video/document fields, oriented dimensions, typed sorts and unknown reflowed page counts"))
