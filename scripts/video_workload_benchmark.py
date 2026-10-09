#!/usr/bin/env python3
"""Compare indexed readers/worker counts on local copies and their NAS originals.

The manifest is produced by scripts/copy_video_test_fixtures.py. No source files
are modified. --cold-local evicts only the selected local file's clean cached
pages; it does not flush global caches or the NAS. Timings exclude UI painting.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
from video_preview_benchmark import inspect, dispatch, strip


def indexed(helper, source, output, width, workers, gpu, mode, cold):
    if cold:
        with source.open("rb") as file:
            os.posix_fadvise(file.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
    metadata = inspect(source)
    started = time.perf_counter()
    command = [str(helper), str(source), str(output), str(width), str(metadata["stream"]),
               str(metadata["duration_ms"]), "--workers", str(workers)]
    if gpu:
        command.append("--gpu")
    if mode == "uring":
        command.append("--io-uring")
    result = subprocess.run(command, capture_output=True, timeout=120, check=True)
    seconds = time.perf_counter() - started
    return {"file": source.name, "seconds": seconds, "workers": workers, "gpu": gpu,
            "mode": mode, "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
            "decoder": result.stderr.decode(errors="replace")}


def run_folder(args, records, folder, storage, temporary):
    sources = [Path(record[storage]) for record in records]
    metadata = [inspect(source) for source in sources]
    expected = {}
    modes = ["sync", "uring"] if args.compare_io else ["sync"]
    counts = [int(value) for value in args.workers.split(",")] if args.workers else [None]
    for round_number in range(args.rounds):
        for mode in (modes if round_number % 2 == 0 else list(reversed(modes))):
            for fixed in counts:
                started = time.perf_counter()
                def run(pair):
                    i, source = pair
                    cpu, gpu, prefer_gpu = dispatch(metadata[i], min(len(sources), 4))
                    workers = fixed or (gpu if prefer_gpu else cpu)
                    result = indexed(args.helper, source, temporary / f"{i}.ppm", args.width,
                                     workers, prefer_gpu, mode, args.cold_local and storage == "local")
                    result.update(folder=folder, storage=storage, round=round_number)
                    return result
                with concurrent.futures.ThreadPoolExecutor(max_workers=min(4, len(sources))) as pool:
                    results = list(pool.map(run, enumerate(sources)))
                batch = time.perf_counter() - started
                for result in results:
                    key = result["file"]
                    assert expected.setdefault(key, result["sha256"]) == result["sha256"], key
                    print(json.dumps(result), flush=True)
                print(json.dumps({"folder": folder, "storage": storage, "mode": mode,
                                  "round": round_number, "batch_seconds": batch}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--helper", type=Path, default=Path(__file__).resolve().parents[1] /
                        "target/tools/video-strip/video-strip")
    parser.add_argument("--folders", nargs="*")
    parser.add_argument("--storage", choices=["local", "source", "both"], default="both")
    parser.add_argument("--workers", help="Fixed sweep, e.g. 1,2,4,8,16; otherwise use Files dispatch")
    parser.add_argument("--compare-io", action="store_true")
    parser.add_argument("--cold-local", action="store_true")
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--width", type=int, default=3200)
    args = parser.parse_args()
    records = json.loads(args.manifest.read_text())["files"]
    folders = sorted({Path(record["local"]).parent.name for record in records})
    storage = ["local", "source"] if args.storage == "both" else [args.storage]
    with tempfile.TemporaryDirectory(prefix="video-workload-") as temporary:
        for folder in args.folders or folders:
            selected = [record for record in records if Path(record["local"]).parent.name == folder]
            assert selected, folder
            for place in storage:
                run_folder(args, selected, folder, place, Path(temporary))


if __name__ == "__main__":
    main()
