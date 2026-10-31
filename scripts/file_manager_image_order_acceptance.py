#!/usr/bin/env python3
"""Image hash/reference ordering on the private Weston/Niri runner."""
from pathlib import Path
import json
import subprocess

from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


def exercise(session, root):
    session.observe("initial", ["Alpha.png", "Copy.png"])
    session.click(*session.word("initial", "Alpha.png", minimum_y=200))
    submit(session, ':image;sort:similarity;ref:selected')
    session.observe("distance-ready", ["Reference: Alpha.png", "4/4 images analyzed", "1 failed", "0 / 64", "Copy.png"])
    alpha = session.word("distance-ready", "Alpha.png", minimum_y=260)
    copy = session.word("distance-ready", "Copy.png", minimum_y=260)
    assert alpha[1] < copy[1], (alpha, copy)
    session.click(*copy)
    session.observe("selection-keeps-reference", ["Reference: Alpha.png", "Selection: 1 file"])
    session.click(820, 285)  # Distance header on the fixed private 1280×800 output.
    session.observe("reversed-distance", ["Opposite.png", "Alpha.png", "64 / 64"])
    opposite = session.word("reversed-distance", "Opposite.png", minimum_y=260)
    alpha = session.word("reversed-distance", "Alpha.png", minimum_y=260)
    assert opposite[1] < alpha[1], (opposite, alpha)
    Image.new("RGB", (80, 40), (80, 20, 180)).save(root / "Alpha.png")
    session.observe("reference-invalidated", ["reapply"], expected_any=["Reference changed", "Reference could not be verified"])
    submit(session, ':image;sort:dhash:desc')
    session.observe("raw-hash", ["4/4 images analyzed", "unsigned dHash-64 order", "Opposite.png"])
    def reader(request):
        result = subprocess.run([session.process.args[0], "--internal-file-search-reader"],
            input=json.dumps(request), text=True, capture_output=True, env=session.env, timeout=30, check=True)
        return json.loads(result.stdout)["Ok"]
    origin = reader({"Probe": str(root)})["Root"]
    entries = reader({"List": {"root": origin, "path": str(root), "hidden": False, "exclusions": []}})["Directory"]["entries"]
    entry = next(entry for entry in entries if entry["name"] == "Opposite.png")
    assert reader({"ImageHash": {"entry": entry, "cached": None}})["ImageHash"] == 2**64 - 1
    data = Path(session.env["XDG_DATA_HOME"]) / "little-screen-clip/search"
    assert not data.joinpath("index.sqlite").exists()
    assert not data.joinpath("models").exists()
    assert not data.joinpath("runtime").exists()
    session.key("Escape")
    session.observe("restored", ["Alpha.png", "Copy.png"])


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / "target/debug/clip-file-manager")
    output, root = media_fixture_directory(args)
    image = Image.new("RGB", (256, 64))
    image.putdata([(x, x, x) for _ in range(64) for x in range(256)])
    image.save(root / "Alpha.png")
    image.save(root / "Copy.png")
    image.transpose(Image.Transpose.FLIP_LEFT_RIGHT).save(root / "Opposite.png")
    (root / "Unreadable.png").write_text("not an image")
    return run_application_acceptance(args, output, root, exercise,
        "Image dHash sorting, reference Hamming distances, header reversal, retained reference and source invalidation without an embedding index")


if __name__ == "__main__":
    raise SystemExit(main())
