#!/usr/bin/env python3
"""Check progressive snapshots, final pixel equality, and repeated render overhead.

Run on local fixtures with no concurrent builds or encodes. Rotates ordinary and
progressive invocations; progress polling mirrors Files' 50 ms worker interval.
CPU seconds exclude GPU execution/waits; elapsed time is the acceptance metric.
"""
import argparse
import hashlib
import json
from pathlib import Path
import resource
import statistics
import subprocess
import tempfile
import time
from PIL import Image
from video_preview_benchmark import fast_strip_command, dispatch, inspect

ROOT = Path(__file__).resolve().parents[1]


def command_for(args, source, output, metadata, width, progressive, preview):
    short = metadata["duration_ms"] <= 600_000
    helper = args.timelens if short else args.indexed
    previous = args.previous_timelens if short else args.previous_indexed
    if not progressive and previous:
        helper = previous
    if short:
        command = fast_strip_command(helper, source, output, metadata, width)
    else:
        cpu, gpu, prefer_gpu = dispatch(metadata)
        command = [str(helper), str(source), str(output), str(width), str(metadata["stream"]),
                   str(metadata["duration_ms"]), "--workers", str(gpu if prefer_gpu else cpu)]
        if prefer_gpu:
            command.append("--gpu")
    if progressive:
        command += ["--progressive", str(preview)]
    return command


def snapshot(path, width):
    with Image.open(path) as image:
        assert image.size == (width, 64), image.size
        assert image.mode == "RGB", image.mode
        # Exercise the bounded pixel conversion performed by the real worker.
        return hashlib.sha256(image.convert("RGBA").tobytes()).hexdigest()


def measure(command, output, preview, width):
    started = time.perf_counter()
    cpu_start = resource.getrusage(resource.RUSAGE_CHILDREN)
    first = None
    modified = None
    snapshots = []
    with tempfile.TemporaryFile() as errors:
        child = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=errors)
        try:
            while child.poll() is None:
                if time.perf_counter() - started > 120:
                    raise TimeoutError(command)
                if preview.is_file() and preview.stat().st_mtime_ns != modified:
                    signature = snapshot(preview, width)
                    modified = preview.stat().st_mtime_ns
                    first = first or time.perf_counter() - started
                    snapshots.append(signature)
                time.sleep(.05)
            assert child.returncode == 0, command
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
        errors.seek(0)
        diagnostic = errors.read().decode(errors="replace")
    elapsed = time.perf_counter() - started
    cpu_end = resource.getrusage(resource.RUSAGE_CHILDREN)
    return {"seconds": elapsed, "first_seconds": first, "updates": len(snapshots),
            "distinct_updates": len(set(snapshots)), "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
            "cpu_seconds": cpu_end.ru_utime + cpu_end.ru_stime - cpu_start.ru_utime - cpu_start.ru_stime,
            "decoder": diagnostic}


def run_case(args, source, width, temporary):
    metadata = inspect(source)
    records = {False: [], True: []}
    expected = None
    for round_number in range(args.rounds):
        for progressive in ([False, True] if round_number % 2 == 0 else [True, False]):
            output = temporary / "final.jpg"
            preview = temporary / "preview.ppm"
            preview.unlink(missing_ok=True)
            command = command_for(args, source, output, metadata, width, progressive, preview)
            result = measure(command, output, preview, width)
            expected = expected or result["sha256"]
            assert result["sha256"] == expected, "Progressive rendering changed final pixels"
            result.update(file=str(source), width=width, progressive=progressive, round=round_number)
            records[progressive].append(result)
            print(json.dumps(result), flush=True)
    summarize(args, source, width, records)


def summarize(args, source, width, records):
    ordinary = statistics.median(r["seconds"] for r in records[False])
    progressive = statistics.median(r["seconds"] for r in records[True])
    first = [r["first_seconds"] for r in records[True] if r["first_seconds"] is not None]
    overhead = (progressive / ordinary - 1) * 100
    summary = {"file": str(source), "width": width, "ordinary_median": ordinary,
               "progressive_median": progressive, "overhead_percent": overhead,
               "first_median": statistics.median(first) if first else None,
               "final_pixels_identical": True}
    print(json.dumps(summary), flush=True)
    assert overhead <= args.max_overhead, summary
    if ordinary > 1:
        assert len(first) == len(records[True]), "A slow render never published a preview"
        assert summary["first_median"] < progressive * .8, "Preview arrived too late"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", type=Path, nargs="+")
    parser.add_argument("--indexed", type=Path, default=ROOT / "target/tools/video-strip/video-strip")
    parser.add_argument("--timelens", type=Path, default=ROOT / "target/tools/timelens/timelens")
    parser.add_argument("--previous-indexed", type=Path)
    parser.add_argument("--previous-timelens", type=Path)
    parser.add_argument("--widths", default="800,3200")
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--max-overhead", type=float, default=10)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="video-progressive-check-") as directory:
        for source in args.paths:
            for width in map(int, args.widths.split(",")):
                run_case(args, source, width, Path(directory))


if __name__ == "__main__":
    main()
