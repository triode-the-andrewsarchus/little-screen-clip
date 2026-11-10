#!/usr/bin/env python3
"""Compare real prepared audio frames with original/current query implementations.

Development-only: requires clean query source files. Builds two library test
executables with the same Cargo profile, replacing only the two query methods
for the baseline build and restoring current sources before any timed runs.
Owned build processes are stopped before restoration on error/interruption.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import sys
import statistics
import subprocess

from search_retrieval_benchmark import is_test_harness
from ui_acceptance_capture import ChildProcesses, stop_process

TEST = 'workbench_layout_tests::waveform_cost::measure_prepared_audio_frames'
QUERIES = ('crates/little-clip-core/src/audio/waveform.rs',
           'crates/clip-workbench/src/rolling_audio_strip.rs')


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def method(source):
    # These two controlled methods contain no brace-bearing string literals.
    start = source.index('    pub fn query_peaks(')
    body = source.index('{', start)
    depth = 1
    end = body + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return start, end


def command(children, repo, name, args, log, timeout):
    child = children.start(name, args, env=os.environ.copy(), cwd=repo, log_path=log)
    try:
        child.wait(timeout=timeout)
    finally:
        stop_process(child)
    if child.returncode:
        raise RuntimeError(f'{name} exited {child.returncode}; see {log}')
    return log.read_text()


def build(children, repo, output, label):
    text = command(children, repo, label + '-build',
        ['cargo', 'test', '-j4', '-p', 'clip-workbench', '--lib', '--no-run', '--message-format=json'],
        output / f'{label}-build.jsonl', 600)
    artifacts = []
    for line in text.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if (message.get('reason') == 'compiler-artifact' and message.get('executable')
                and message['target']['name'].replace('-', '_') == 'clip_workbench'
                and message['profile']['test']):
            artifacts.append(Path(message['executable']))
    assert len(artifacts) == 1, artifacts
    assert is_test_harness(artifacts[0]), 'Refusing to execute a GUI binary as a test'
    executable = output / f'{label}-test'
    shutil.copy2(artifacts[0], executable)
    return executable


def measure(children, repo, output, executables, rounds):
    rows = {'baseline': [], 'current': []}
    for round_index in range(rounds):
        order = ['baseline', 'current'] if round_index % 2 == 0 else ['current', 'baseline']
        for label in order:
            print(f'Measuring {label} round {round_index + 1}/{rounds}', flush=True)
            rss = output / f'{label}-{round_index}-rss.txt'
            text = command(children, repo, f'{label}-{round_index}',
                [sys.executable, '-c',
                 "import resource,subprocess,sys; from pathlib import Path; "
                 "r=subprocess.run(sys.argv[2:]); "
                 "Path(sys.argv[1]).write_text(str(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)); "
                 "sys.exit(r.returncode)",
                 str(rss), str(executables[label]), TEST, '--exact', '--ignored', '--nocapture', '--test-threads=1'],
                output / f'{label}-{round_index}.log', 180)
            assert 'test result: ok. 1 passed' in text, text[-1000:]
            cases = [json.loads(line.split('WAVEFORM_FRAME_COST ', 1)[1])
                     for line in text.splitlines() if 'WAVEFORM_FRAME_COST ' in line]
            assert len(cases) == 16, len(cases)
            rows[label].append({'max_rss_kib': int(rss.read_text()), 'cases': cases})
    return rows


def compare_runs(rows):
    comparisons = []
    for index, first in enumerate(rows['baseline'][0]['cases']):
        fields = ('kind', 'width', 'density', 'seconds', 'fingerprint', 'vertices', 'indices', 'geometry_bytes', 'frames')
        for samples in rows.values():
            for sample in samples:
                assert all(first[field] == sample['cases'][index][field] for field in fields), (first, sample['cases'][index])
        summary = {field: first[field] for field in ('kind', 'width', 'density', 'seconds', 'geometry_bytes')}
        for label in rows:
            for statistic in ('median_ms', 'p95_ms'):
                summary[f'{label}_{statistic}'] = statistics.median(run['cases'][index][statistic] for run in rows[label])
        comparisons.append(summary)
    return {'runs': rows, 'comparisons': comparisons,
            'rss_median_kib': {label: statistics.median(run['max_rss_kib'] for run in samples) for label, samples in rows.items()}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output-dir', type=Path, required=True)
    parser.add_argument('--baseline-ref', default='4fabcc3')
    parser.add_argument('--rounds', type=int, choices=range(3, 10), default=7)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    output = args.output_dir.resolve()
    output.mkdir(parents=True, mode=0o700, exist_ok=False)
    originals, baseline = {}, {}
    for name in QUERIES:
        subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--', name], cwd=repo, check=True, stdout=subprocess.DEVNULL)
        path = repo / name
        originals[path] = path.read_text()
        old = subprocess.check_output(['git', 'show', f'{args.baseline_ref}:{name}'], cwd=repo, text=True)
        a, b = method(originals[path]); c, d = method(old)
        baseline[path] = originals[path][:a] + old[c:d] + originals[path][b:]
        (output / path.parent.name).mkdir(exist_ok=True)
        (output / path.parent.name / path.name).write_text(originals[path])
    fixture = repo / 'crates/clip-workbench/src/workbench/tests/waveform_cost.rs'
    fixture_hash = digest(fixture)
    children = ChildProcesses()
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    try:
        print('Building baseline query variant', flush=True)
        try:
            for path, text in baseline.items():
                path.write_text(text)
            before = build(children, repo, output, 'baseline')
        finally:
            children.cleanup()
            changed = []
            for path, original in originals.items():
                if path.read_text() == baseline[path]:
                    path.write_text(original)
                elif path.read_text() != original:
                    changed.append(str(path))
            if changed:
                raise RuntimeError(f'Sources edited concurrently; preserved those edits: {changed}')
        print('Building restored current query variant', flush=True)
        after = build(children, repo, output, 'current')
        assert digest(fixture) == fixture_hash, 'Measurement fixture changed while building'
        assert all(path.read_text() == text for path, text in originals.items()), 'Current query source changed while building'
        report = compare_runs(measure(children, repo, output, {'baseline': before, 'current': after}, args.rounds))
        report.update(status='passed', profile='matched Cargo development/test profile; CPU painting and tessellation only',
            revision=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=repo, text=True).strip(),
            baseline_ref=args.baseline_ref, compiler=subprocess.check_output(['rustc', '--version'], text=True).strip(),
            query_sources={str(path.relative_to(repo)): digest(path) for path in originals},
            executables={label: digest(path) for label, path in [('baseline', before), ('current', after)]},
            fixture_sha256=fixture_hash, driver_sha256=digest(Path(__file__)))
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report['comparisons'], indent=2))
    finally:
        children.cleanup()


if __name__ == '__main__':
    main()
