#!/usr/bin/env python3
"""Measure uncached strips and persistent scrubbing using the production video helper routing."""
import argparse
import concurrent.futures
import json
from pathlib import Path
import statistics
import struct
import subprocess
import tempfile
import time


def inspect(path):
    result = subprocess.run([
        "ffprobe", "-v", "error", "-select_streams", "v", "-show_entries",
        "stream=codec_name,width,height:stream_disposition=attached_pic:format=duration,size",
        "-of", "json", str(path),
    ], capture_output=True, check=True, text=True)
    data = json.loads(result.stdout)
    stream, video = next((i, v) for i, v in enumerate(data["streams"])
                         if not v.get("disposition", {}).get("attached_pic"))
    return {**video, "stream": stream, "size_bytes": int(data["format"].get("size", 0)), "duration_ms": int(float(data["format"]["duration"]) * 1000)}


def arguments(helper, path, metadata, width, height, mode):
    return [str(helper), str(path), mode, "--width", str(width), "--height", str(height),
            "--video-stream", str(metadata["stream"])] + (
                ["--gpu"] if metadata["codec_name"] == "hevc" else [])


def fast_strip_command(helper, path, output, metadata, width):
    return arguments(helper, path, metadata, width, 64, "--fast") + [
        "--timeline", str(output), "--duration-ms", str(metadata["duration_ms"]),
        "--source-width", str(metadata["width"]), "--source-height", str(metadata["height"]),
        "--codec", metadata["codec_name"],
    ]


def av1_strip_command(path, output, metadata, width):
    duration = metadata["duration_ms"]
    return ["ffmpeg", "-v", "error", "-skip_frame", "nokey", "-threads", "2", "-i", str(path),
            "-map", f"0:v:{metadata['stream']}", "-an", "-sn", "-dn", "-vf",
            f"scale=1:64:flags=area,tpad=stop_mode=clone:stop_duration={duration / 1000},"
            f"fps={width}000/{duration},tile={width}x1", "-frames:v", "1", "-q:v", "2", "-y", str(output)]


def dispatch(metadata, peers=1):
    # Mirror the tested Files policy; decoder workers share a 16-core ceiling.
    import os
    peers = min(4, max(1, peers))
    pixels = metadata["width"] * metadata["height"]
    heavy = (metadata["size_bytes"] >= 512 * 1024 * 1024
             or metadata["duration_ms"] >= 1_800_000 or pixels >= 2_000_000)
    budget = min(16, max(1, len(os.sched_getaffinity(0))))
    peers = min(peers, budget)
    cpu = min(8 if heavy else 4, max(1, budget // peers))
    gpu = min(cpu, 4 if peers == 1 else 2)
    prefer_gpu = ((metadata["codec_name"] == "h264" and pixels >= 2_000_000)
                  or (metadata["codec_name"] == "hevc" and pixels >= 4_000_000))
    return cpu, gpu, prefer_gpu


def strip(helper, path, output, width=3200, peers=1, io_mode=None):
    metadata = inspect(path)
    command = fast_strip_command(helper, path, output, metadata, width)
    if metadata["codec_name"] == "av1" and metadata["duration_ms"] > 600_000:
        command = av1_strip_command(path, output, metadata, width)
    indexed = Path(__file__).resolve().parents[1] / "target/tools/video-strip/video-strip"
    started = time.perf_counter()
    generated = False
    if indexed.is_file() and metadata["duration_ms"] > 600_000:
        cpu_workers, gpu_workers, prefer_gpu = dispatch(metadata, peers)
        timeout = 60 if metadata["width"] * metadata["height"] >= 4_000_000 else 30
        for gpu in ([True, False] if prefer_gpu else [False]):
            try:
                native = subprocess.run([str(indexed), str(path), str(output), str(width),
                                         str(metadata["stream"]), str(metadata["duration_ms"]),
                                         "--workers", str(gpu_workers if gpu else cpu_workers)] +
                                        (["--io-uring"] if io_mode == "uring" else []) +
                                        (["--gpu"] if gpu else []),
                                        stdout=subprocess.DEVNULL, timeout=timeout)
                generated = native.returncode == 0
            except subprocess.TimeoutExpired:
                generated = False
            if generated:
                break
    if not generated:
        subprocess.run(command, stdout=subprocess.DEVNULL, check=True, timeout=240)
    elapsed = time.perf_counter() - started
    print(json.dumps({"file": path.name, "strip_seconds": elapsed, "columns": width}), flush=True)
    return elapsed


def read_exact(pipe, count):
    chunks = bytearray()
    while len(chunks) < count:
        chunk = pipe.read(count - len(chunks))
        if not chunk:
            raise RuntimeError("Scrub helper closed its pipe")
        chunks.extend(chunk)
    return chunks


def scrub(helper, path):
    metadata = inspect(path)
    scale = min(480 / metadata["width"], 320 / metadata["height"])
    width, height = [max(1, round(metadata[key] * scale)) for key in ("width", "height")]
    command = arguments(helper, path, metadata, width, height, "--preview-server")
    child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    timings, timestamps = [], []
    duration = metadata["duration_ms"]
    # Continuous forward scrubbing, then reverse through the same positions.
    positions = [min(duration - 100, round(duration * fraction + i * duration / 1024))
                 for fraction in (.1, .5, .8) for i in range(8)]
    positions += list(reversed(positions[-8:]))
    try:
        for position in positions:
            started = time.perf_counter()
            child.stdin.write(f"{position}\n".encode())
            child.stdin.flush()
            timestamp = struct.unpack("<Q", read_exact(child.stdout, 8))[0]
            pixels = read_exact(child.stdout, width * height * 4)
            timings.append((time.perf_counter() - started) * 1000)
            timestamps.append(timestamp)
            assert 0 <= timestamp <= duration + 1000
            assert len(pixels) == width * height * 4
        child.stdin.close()
        child.wait(timeout=5)
        assert child.returncode == 0
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
    warm = sorted(timings[1:])
    print(json.dumps({"file": path.name, "cold_hover_ms": timings[0],
                      "warm_hover_median_ms": statistics.median(warm),
                      "warm_hover_p95_ms": warm[round((len(warm) - 1) * .95)],
                      "warm_hover_max_ms": max(warm),
                      "cached_reverse_median_ms": statistics.median(timings[-8:]),
                      "distinct_keyframes": len(set(timestamps)),
                      "requests": len(positions)}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", type=Path, nargs="*")
    parser.add_argument("--helper", type=Path, default=Path(__file__).resolve().parents[1] /
                        "target/tools/timelens/timelens")
    parser.add_argument("--limit", type=int, default=6)
    parser.add_argument("--width", type=int, default=3200)
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--synthetic", action="store_true")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="files-video-benchmark-") as directory:
        output = Path(directory)
        if args.synthetic:
            fixture = output / "synthetic # 短片.mp4"
            subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                            "testsrc2=size=640x360:rate=30", "-t", "8", "-c:v", "libx264",
                            "-threads", "2", "-g", "30", "-pix_fmt", "yuv420p", str(fixture)],
                           check=True)
            args.paths.append(fixture)
        for path in args.paths:
            files = sorted(p for p in path.iterdir() if p.suffix.lower() in
                           {".mkv", ".mp4", ".mov", ".webm"})[:args.limit] if path.is_dir() else [path]
            assert files, f"No videos in {path}"
            started = time.perf_counter()
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
                futures = [pool.submit(strip, args.helper, file, output / f"{i}.jpg", args.width,
                                       min(len(files), args.workers, 4))
                           for i, file in enumerate(files)]
                for future in futures:
                    future.result()
            print(json.dumps({"path": str(path), "batch_seconds": time.perf_counter() - started,
                              "files": len(files), "workers": args.workers}), flush=True)
            scrub(args.helper, files[0])


if __name__ == "__main__":
    main()
