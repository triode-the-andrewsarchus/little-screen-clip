#!/usr/bin/env python3
"""Regenerate independent Supernote reference pages from retained upstream source.

Development only: requires supernotelib's Python dependencies. Applications do
not invoke this script or require those packages for notebook preview rendering.
"""
import argparse
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "vendor/supernote-tool/upstream"))

CASES = {
    "demo-a5x-20230015-1to10": (1, 10),
    "blank-a6x-3.26.40-two-pages": (2,),
    "stroke-n5-20260016-isolation-tools-colors-widths": (1, 5),
    "layout-n5-20230015-horizontal-1090": (1,),
    "render-n6-20230015-moonchild-user-bg": (1,),
    "test-a5x-20220011-old-pen-ids": (1,),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    import supernotelib
    from supernotelib.converter import ImageConverter
    from supernotelib.fileformat import KEY_LAYERS
    for stem, pages in CASES.items():
        source_path = ROOT / "docs/examples/notebooks/supernote" / f"{stem}.note"
        with source_path.open("rb") as source:
            metadata = supernotelib.parse_metadata(source)
            if stem == "render-n6-20230015-moonchild-user-bg":
                # This fixture omits protocol tags and unused layer records.
                # Match the TypeScript parser's defaults before calling the
                # unchanged reference codec/compositor; disclose the adapter.
                for page in metadata.pages:
                    layers = page.get(KEY_LAYERS)
                    if layers is not None:
                        for layer in layers:
                            if layer.get("LAYERBITMAP"):
                                layer.setdefault("LAYERPROTOCOL", "RATTA_RLE")
                        while len(layers) < 5:
                            layers.append({})
            notebook = supernotelib.load(source, metadata)
            converter = ImageConverter(notebook)
            for page in pages:
                output = args.output_dir / f"{stem}-page-{page}-reference.png"
                converter.convert(page - 1).convert("RGB").save(output)
                print(output)


if __name__ == "__main__":
    main()
