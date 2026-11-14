#!/usr/bin/env python3
"""Build explicitly derived reMarkable archive cases from retained licensed ink."""
from copy import deepcopy
from io import BytesIO
import json
from pathlib import Path
import tarfile
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED

from reportlab.pdfgen import canvas
from PIL import Image

ROOT = Path(__file__).resolve().parent.parent / "docs/examples/notebooks/remarkable"


def tar_bytes(files):
    output = BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name, data in files.items():
            header = tarfile.TarInfo(name)
            header.size = len(data)
            header.mode = 0o644
            archive.addfile(header, BytesIO(data))
    return output.getvalue()


def json_bytes(value):
    return (json.dumps(value, indent=2) + "\n").encode()


def main():
    output = ROOT / "generated"
    output.mkdir(exist_ok=True)
    with ZipFile(ROOT / "archives/Test.rmdoc") as source:
        original = {name: source.read(name) for name in source.namelist()}
    root = next(name.removesuffix(".metadata") for name in original if name.endswith(".metadata"))
    content = json.loads(original[root + ".content"])
    svg = b'''<svg xmlns="http://www.w3.org/2000/svg" width="1404" height="1872">
<rect width="1404" height="1872" fill="#fff8e0"/>
<path d="M0 468H1404 M0 936H1404 M0 1404H1404 M468 0V1872 M936 0V1872"
 stroke="#228855" stroke-width="8" fill="none"/>
</svg>'''
    template = tar_bytes({"template.json": json_bytes({"name": "Review Grid", "fileName": "review-grid", "categories": ["Grids"], "landscape": False}), "template.svg": svg})
    files = deepcopy(original)
    for page in content["cPages"]["pages"]:
        page["template"]["value"] = "review-grid"
    files[root + ".content"] = json_bytes(content)
    # Filename differs from the template's fileName: resolution must use JSON.
    files["Review Grid.rmt"] = template
    (output / "layered-template.rmn").write_bytes(tar_bytes(files))

    raster = BytesIO()
    Image.new("RGB", (1404, 1872), (232, 240, 255)).save(raster, format="PNG")
    files = deepcopy(original)
    raster_content = json.loads(original[root + ".content"])
    for page in raster_content["cPages"]["pages"]:
        page["template"]["value"] = "review-raster"
    files[root + ".content"] = json_bytes(raster_content)
    files["Raster.rmt"] = tar_bytes({
        "template.json": json_bytes({"name": "Review Raster", "fileName": "review-raster"}),
        "template.png": raster.getvalue(),
    })
    (output / "raster-template.rmn").write_bytes(tar_bytes(files))

    pdf_buffer = BytesIO()
    pdf = canvas.Canvas(pdf_buffer, pagesize=(468, 624), invariant=1)
    for color, label in [((1, .8, .8), "SOURCE PAGE ONE"), ((.8, .9, 1), "SOURCE PAGE TWO")]:
        pdf.setFillColorRGB(*color)
        pdf.rect(0, 0, 468, 624, stroke=0, fill=1)
        pdf.setFillColorRGB(0, 0, 0)
        pdf.drawString(24, 600, label)
        pdf.showPage()
    pdf.save()
    files = deepcopy(original)
    content = json.loads(original[root + ".content"])
    content["fileType"] = "pdf"
    for page, redirection in zip(content["cPages"]["pages"], [1, 0]):
        page["redir"] = {"value": redirection}
        page["template"] = {"value": "Blank"}
    content["cPages"]["pages"].append({"id": "inserted", "redir": {"value": -1}})
    files[root + ".content"] = json_bytes(content)
    files[root + ".pdf"] = pdf_buffer.getvalue()
    with ZipFile(output / "redirected-pdf.rmdoc", "w") as archive:
        for name, data in files.items():
            entry = ZipInfo(name)
            entry.compress_type = ZIP_DEFLATED
            archive.writestr(entry, data)

    legacy = {"legacy.metadata": json_bytes({"type": "DocumentType", "visibleName": "Legacy paper"}),
              "legacy.content": json_bytes({"fileType": "notebook", "pages": ["1", "0"], "pageCount": 2}),
              "legacy/0.rm": (ROOT / "legacy-real/test_v5_math.rm").read_bytes(),
              "legacy/1.rm": (ROOT / "legacy-real/test_v5_advent_of_code.rm").read_bytes(),
              "legacy.pagedata": b"review-grid\nBlank\n", "Review Grid.rmt": template}
    (output / "legacy-paper.rmn").write_bytes(tar_bytes(legacy))


if __name__ == "__main__":
    main()
