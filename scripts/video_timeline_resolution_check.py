#!/usr/bin/env python3
"""Regression check: a 128-frame luminance ramp must survive full-width timeline generation."""
import argparse
from pathlib import Path
import subprocess
import tempfile
from PIL import Image
from video_preview_benchmark import av1_strip_command


def check_strip(output, mode, width):
    with Image.open(output) as image:
        assert image.size == (width, 64), image.size
        # Indexed columns span [0, duration-1]; rate-filtered columns
        # use duration/width. Match each sampling model at known frame
        # PTS, away from nearest-frame midpoint ties.
        positions = ([round(i * 4000 / 128 * (width - 1) / 3999) for i in range(128)]
                     if mode.startswith("indexed") else
                     [round((i + .25) * width / 128) for i in range(128)])
        levels = [image.getpixel((x, 32))[0] for x in positions]
        distinct = len(set(levels))
        assert distinct >= 110, f"Temporal detail lost: {distinct} of 128 luminance steps at width {width}"
        assert all(b >= a - 2 for a, b in zip(levels, levels[1:])), "Timeline order changed"
        assert min(levels) <= 2 and max(levels) >= 145, "Timeline does not cover the complete clip"
        print(f"{mode} {width} columns: {distinct}/128 luminance steps retained")


def check_progressive(command, output, expected, mode, width):
    preview = output.with_suffix(".progress.ppm")
    preview.unlink(missing_ok=True)
    subprocess.run(command + ["--progressive", str(preview)], check=True, timeout=30)
    assert output.read_bytes() == expected, "Progressive output changed final pixels"
    if preview.exists():
        with Image.open(preview) as image:
            assert image.size == (width, 64)
            row = [image.getpixel((x, 32))[0] for x in range(width)]
            assert all(b >= a - 2 for a, b in zip(row, row[1:])), "Partial strip has gaps/order errors"
            if mode.startswith("indexed"):
                assert row[0] <= 2 and row[-1] >= 145, "Partial strip misses the clip endpoints"

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--helper", type=Path, default=Path(__file__).resolve().parents[1] /
                        "target/tools/timelens/timelens")
    parser.add_argument("--gpu", action="store_true", help="Also verify CUDA indexed decoding")
    parser.add_argument("--io-uring", action="store_true", help="Verify the optional indexed prefetch reader")
    parser.add_argument("--progressive", action="store_true", help="Verify progressive output retains identical final detail")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="files-timeline-resolution-") as directory:
        root = Path(directory)
        source = root / "128 distinct frames.mp4"
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                        "nullsrc=size=256x16:rate=32,geq=lum='16+N':cb=128:cr=128",
                        "-frames:v", "128", "-c:v", "libx264", "-qp", "0", "-threads", "2",
                        "-g", "64", "-pix_fmt", "yuv420p", str(source)], check=True)
        indexed = Path(__file__).resolve().parents[1] / "target/tools/video-strip/video-strip"
        intra = root / "128 indexed frames.mp4"
        subprocess.run(["ffmpeg", "-v", "error", "-i", str(source), "-c:v", "libx264",
                        "-qp", "0", "-g", "1", "-threads", "2", str(intra)], check=True)
        hevc = root / "128 indexed frames.hevc.mkv"
        subprocess.run(["ffmpeg", "-v", "error", "-i", str(source), "-vf", "scale=256:64",
                        "-c:v", "libx265", "-x265-params",
                        "lossless=1:keyint=1:pools=2:frame-threads=1:log-level=error", str(hevc)],
                       check=True, timeout=30)
        av1 = root / "128 distinct frames.av1.mkv"
        subprocess.run(["ffmpeg", "-v", "error", "-i", str(source), "-vf", "scale=256:64", "-c:v", "librav1e",
                        "-speed", "10", "-qp", "0", "-g", "1", "-threads", "2", str(av1)],
                       check=True, timeout=30)
        modes = ["timelens", "av1", "indexed", "indexed_hevc"] + (["indexed_gpu"] if args.gpu else [])
        for mode, width in [(mode, width) for mode in modes
                            for width in ((200, 400, 800, 1600, 3200) if mode.startswith("indexed")
                                          else (1024, 2048, 3200))]:
            output = root / f"timeline-{width}.jpg"
            command = [str(args.helper), str(source), "--fast", "--timeline", str(output),
                            "--width", str(width), "--height", "64", "--video-stream", "0",
                            "--duration-ms", "4000", "--source-width", "256", "--source-height", "16",
                            "--codec", "h264"]
            if mode == "av1":
                command = av1_strip_command(av1, output, {"duration_ms": 4000, "stream": 0}, width)
            if mode.startswith("indexed"):
                fixture = hevc if mode == "indexed_hevc" else intra
                command = [str(indexed), str(fixture), str(output), str(width), "0", "4000"]
                if mode == "indexed_gpu":
                    command.append("--gpu")
                if args.io_uring:
                    command.append("--io-uring")
            subprocess.run(command, check=True, timeout=30)
            expected = output.read_bytes()
            if args.progressive and mode != "av1":
                check_progressive(command, output, expected, mode, width)
            check_strip(output, mode, width)



if __name__ == "__main__":
    main()
