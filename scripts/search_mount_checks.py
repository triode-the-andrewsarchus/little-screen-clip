#!/usr/bin/env python3
"""Run guarded search mount tests in disposable user/mount namespaces.

Build tests first with `cargo test -p clip-file-manager --lib --no-run`. No mount is
created in the host namespace. Fixtures are removed only after the child and its
private mounts have exited, including on failure. This script sends no UI input.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
from search_retrieval_benchmark import resolve_binary

TEST = 'file_manager::search::reader::tests::bind_mount_boundaries_block_inventory_extraction_and_absence_checks'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--test-binary', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    binary = resolve_binary(argparse.Namespace(binary=args.test_binary, profile='debug'), repo, crate='clip-file-manager')
    with tempfile.TemporaryDirectory(prefix='clip-search-private-mount-') as fixture:
        env = dict(os.environ, CLIP_SEARCH_PRIVATE_MOUNT_TEST='1',
                   CLIP_SEARCH_PRIVATE_MOUNT_FIXTURE=fixture)
        command = ['unshare', '--user', '--map-root-user', '--mount', '--propagation', 'private',
                   '--', str(binary), TEST, '--exact', '--ignored', '--nocapture']
        try:
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=90)
            output = result.stdout + result.stderr
            passed = result.returncode == 0 and '1 passed; 0 failed' in output
            report = {'status': 'passed' if passed else 'failed', 'binary': str(binary),
                      'test': TEST, 'namespace': 'private user/mount', 'returncode': result.returncode,
                      'output': output}
        except subprocess.TimeoutExpired:
            report = {'status': 'failed', 'test': TEST, 'error': 'Private mount check exceeded 90 seconds'}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2))
    print(json.dumps(report))
    return 0 if report['status'] == 'passed' else 1


if __name__ == '__main__':
    raise SystemExit(main())
