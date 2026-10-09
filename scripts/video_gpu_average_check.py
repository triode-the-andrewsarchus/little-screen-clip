#!/usr/bin/env python3
"""Verify full-frame CUDA area reduction, or benchmark paired real workloads.

No encoding/builds occur during benchmarks. Stage times are sums of parallel
worker host-call durations, not elapsed time or isolated CUDA kernel timings.
"""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

import numpy as np
from PIL import Image
from video_preview_benchmark import dispatch, inspect

HELPER = Path(__file__).resolve().parents[1] / 'target/tools/video-strip/video-strip'


def render(source, output, metadata, enabled, workers, width, slices=0):
    env = {**os.environ, 'VIDEO_STRIP_PROFILE': '1'}
    env.pop('VIDEO_STRIP_GPU_AVERAGE', None)
    env['VIDEO_STRIP_GPU_SLICES'] = str(slices)
    if enabled is not None:
        env['VIDEO_STRIP_GPU_AVERAGE'] = str(int(enabled))
    command = [str(HELPER), str(source), str(output), str(width),
               str(metadata['stream']), str(metadata['duration_ms']),
               '--gpu', '--workers', str(workers)]
    started = time.perf_counter()
    result = subprocess.run(command, env=env, capture_output=True, text=True,
                            check=True, timeout=120)
    elapsed = time.perf_counter() - started
    if enabled is not False:
        assert 'averaged=' in result.stderr and 'downloaded=0' in result.stderr
        assert 'unavailable' not in result.stderr, result.stderr
        kernel = {0: 'area_column', 5: 'five_slice_column', 1: 'center_column'}[slices]
        assert f'GPU reduction: {kernel}' in result.stderr, result.stderr
    return {'seconds': elapsed, 'stages': result.stderr}


def compare(old, new):
    with Image.open(old) as image:
        a = np.asarray(image).astype(np.int16)
    with Image.open(new) as image:
        b = np.asarray(image).astype(np.int16)
    assert a.shape == b.shape
    error = np.abs(a - b)
    return {'mae': float(error.mean()), 'p99': float(np.percentile(error, 99)),
            'max': int(error.max())}


def verify_sampled_column(source, output, metadata, pixel, slices):
    """Independent reference from decoded pixels, not the full-frame average."""
    ten = pixel == 'yuv420p10le'
    decoded = subprocess.run(['ffmpeg', '-v', 'error', '-threads', '2', '-i', str(source),
        '-frames:v', '1', '-pix_fmt', pixel, '-f', 'rawvideo', 'pipe:1'],
        capture_output=True, check=True, timeout=30).stdout
    values = np.frombuffer(decoded, dtype='<u2' if ten else np.uint8)
    width, height = metadata['width'], metadata['height']
    offset, planes = 0, []
    for plane in range(3):
        w, h = (width, height) if plane == 0 else ((width + 1) // 2, (height + 1) // 2)
        source_plane = values[offset:offset + w * h].reshape(h, w)
        offset += w * h
        if slices == 1:
            rows = source_plane[(2 * np.arange(64) + 1) * h // 128, w // 2]
        else:
            columns = [i * (w - 1) // 4 for i in range(5)]
            sampled = source_plane[:, columns].astype(float).sum(axis=1)
            rows = []
            for row in range(64):
                begin, end = row * h / 64, (row + 1) * h / 64
                indices = np.arange(int(begin), int(np.ceil(end)))
                weights = np.minimum(end, indices + 1) - np.maximum(begin, indices)
                rows.append(np.floor((sampled[indices] * weights).sum() / (5 * (end - begin)) + .5))
        planes.append(np.array(rows, dtype='<u2' if ten else np.uint8).tobytes())
    rgb = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'rawvideo', '-pixel_format',
        'yuv444p10le' if ten else 'yuv444p', '-video_size', '1x64', '-i', 'pipe:0',
        '-frames:v', '1', '-vf', 'scale=1:64:flags=area', '-pix_fmt', 'rgb24',
        '-threads', '1', '-f', 'rawvideo', 'pipe:1'], input=b''.join(planes),
        capture_output=True, check=True, timeout=30).stdout
    reference = np.frombuffer(rgb, dtype=np.uint8).reshape(64, 3).astype(np.int16)
    with Image.open(output) as image:
        actual = np.asarray(image)[:, 0, :].astype(np.int16)
    error = np.abs(actual - reference)
    assert error.max() <= 2, {'slices': slices, 'reference_max_error': int(error.max())}


def synthetic(root, args):
    # Nonuniform color detail at the edges and fractional vertical boundaries
    # expose center-slice shortcuts, missing chroma, stride and bit-depth errors.
    for codec, pixel, dimensions in [('libx264', 'yuv420p', '1920x1080'),
                                      ('libx265', 'yuv420p10le', '3840x2160')]:
        source = root / f'{codec}.mkv'
        command = ['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
                   f'testsrc2=size={dimensions}:rate=8', '-frames:v', '8',
                   '-c:v', codec, '-pix_fmt', pixel, '-g', '1', '-threads', '2']
        if codec == 'libx265':
            command += ['-x265-params', 'pools=2:frame-threads=1:log-level=error']
        subprocess.run(command + [str(source)], check=True, timeout=60)
        metadata = inspect(source)
        for workers in (1, 4):
            a, b = root / 'old.ppm', root / 'new.ppm'
            render(source, a, metadata, False, workers, 128)
            render(source, b, metadata, True, workers, 128)
            error = compare(a, b)
            assert error['mae'] < 3 and error['p99'] < 10, error
            print(json.dumps({'synthetic': codec, 'workers': workers, **error}), flush=True)
            default = root / 'default.ppm'
            render(source, default, metadata, None, workers, 128)
            assert default.read_bytes() == b.read_bytes(), 'Default did not use GPU averaging'
            for slices in ([5, 1] if args.sparse_modes else [5] if args.five_slices else []):
                sparse = root / f'slices-{slices}.ppm'
                render(source, sparse, metadata, True, workers, 128, slices=slices)
                verify_sampled_column(source, sparse, metadata, pixel, slices)
                print(json.dumps({'sparse_synthetic': codec, 'workers': workers, 'slices': slices,
                                  'quality_vs_full': compare(b, sparse)}), flush=True)
            if args.previous_helper:
                previous = root / 'previous.ppm'
                subprocess.run([str(args.previous_helper), str(source), str(previous), '128',
                    str(metadata['stream']), str(metadata['duration_ms']), '--gpu',
                    '--workers', str(workers)], check=True, timeout=30,
                    env={**os.environ, 'VIDEO_STRIP_GPU_AVERAGE': '1', 'VIDEO_STRIP_GPU_SLICES': '0'},
                    capture_output=True)
                assert previous.read_bytes() == b.read_bytes(), 'Full-frame rollback output changed'
                print(json.dumps({'previous_identical': codec, 'workers': workers}), flush=True)
            if args.fallback_helper:
                output = root / 'fallback.ppm'
                command = [str(args.fallback_helper), str(source), str(output), '128',
                           str(metadata['stream']), str(metadata['duration_ms']),
                           '--gpu', '--workers', str(workers)]
                result = subprocess.run(command, capture_output=True, text=True,
                    env={**os.environ, 'VIDEO_STRIP_GPU_AVERAGE': '1', 'VIDEO_STRIP_PROFILE': '1'},
                    check=True, timeout=30)
                assert 'averaged=0' in result.stderr and 'downloaded=' in result.stderr
                assert output.read_bytes() == a.read_bytes(), 'Fallback changed the old output'
                print(json.dumps({'fallback': codec, 'workers': workers, 'identical': True}), flush=True)


def compare_modes(root, folder, sources, modes):
    """Report quality separately from the timed mode-dispatch loop."""
    baseline = modes[0][0]
    for mode, _, _ in modes[1:]:
        for i in range(len(sources)):
            error = compare(root / f'{folder}-{i}-{baseline}.ppm', root / f'{folder}-{i}-{mode}.ppm')
            if baseline == 'download':
                assert error['mae'] < 3 and error['p99'] < 10, error
            print(json.dumps({'folder': folder, 'file': i, 'mode': mode,
                              'quality': error}), flush=True)

def benchmark(args, root):
    modes = [('download', False, 0), ('full', True, 0)]
    if args.five_slices or args.sparse_modes:
        modes = [('full', True, 0), ('five', True, 5)]
    if args.sparse_modes:
        modes.append(('center', True, 1))
    baseline = modes[0][0]
    records = json.loads(args.manifest.read_text())['files']
    for folder in args.folders:
        selected = [r for r in records if Path(r['local']).parent.name == folder]
        assert selected, folder
        sources = [Path(r[args.storage]) for r in selected]
        metadata = [inspect(p) for p in sources]
        totals = {mode: [] for mode, _, _ in modes}
        for repeat in range(args.rounds):
            shift = repeat % len(modes)
            for mode, enabled, slices in modes[shift:] + modes[:shift]:
                def run(index):
                    _, workers, _ = dispatch(metadata[index], len(sources))
                    output = root / f'{folder}-{index}-{mode}.ppm'
                    return render(sources[index], output, metadata[index],
                                  enabled, workers, args.width, slices=slices)
                started = time.perf_counter()
                with concurrent.futures.ThreadPoolExecutor(max_workers=min(4, len(sources))) as pool:
                    results = list(pool.map(run, range(len(sources))))
                elapsed = time.perf_counter() - started
                totals[mode].append(elapsed)
                print(json.dumps({'folder': folder, 'storage': args.storage,
                                  'repeat': repeat, 'mode': mode,
                                  'batch_seconds': elapsed, 'files': results}), flush=True)
            compare_modes(root, folder, sources, modes)
        old = statistics.median(totals[baseline])
        for mode, _, _ in modes[1:]:
            new = statistics.median(totals[mode])
            print(json.dumps({'summary': folder, 'baseline': baseline, 'candidate': mode,
                              'old_seconds': old, 'new_seconds': new,
                              'speedup': old / new}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--synthetic', action='store_true')
    parser.add_argument('--sparse-modes', action='store_true',
                        help='Compare full GPU averaging, five slices and unaveraged center column')
    parser.add_argument('--five-slices', action='store_true',
                        help='Compare full GPU averaging with five vertical slices')
    parser.add_argument('--previous-helper', type=Path,
                        help='Verify full-frame output against the saved pre-experiment helper')
    parser.add_argument('--fallback-helper', type=Path,
                        help='Synthetic checks also verify this reducer-unavailable helper')
    parser.add_argument('--manifest', type=Path,
                        default=Path('/home/vega/Videos/video-preview-tests-20261008/manifest.json'))
    parser.add_argument('--folders', nargs='+', default=['single-wide-4k', 'single-4k', 'h264-batch'])
    parser.add_argument('--storage', choices=['local', 'source'], default='local')
    parser.add_argument('--rounds', type=int, default=3)
    parser.add_argument('--width', type=int, default=3200)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='video-gpu-average-') as directory:
        if args.synthetic:
            synthetic(Path(directory), args)
        else:
            benchmark(args, Path(directory))


if __name__ == '__main__':
    main()
