#!/usr/bin/env python3
"""Owned Q8 helper startup, inference and memory probes; no index or user content."""
import argparse
import base64
from collections import deque
import ctypes
import json
import math
import mmap
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import tempfile
import threading
import time
import xml.etree.ElementTree as ET
from quality import Worker
from smoke import image, audio, vectors
from setup import FILES, digest


def gpu_memory(xml, pid):
    devices = []
    for gpu in ET.fromstring(xml).findall('gpu'):
        for process in gpu.findall('processes/process_info'):
            if process.findtext('pid', '').strip() != str(pid):
                continue
            raw = process.findtext('used_memory', '').strip()
            match = re.fullmatch(r'(\d+) MiB', raw)
            devices.append({'uuid': gpu.findtext('uuid'), 'type': process.findtext('type'),
                            'bytes': int(match[1]) * 1024 * 1024 if match else None})
    known = bool(devices) and all(device['bytes'] is not None for device in devices)
    return {'gpu_bytes': sum(device['bytes'] for device in devices) if known else None,
            'gpu_status': 'reported' if known else 'unknown' if devices else 'not-listed',
            'gpu_devices': devices}


def process_memory(pid):
    fields = {}
    for line in Path(f'/proc/{pid}/status').read_text().splitlines():
        key, _, value = line.partition(':')
        if key in ('VmRSS', 'VmHWM'):
            fields[key] = int(value.split()[0]) * 1024
        elif key == 'Threads':
            fields[key] = int(value)
    return fields


class Sampler:
    def __init__(self):
        self.records = deque(maxlen=10000)
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.thread = None
        self.total_samples = 0
        self.peaks = {'gpu_bytes': None, 'VmHWM': None}

    def start(self, pid):
        self.pid = pid
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def capture(self):
        with self.lock:
            record = {'time': time.monotonic()}
            try:
                record.update(process_memory(self.pid))
            except (OSError, ValueError) as error:
                record['process_error'] = str(error)
            try:
                output = subprocess.run(['nvidia-smi', '-q', '-x'], capture_output=True, timeout=2, check=True)
                record.update(gpu_memory(output.stdout, self.pid))
            except (OSError, ValueError, subprocess.SubprocessError, ET.ParseError) as error:
                record.update(gpu_status='unavailable', gpu_bytes=None, gpu_error=str(error))
            self.records.append(record)
            self.total_samples += 1
            for field in self.peaks:
                value = record.get(field)
                if value is not None:
                    self.peaks[field] = max(value, self.peaks[field] if self.peaks[field] is not None else value)
            return record

    def run(self):
        while not self.stop.is_set():
            self.capture()
            self.stop.wait(0.2)

    def close(self):
        self.stop.set()
        if self.thread:
            self.thread.join(3)
            if self.thread.is_alive():
                raise RuntimeError('Resource sampler did not retire')


def resident_pages(path):
    with path.open('rb') as file, mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_COPY) as mapping:
        pages = math.ceil(len(mapping) / os.sysconf('SC_PAGE_SIZE'))
        residency = (ctypes.c_ubyte * pages)()
        library = ctypes.CDLL(None, use_errno=True)
        library.mincore.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_void_p]
        address = ctypes.addressof(ctypes.c_char.from_buffer(mapping))
        if library.mincore(address, len(mapping), residency):
            raise OSError(ctypes.get_errno(), 'mincore failed')
        return sum(bool(value & 1) for value in residency), pages


def prepare_models(source, destination, cold):
    records = []
    for name, size, checksum in FILES:
        path = source / name
        if cold:
            copied = destination / name
            shutil.copyfile(path, copied)
            copied.chmod(0o600)
            path = copied
        if path.stat().st_size != size or digest(path) != checksum:
            raise ValueError(f'Pinned Q8 model mismatch: {path}')
        if cold:
            with path.open('rb') as file:
                os.fsync(file.fileno())
                os.posix_fadvise(file.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        resident, pages = resident_pages(path)
        records.append({'file': name, 'sha256': checksum, 'resident_pages_before_start': resident, 'pages': pages})
    return records


def requests(media):
    cases = [
        ('short_query', {'op': 'text', 'inputs': ['task: search result | query: a document about mountain hiking'], 'token_limit': 512}),
        ('near_limit_query', {'op': 'text', 'inputs': ['task: search result | query: ' + 'report ' * 480], 'token_limit': 512}),
        ('near_limit_batch4', {'op': 'text', 'inputs': ['title: none | text: ' + 'analysis ' * 480] * 4, 'token_limit': 2048}),
    ]
    if media:
        cases.extend([
            ('image_1024', {'op': 'image', 'data': base64.b64encode(image(1024)).decode(), 'budget': 280}),
            ('audio_30s', {'op': 'audio', 'data': base64.b64encode(audio(30)).decode()}),
        ])
    return cases


def measure(worker, request, samples):
    first = time.monotonic()
    first_vectors = vectors(worker.request(request))
    first_ms = (time.monotonic() - first) * 1000
    times = []
    for _ in range(samples):
        start = time.monotonic()
        vectors(worker.request(request))
        times.append((time.monotonic() - start) * 1000)
    times.sort()
    return {'first_ms': first_ms, 'first_vectors': first_vectors, 'samples': samples, 'median_ms': statistics.median(times),
            'min_ms': times[0], 'max_ms': times[-1],
            'p95_ms': times[math.ceil(samples * 0.95) - 1] if samples >= 20 else None}


def probe(args):
    sampler = Sampler()
    worker = None
    started = time.monotonic()
    try:
        worker = Worker(args, args.output_dir, ('--text-only',) if args.text_only else (), on_spawn=sampler.start)
        startup_ms = (time.monotonic() - started) * 1000
        ready = sampler.capture()
        cases = []
        for name, request in requests(not args.text_only):
            timing = measure(worker, request, args.samples)
            timing.update(name=name, after=sampler.capture())
            cases.append(timing)
            print(json.dumps({'case': name, 'timing': {key: value for key, value in timing.items() if key != 'first_vectors'}}), flush=True)
        report = {'model': worker.ready, 'pid': worker.process.pid, 'fresh_process_startup_ms': startup_ms,
                  'ready': ready, 'cases': cases}
    finally:
        try:
            if worker:
                worker.close()
        finally:
            sampler.close()
        (args.output_dir / 'resource-samples.json').write_text(json.dumps(list(sampler.records), indent=2))
        (args.output_dir / 'resource-summary.json').write_text(json.dumps({
            'samples_total': sampler.total_samples, 'samples_retained': len(sampler.records),
            'records_truncated': sampler.total_samples > len(sampler.records), 'peaks': sampler.peaks}, indent=2))
    report['helper_reaped'] = worker.process.poll() is not None
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--models', type=Path, required=True)
    parser.add_argument('--device', choices=['cuda', 'vulkan', 'cpu'], required=True)
    parser.add_argument('--text-only', action='store_true')
    parser.add_argument('--cold-model-pages', action='store_true')
    parser.add_argument('--samples', type=int, default=7, choices=range(1, 22))
    parser.add_argument('--output-dir', type=Path, required=True)
    args = parser.parse_args()
    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=False, mode=0o700)
    source = args.models.resolve()
    with tempfile.TemporaryDirectory(prefix='model-pages-', dir=args.output_dir) as copies:
        metadata = prepare_models(source, Path(copies), args.cold_model_pages)
        args.models = Path(copies) if args.cold_model_pages else source
        try:
            report = probe(args)
        except BaseException as error:
            (args.output_dir / 'failure.json').write_text(json.dumps({'error': str(error)}))
            raise
    summary = json.loads((args.output_dir / 'resource-summary.json').read_text())
    report.update({'weights': metadata, 'driver_sha256': digest(Path(__file__)),
        'source_sha256': {name: digest(Path(__file__).with_name(name)) for name in ['quality.py', 'smoke.py', 'worker.py']},
        'runtime_manifest_sha256': digest(args.runtime / 'runtime.json'),
        'native_shim_sha256': digest(args.runtime / 'lib/libclip-search.so'),
        'model_pages_verified_nonresident': args.cold_model_pages and all(row['resident_pages_before_start'] == 0 for row in metadata),
        'system_disk_cold_proven': False, 'sampled_gpu_peak_bytes': summary['peaks']['gpu_bytes'],
        'kernel_rss_peak_bytes': summary['peaks']['VmHWM'],
        'resource_sample_count': summary['samples_total'], 'resource_records_truncated': summary['records_truncated'],
        'scope': 'Fresh owned helper, bounded synthetic inference with concurrent telemetry; GPU peak is sampled, RSS HWM is kernel-reported. Excludes SQLite, extraction and UI; model-page cold does not prove physical disk/system cold.',
        'full_phase6_acceptance_proven': False})
    steady = [report['ready'], *(case['after'] for case in report['cases'])]
    known = all(row.get('gpu_bytes') is not None for row in steady)
    report['steady_gpu_target'] = {'budget_bytes': 2 * 1024**3,
        'within_target': max(row['gpu_bytes'] for row in steady) <= 2 * 1024**3 if known else None}
    (args.output_dir / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps({key: value for key, value in report.items() if key not in ('cases', 'weights')}), flush=True)


if __name__ == '__main__':
    main()
