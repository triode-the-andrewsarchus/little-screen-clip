#!/usr/bin/env python3
"""Compare synthetic click-to-visible-selection bounds on private Weston/Niri.

Run raster/native separately with the same --scale and --samples. These are
observation upper bounds including process launch and screenshot delivery, not
physical-input, GPU-presentation timestamps, or a low-latency benchmark.
"""
import hashlib
import json
import math
from pathlib import Path
import statistics
import subprocess
import time

from PIL import Image

import ui_acceptance_capture as private
from native_mirror_acceptance import MixedMirrorDaemon


def border_count(path, scale):
    with Image.open(path) as image:
        assert image.size == (round(1280 * scale), round(800 * scale)), image.size
        strip = image.convert('RGB').crop(tuple(round(value * scale) for value in (999, 160, 1006, 390)))
        pixels = strip.tobytes()
        return sum(b > 140 and g > 120 and r < 135 and g - r > 35 and b - r > 40
                   for r, g, b in zip(pixels[0::3], pixels[1::3], pixels[2::3]))


def distribution(values):
    return {'median_ms': statistics.median(values),
            'p95_ms': sorted(values)[math.ceil(len(values) * .95) - 1],
            'min_ms': min(values), 'max_ms': max(values)}


def measure(args, config, env, outputs, children, daemon):
    private.verify_private_wayland_target(env, config['runtime'])
    pointer = private.build_private_virtual_pointer(args.output_dir)
    private.run_checked([str(pointer), 'probe'], env=env)
    size = private.output_logical_size(outputs)
    assert size == (1280, 800), size
    app_env = env | {'LITTLE_CLIP_NATIVE_MIRROR': '1' if args.mode == 'native' else '0'}
    app = children.start('workbench', private.app_arguments(args, args.binary), env=app_env,
                         cwd=config['scratch'], log_path=args.output_dir / 'workbench.log')
    windows, _ = private.wait_for_app_window(config['niri_bin'], env, app, args.output_dir)
    geometry = private.geometry_observation(windows[0], size)
    assert geometry['centered'], geometry
    time.sleep(12)  # Same preparation settling period as the existing mixed-content fixture.

    def capture(name):
        path = args.output_dir / f'{name}.png'
        # Each observation owns a new destination; never accept an older async capture.
        assert not path.exists(), path
        started = time.monotonic()
        private.screenshot(config['niri_bin'], env, path, poll_interval=.001)
        deadline = time.monotonic() + 8
        while True:
            try:
                count = border_count(path, args.scale)
                break
            except OSError:  # The async writer may still be completing the PNG.
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.001)
        return path, count, (time.monotonic() - started) * 1000

    def click():
        private.verify_private_wayland_target(env, config['runtime'])
        private.run_checked([str(pointer), 'click', '1280', '800', '1100', '240'], env=env)

    ready, unselected, capture_ms = capture('ready')
    content = subprocess.check_output(['tesseract', str(ready), 'stdout', '-l', 'eng'],
                                      stderr=subprocess.DEVNULL, text=True, timeout=30)
    assert all(text in ''.join(content.split()) for text in ('sample.rs', 'Stereo.wav')), content
    click()
    time.sleep(.4)
    _, selected, _ = capture('calibration-selected')
    assert selected - unselected > 100 * args.scale ** 2, (unselected, selected)
    click()
    time.sleep(.4)
    _, restored, _ = capture('calibration-unselected')
    threshold = (selected + unselected) / 2
    assert restored < threshold, (restored, threshold)

    rows = []
    captures = [capture_ms]
    for index in range(args.samples):
        expected = index % 2 == 0
        started = time.monotonic()
        click()
        dispatch_ms = (time.monotonic() - started) * 1000
        attempts = []
        deadline = started + 5
        while time.monotonic() < deadline:
            _, count, cost_ms = capture(f'input-{index:02d}-{len(attempts):02d}')
            elapsed = (time.monotonic() - started) * 1000
            captures.append(cost_ms)
            attempts.append({'observed_after_ms': elapsed, 'border_pixels': count, 'capture_ms': cost_ms})
            if (count > threshold) == expected:
                break
        else:
            raise RuntimeError(f'Click {index} selection did not appear: {attempts}')
        rows.append({'selected': expected, 'dispatch_return_ms': dispatch_ms,
                     'visible_upper_bound_ms': elapsed, 'observations': attempts})
        time.sleep(.4)  # Keep individual clicks distinct; no implicit timing reset in a sample.
    return {'geometry': geometry, 'content_ready': True,
            'calibration_pixels': {'selected': selected, 'unselected': unselected, 'threshold': threshold},
            'samples': rows, 'visible_upper_bounds': distribution([row['visible_upper_bound_ms'] for row in rows]),
            'capture_cost': distribution(captures), 'daemon': daemon.close()}


def main():
    parser = private.create_parser()
    parser.add_argument('--mode', choices=('raster', 'native'), required=True)
    parser.add_argument('--samples', type=int, default=20)
    args = parser.parse_args()
    assert 2 <= args.samples <= 100
    assert args.appearance == "dark", "The calibrated border fixture uses dark appearance"
    assert (args.width, args.height) == (1280, 800), 'This fixed geometry fixture supports 1280x800 only'
    args.fake_text_preview = True
    args.binary = private.validate_cli(args, Path(__file__).resolve().parent.parent)
    args.output_dir = args.output_dir.expanduser().resolve()
    args.output_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
    with args.binary.open('rb') as binary_file:
        binary_hash = hashlib.file_digest(binary_file, 'sha256').hexdigest()
    children = private.ChildProcesses()
    daemon = None
    report = {'status': 'running', 'mode': args.mode, 'scale': args.scale,
              'binary': str(args.binary), 'binary_sha256': binary_hash,
              'versions': {tool: private.command_version(tool) for tool in ('niri', 'weston')},
              'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
              'screenshot_poll_ms': 1,
              'scope': 'Synthetic private Wayland click dispatch to observed selection pixels; upper bounds include capture/process overhead. No physical input or GPU presentation latency claim.'}
    try:
        config = private.prepare_configuration(args, args.output_dir)
        _, env, outputs = private.start_compositors(config, args, args.output_dir, children)
        daemon = MixedMirrorDaemon(config['runtime'], args.output_dir / 'fake-daemon-events.json')
        report.update(measure(args, config, env, outputs, children, daemon))
        daemon = None
        report['status'] = 'passed'
    except Exception as exc:
        report.update(status='failed', error=str(exc))
        raise
    finally:
        children.cleanup()
        if daemon is not None:
            daemon.close()
        report['processes'] = children.exit_statuses()
        (args.output_dir / 'input-report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report['visible_upper_bounds']))


if __name__ == '__main__':
    main()
