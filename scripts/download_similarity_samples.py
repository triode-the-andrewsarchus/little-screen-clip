#!/usr/bin/env python3
"""
Downloads a diverse 1,000-image ImageNet sample dataset for photo similarity
and multimodal clustering tests.

Usage:
    python3 scripts/download_similarity_samples.py [--output <path>]

Default output directory:
    ~/Pictures/similarity-samples
"""

import argparse
import os
import shutil
import sys
import tempfile
import urllib.request
import zipfile

SAMPLE_DATASET_URL = (
    "https://codeload.github.com/EliSchwartz/imagenet-sample-images/zip/refs/heads/master"
)


def download_similarity_samples(output_dir: str):
    output_path = os.path.expanduser(output_dir)
    os.makedirs(output_path, exist_ok=True)

    print(f"Downloading ImageNet sample images to temporary archive...")
    with tempfile.NamedTemporaryFile(suffix=".zip", delete=False) as tmp_file:
        tmp_path = tmp_file.name

    try:
        urllib.request.urlretrieve(SAMPLE_DATASET_URL, tmp_path)
        print(f"Extracting images into {output_path}...")

        with zipfile.ZipFile(tmp_path, "r") as zf:
            extracted = 0
            for member in zf.infolist():
                if member.filename.lower().endswith(".jpeg") or member.filename.lower().endswith(".jpg"):
                    filename = os.path.basename(member.filename)
                    if filename:
                        target = os.path.join(output_path, filename)
                        with zf.open(member) as source, open(target, "wb") as dest:
                            shutil.copyfileobj(source, dest)
                        extracted += 1

        print(f"Successfully extracted {extracted} sample images into {output_path}.")
    finally:
        if os.path.exists(tmp_path):
            os.remove(tmp_path)


def main():
    parser = argparse.ArgumentParser(
        description="Download sample image dataset for similarity clustering."
    )
    default_dir = os.environ.get(
        "SIMILARITY_SAMPLES_DIR",
        os.path.join(os.path.expanduser("~"), "Pictures", "similarity-samples"),
    )
    parser.add_argument(
        "--output",
        "-o",
        default=default_dir,
        help=f"Target output directory (default: {default_dir})",
    )
    args = parser.parse_args()

    download_similarity_samples(args.output)


if __name__ == "__main__":
    main()
