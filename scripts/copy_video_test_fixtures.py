#!/usr/bin/env python3
"""Copy grouped video fixtures with a SHA-256 manifest and a strict 20 GB cap."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import time

LIMIT = 20_000_000_000


def copy_one(source, output):
    before = source.stat()
    assert source.is_file(), source
    output.parent.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256()
    size = 0
    started = time.monotonic()
    with source.open("rb") as inp, tempfile.NamedTemporaryFile(dir=output.parent) as target:
        while chunk := inp.read(4 * 1024 * 1024):
            size += len(chunk)
            if size > before.st_size:
                raise ValueError("Source grew during copy")
            target.write(chunk)
            digest.update(chunk)
        target.flush()
        os.fsync(target.fileno())
        assert size == before.st_size and source.stat().st_mtime_ns == before.st_mtime_ns
        # Atomic no-overwrite publication; the temporary link is removed on exit.
        os.link(target.name, output)
    return {"source": str(source), "local": str(output), "bytes": size,
            "sha256": digest.hexdigest(), "seconds": time.monotonic() - started}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--group", nargs=2, action="append", required=True,
                        metavar=("FOLDER", "SOURCE"))
    parser.add_argument("--append", action="store_true", help="Append unused paths to our existing manifest")
    args = parser.parse_args()
    destination = args.destination.resolve()
    manifest_path = destination / "manifest.json"
    if args.append:
        manifest = json.loads(manifest_path.read_text())
        existing = sum(Path(record["local"]).stat().st_size for record in manifest["files"])
    else:
        destination.mkdir(parents=True, exist_ok=False)
        manifest = {"limit_bytes": LIMIT, "files": []}
        existing = 0
    selected = []
    for folder, path in args.group:
        assert folder not in {"", ".", ".."} and Path(folder).name == folder, folder
        source = Path(path).resolve()
        output = destination / folder / source.name
        assert not output.exists(), output
        selected.append((source, output))
    planned = existing + sum(source.stat().st_size for source, _ in selected)
    if planned > LIMIT:
        raise ValueError(f"Copy budget exceeded: {planned} > {LIMIT}")
    manifest["planned_bytes"] = planned
    manifest_path.write_text(json.dumps(manifest, indent=2))
    for source, output in selected:
        record = copy_one(source, output)
        existing += record["bytes"]
        manifest["files"].append(record)
        manifest["copied_bytes"] = existing
        manifest_path.write_text(json.dumps(manifest, indent=2))
        print(json.dumps(record), flush=True)


if __name__ == "__main__":
    main()
