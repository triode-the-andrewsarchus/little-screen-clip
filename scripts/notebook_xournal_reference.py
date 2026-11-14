#!/usr/bin/env python3
"""Export independent Xournal reference images with an installed upstream CLI.

This developer-only tool is never called by the application. Source fixtures and
licenses remain under docs/examples/notebooks/xournal; no editor code is imported.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

CASES = [
    ("legacy-stroke.xoj", 1, "legacy-stroke"),
    ("load/strokes.xopp", 1, "strokes"),
    ("load/layers.xopp", 1, "layers"),
    ("load/text-fileversion-5.xopp", 1, "transformed-text"),
    ("load/image-fileversion-5.xopp", 1, "transformed-images"),
    ("load/pages.xopp", 3, "lined-page"),
    ("load/pages.xopp", 11, "image-background"),
    ("load/relativePaths.xopp", 1, "external-pdf"),
    ("load/pages.xopp", 5, "staves-page"),
    ("load/pages.xopp", 7, "dotted-page"),
    ("load/pages.xopp", 8, "isodotted-page"),
    ("load/pages.xopp", 9, "isograph-page"),
    ("packaged/image.xopp", 1, "packaged-image"),
    ("load/latex-fileversion-5.xopp", 1, "embedded-latex"),
    ("packaged/stroke.xopp", 1, "packaged-stroke"),
    ("packaged/pdf-background.xopp", 2, "packaged-pdf-page-2"),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).resolve().parent.parent / "docs/examples/notebooks/xournal"
    results = []
    with tempfile.TemporaryDirectory(prefix="xournal-reference-home-") as home:
        env = os.environ | {"HOME": home, "XDG_CONFIG_HOME": home + "/config",
                            "XDG_DATA_HOME": home + "/data", "XDG_CACHE_HOME": home + "/cache"}
        # Generic document fonts are resolved identically on any developer host.
        # This changes font selection only; the upstream renderer remains untouched.
        fonts = Path(__file__).resolve().parent.parent / "crates/little-clip-render/assets/fonts"
        config = Path(home) / "fonts.conf"
        aliases = {"Sans": "Arimo", "sans-serif": "Arimo", "Liberation Sans": "Arimo",
                   "Serif": "Tinos", "serif": "Tinos", "Times New Roman": "Tinos",
                   "Monospace": "Cousine", "monospace": "Cousine", "Courier New": "Cousine"}
        from xml.sax.saxutils import escape
        rules = "".join(f'<alias binding="strong"><family>{escape(name)}</family><prefer><family>{family}</family></prefer></alias>' for name, family in aliases.items())
        config.write_text(f'<?xml version="1.0"?><!DOCTYPE fontconfig SYSTEM "fonts.dtd"><fontconfig><dir>{escape(str(fonts))}</dir><cachedir>{escape(home)}/font-cache</cachedir>{rules}</fontconfig>')
        env["FONTCONFIG_FILE"] = str(config)
        env.pop("DISPLAY", None)
        env.pop("WAYLAND_DISPLAY", None)
        for name, page, label in CASES:
            destination = output / f"{label}-reference.png"
            process = subprocess.run([
                str(args.binary.resolve()), "--create-img", str(destination),
                "--export-range", str(page), "--export-png-height", "800", str(source / name),
            ], env=env, capture_output=True, timeout=30)
            diagnostic = process.stderr.decode(errors="replace")[-4096:]
            if process.returncode or not destination.is_file():
                raise RuntimeError(f"Reference export failed for {name}, page {page}: {diagnostic}")
            results.append(dict(source=name, page=page, image=destination.name,
                                font_policy="bundled Arimo/Tinos/Cousine and CJK fallback",
                                sha256=hashlib.sha256(destination.read_bytes()).hexdigest(),
                                diagnostic=diagnostic))
    (output / "reference-exports.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"Exported {len(results)} independent Xournal reference images to {output}")


if __name__ == "__main__":
    main()
