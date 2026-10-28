#!/usr/bin/env python3
"""
Downloads the CC0 camera RAW test corpus from raw.pixls.us for testing
flashraw decoders, metadata inspection, and gallery thumbnail generation.

Usage:
    python3 scripts/download_raw_samples.py [--output <path>] [--force]

Default output directory:
    ~/Pictures/Raw
"""

import argparse
import os
import sys
import time
import urllib.parse
import urllib.request

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


def download_raw_samples(output_dir: str, force: bool = False):
    output_path = os.path.expanduser(output_dir)
    os.makedirs(output_path, exist_ok=True)

    print(f"Target directory: {output_path}")
    print(f"Fetching {len(RAW_SAMPLES)} RAW sample files...")

    total_downloaded = 0
    total_existing = 0

    for idx, (filename, raw_url) in enumerate(RAW_SAMPLES, 1):
        target_file = os.path.join(output_path, filename)
        if not force and os.path.exists(target_file) and os.path.getsize(target_file) > 1024 * 1024:
            file_size_mb = os.path.getsize(target_file) / (1024 * 1024)
            print(f"[{idx}/{len(RAW_SAMPLES)}] Already present: {filename} ({file_size_mb:.1f} MB)")
            total_existing += os.path.getsize(target_file)
            continue

        print(f"[{idx}/{len(RAW_SAMPLES)}] Downloading {filename}...")
        parts = urllib.parse.urlsplit(raw_url)
        encoded_path = urllib.parse.quote(parts.path)
        url = urllib.parse.urlunsplit((parts.scheme, parts.netloc, encoded_path, parts.query, parts.fragment))

        req = urllib.request.Request(
            url,
            headers={"User-Agent": "Mozilla/5.0 (compatible; LittleClipRawDownloader/1.0)"},
        )

        start_time = time.time()
        try:
            with urllib.request.urlopen(req) as resp, open(target_file, "wb") as f:
                data = resp.read()
                f.write(data)
                download_time = time.time() - start_time
                mb = len(data) / (1024 * 1024)
                speed = mb / download_time if download_time > 0 else 0
                print(f"    -> Done: {mb:.1f} MB in {download_time:.1f}s ({speed:.1f} MB/s)")
                total_downloaded += len(data)
        except Exception as err:
            print(f"    ERROR downloading {filename}: {err}", file=sys.stderr)
            if os.path.exists(target_file):
                os.remove(target_file)
            sys.exit(1)

    all_mb = (total_downloaded + total_existing) / (1024 * 1024)
    print(f"\nAll {len(RAW_SAMPLES)} RAW test files verified in {output_path} ({all_mb:.1f} MB total).")


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
