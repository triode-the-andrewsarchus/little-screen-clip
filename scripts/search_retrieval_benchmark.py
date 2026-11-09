#!/usr/bin/env python3
"""Run the production Rust SQLite/ranking component benchmark.

Build first: cargo test --release -p clip-workbench --lib --no-run
Synthetic unit vectors are used, with 1.5 segments per real fixture file. This
measures retrieval only, not end-to-end model/source-verification/UI latency.
Owned temporary source/index folders are deleted by the Rust test on completion.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile

TEST = 'file_manager::search::retrieval::benchmark::retrieval_scale_benchmark'


def is_test_harness(binary):
    # Prove this is a Rust test executable before passing it any arguments.
    # Cargo also produces a GUI binary named clip_workbench-* in deps; asking
    # that binary for --list can launch the ordinary application instead.
    with tempfile.TemporaryFile(mode='w+') as symbols:
        result = subprocess.run(['nm', '-C', '--defined-only', str(binary)],
            stdout=symbols, stderr=subprocess.DEVNULL, timeout=30)
        symbols.seek(0)
        return result.returncode == 0 and any('test::test_main_static' in line for line in symbols)


def resolve_binary(args, repo):
    if args.binary:
        binary = args.binary.resolve(strict=True)
        if not is_test_harness(binary):
            raise RuntimeError(f'Refusing to execute a non-test binary: {binary}; build cargo test --release -p clip-workbench --lib --no-run')
        return binary
    candidates = [path for path in (repo / f'target/{args.profile}/deps').glob('clip_workbench-*')
                  if path.is_file() and os.access(path, os.X_OK) and not path.suffix]
    for binary in sorted(candidates, key=lambda path: path.stat().st_mtime_ns, reverse=True):
        if is_test_harness(binary):
            return binary
    raise RuntimeError(f'Build the {args.profile} clip-workbench library tests first; no verified test harness exists')


def execute(binary, env, timeout, log, test=TEST):
    with log.open('w') as stream:
        process = subprocess.Popen([str(binary), test, '--exact', '--ignored', '--nocapture'],
            env=env, stdout=stream, stderr=subprocess.STDOUT, text=True, start_new_session=True)
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            raise RuntimeError(f'Retrieval component benchmark exceeded {timeout}s; see {log}')
        finally:
            # Stop/reap the owned test before the caller cleans its fixture,
            # including cancellation of this Python driver.
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
    output = log.read_text()
    print(output)
    if process.returncode or '1 passed; 0 failed' not in output:
        raise RuntimeError(f'Benchmark did not pass: return code {process.returncode}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--profile', choices=['release', 'debug'], default='release')
    parser.add_argument('--files', type=int, default=100_000)
    parser.add_argument('--samples', type=int, default=21)
    parser.add_argument('--timeout', type=int, default=1800)
    parser.add_argument('--sqlite-cache-mib', type=int, help='Benchmark-only SQLite pager budget, 1..512 MiB; default uses production setting')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if not 100 <= args.files <= 200_000 or not 1 <= args.samples <= 100:
        parser.error('Use 100..200000 files and 1..100 samples')
    if args.sqlite_cache_mib is not None and not 1 <= args.sqlite_cache_mib <= 512:
        parser.error('Use 1..512 MiB for the SQLite pager budget')
    repo = Path(__file__).resolve().parent.parent
    binary = resolve_binary(args, repo)
    report = args.report.resolve()
    report.parent.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CLIP_RETRIEVAL_BENCH_FILES=str(args.files),
               CLIP_RETRIEVAL_BENCH_SAMPLES=str(args.samples), CLIP_RETRIEVAL_BENCH_REPORT=str(report))
    if args.sqlite_cache_mib is not None:
        env['CLIP_RETRIEVAL_BENCH_SQLITE_CACHE_MIB'] = str(args.sqlite_cache_mib)
    else:
        env.pop('CLIP_RETRIEVAL_BENCH_SQLITE_CACHE_MIB', None)
    sources = {name: hashlib.sha256((repo / 'crates/clip-workbench/src/file_manager/search' / name).read_bytes()).hexdigest()
               for name in ('retrieval.rs', 'retrieval/benchmark.rs', 'store.rs', 'store/vectors.rs', 'store/lexical.rs', 'policy.rs', 'query.rs', '../filesystem.rs', '../type_navigation.rs')}
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    log = report.with_suffix(report.suffix + '.stdout.log')
    with tempfile.TemporaryDirectory(prefix='clip-retrieval-bench-') as workspace:
        env['CLIP_RETRIEVAL_BENCH_ROOT'] = workspace
        execute(binary, env, args.timeout, log)
    data = json.loads(report.read_text())
    assert data['files'] == args.files and data['vectors'] == args.files + args.files // 2
    data['test_binary'] = str(binary)
    data['binary_sha256'] = binary_hash
    data['source_sha256'] = sources
    data['stdout_log'] = str(log)
    data['component_targets_evaluated'] = data['profile'] == 'release' and args.files == 100_000 and args.samples >= 20
    if data['component_targets_evaluated']:
        for case in data['cases']:
            budget = 500 if case['name'].startswith('hybrid') else 100
            case['component_budget_ms'] = budget
            case['within_component_budget'] = case['p95_ms'] <= budget
    data['end_to_end_acceptance_proven'] = False
    report.write_text(json.dumps(data, indent=2))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
