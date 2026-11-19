#!/usr/bin/env python3
"""
Downloads representative camera RAW samples from raw.pixls.us for testing
FlashRAW decoding and gallery thumbnail generation.

Usage:
    python3 scripts/download_raw_samples.py [--output <path>] [--force]

Default output directory:
    ~/Pictures/Raw
"""

import argparse
import os
import sys
import tempfile
import time
import urllib.parse
import urllib.request

MIN_SAMPLE_BYTES = 1024 * 1024
MAX_SAMPLE_BYTES = 512 * 1024 * 1024

RAW_SAMPLES = [
    (
        "Canon_EOS_5D_Mark_III.CR2",
        "https://raw.pixls.us/getfile.php/771/nice/Canon - EOS 5D Mark III.CR2",
    ),
    (
        "Canon_EOS_R5.CR3",
        "https://raw.pixls.us/getfile.php/4691/nice/Canon - EOS R5 - RAW, 1.6 crop, dual-pixel (3:2).CR3",
    ),
    (
        "Nikon_D750.NEF",
        "https://raw.pixls.us/getfile.php/895/nice/Nikon - D750 - 12bit 12bit compressed (Lossy (type 2)) (3:2).NEF",
    ),
    (
        "Sony_ILCE-7M3.ARW",
        "https://raw.pixls.us/getfile.php/2414/nice/Sony - ILCE-7M3 - 14bit 14bit compressed (3:2).ARW",
    ),
    (
        "Fujifilm_X-T3.RAF",
        "https://raw.pixls.us/getfile.php/2783/nice/Fujifilm - X-T3 - 14bit 14bit compressed (3:2).RAF",
    ),
    (
        "Panasonic_DC-GH5.RW2",
        "https://raw.pixls.us/getfile.php/1514/nice/Panasonic - DC-GH5 - 1:1.RW2",
    ),
    (
        "Olympus_E-M1MarkII.ORF",
        "https://raw.pixls.us/getfile.php/1993/nice/Olympus - E-M1MarkII - 16bit (4:3).ORF",
    ),
    (
        "Leica_M_Typ240.DNG",
        "https://raw.pixls.us/getfile.php/2317/nice/Leica - M (Typ 240) - 16bit 16bit compressed (3:2).DNG",
    ),
    (
        "Pentax_K-3.PEF",
        "https://raw.pixls.us/getfile.php/1075/nice/Pentax - K-3 - 14bit (3:2).PEF",
    ),
    (
        "Hasselblad_CFV.3FR",
        "https://raw.pixls.us/getfile.php/709/nice/Hasselblad - CFV.3FR",
    ),
    (
        "Nikon_Coolpix_P7800.NRW",
        "https://raw.pixls.us/getfile.php/1425/nice/Nikon - Coolpix P7800 - 12bit 12bit compressed ((7)) (4:3).NRW",
    ),
    (
        "Samsung_NX100.SRW",
        "https://raw.pixls.us/getfile.php/556/nice/Samsung - NX100 - 12bit (3:2).SRW",
    ),
]


def download_sample(raw_url: str, target_file: str) -> int:
    parts = urllib.parse.urlsplit(raw_url)
    encoded_path = urllib.parse.quote(parts.path)
    url = urllib.parse.urlunsplit((parts.scheme, parts.netloc, encoded_path, parts.query, parts.fragment))
    req = urllib.request.Request(
        url,
        headers={"User-Agent": "Mozilla/5.0 (compatible; LittleClipRawDownloader/1.0)"},
    )

    temporary = None
    try:
        with urllib.request.urlopen(req, timeout=30) as response:
            content_length = response.headers.get("Content-Length")
            declared = int(content_length) if content_length is not None else None
            if declared is not None and not MIN_SAMPLE_BYTES <= declared <= MAX_SAMPLE_BYTES:
                raise ValueError(f"unexpected response length: {declared}")
            with tempfile.NamedTemporaryFile(dir=os.path.dirname(target_file), delete=False) as staged:
                temporary = staged.name
                size = 0
                while chunk := response.read(1024 * 1024):
                    size += len(chunk)
                    if size > MAX_SAMPLE_BYTES:
                        raise ValueError("sample exceeds the download limit")
                    staged.write(chunk)
            if size < MIN_SAMPLE_BYTES or (declared is not None and size != declared):
                raise ValueError(f"incomplete sample: {size} bytes")
        os.replace(temporary, target_file)
        return size
    finally:
        if temporary is not None and os.path.exists(temporary):
            os.remove(temporary)


def download_raw_samples(output_dir: str, force: bool = False):
    output_path = os.path.expanduser(output_dir)
    os.makedirs(output_path, exist_ok=True)

    print(f"Target directory: {output_path}")
    print(f"Fetching {len(RAW_SAMPLES)} RAW sample files...")

    total_downloaded = 0
    total_existing = 0

    for idx, (filename, raw_url) in enumerate(RAW_SAMPLES, 1):
        target_file = os.path.join(output_path, filename)
        if not force and os.path.isfile(target_file):
            existing_size = os.path.getsize(target_file)
            if MIN_SAMPLE_BYTES <= existing_size <= MAX_SAMPLE_BYTES:
                file_size_mb = existing_size / (1024 * 1024)
                print(f"[{idx}/{len(RAW_SAMPLES)}] Already present: {filename} ({file_size_mb:.1f} MB)")
                total_existing += existing_size
                continue

        print(f"[{idx}/{len(RAW_SAMPLES)}] Downloading {filename}...")
        start_time = time.monotonic()
        try:
            size = download_sample(raw_url, target_file)
        except (OSError, ValueError) as err:
            print(f"    ERROR downloading {filename}: {err}", file=sys.stderr)
            sys.exit(1)
        elapsed = time.monotonic() - start_time
        mb = size / (1024 * 1024)
        print(f"    -> Done: {mb:.1f} MB in {elapsed:.1f}s ({mb / elapsed:.1f} MB/s)")
        total_downloaded += size

    all_mb = (total_downloaded + total_existing) / (1024 * 1024)
    print(f"{len(RAW_SAMPLES)} RAW sample files available in {output_path} ({all_mb:.1f} MB).")
    print("Pass that directory as RAW_SAMPLES_DIR to the ignored camera RAW corpus test.")


def main():
    parser = argparse.ArgumentParser(
        description="Download representative camera RAW sample files for tests."
    )
    default_dir = os.environ.get(
        "RAW_SAMPLES_DIR",
        os.path.join(os.path.expanduser("~"), "Pictures", "Raw"),
    )
    parser.add_argument(
        "--output",
        "-o",
        default=default_dir,
        help=f"Target output directory (default: {default_dir})",
    )
    parser.add_argument(
        "--force",
        "-f",
        action="store_true",
        help="Re-download files even if already present",
    )
    args = parser.parse_args()

    download_raw_samples(args.output, force=args.force)


if __name__ == "__main__":
    main()
