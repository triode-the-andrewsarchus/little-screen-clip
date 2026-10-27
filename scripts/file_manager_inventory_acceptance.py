#!/usr/bin/env python3
"""Large flat-directory search crosses the old stdout limit without losing entries."""
import json
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_search_acceptance import submit


COUNT = 100_000


def check_transfer(binary, root, output):
    def call(request, **options):
        return subprocess.run([str(binary.resolve()), '--internal-file-search-reader'],
            input=json.dumps(request).encode(), capture_output=True, timeout=150, check=True, **options)
    observed = json.loads(call({'Probe': str(root)}).stdout)['Ok']['Root']
    with tempfile.TemporaryFile() as inventory:
        started = time.monotonic()
        response = call({'List': {'root': observed, 'path': str(root), 'hidden': False,
            'exclusions': [], 'output': inventory.fileno()}}, pass_fds=(inventory.fileno(),))
        elapsed = time.monotonic() - started
        assert json.loads(response.stdout) == {'Ok': 'DirectoryWritten'}, response.stdout[:1000]
        inventory_bytes = inventory.tell()
        assert inventory_bytes > 24 * 1024 * 1024, inventory_bytes
        inventory.seek(0)
        directory = json.load(inventory)
        assert len(directory['entries']) == COUNT
        assert directory['skipped'] == 0
        assert len({entry['path'] for entry in directory['entries']}) == COUNT
    report = {'files': COUNT, 'inventory_bytes': inventory_bytes,
        'stdout_bytes': len(response.stdout), 'list_seconds': elapsed,
        'complete_inventory': True, 'transport': 'owned anonymous file descriptor'}
    (output / 'inventory-transfer.json').write_text(json.dumps(report, indent=2))


def exercise(session, root):
    session.observe('initial-large-directory', ['file_000000.unknown'])
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    started = time.monotonic()
    submit(session, ':*.unknown file_099999;scope:recursive')
    session.observe('last-file-search', ['file_099999.unknown', '1 matching entries',
        '0 unsupported/incomplete reads'])
    elapsed = time.monotonic() - started
    assert not index.exists(), 'Explicit filename traversal must not enroll or build an index'
    (session.output / 'large-directory-search.json').write_text(json.dumps({
        'files': COUNT, 'last_file_found': True, 'incomplete_reads': 0,
        'no_index_created': True, 'submit_to_observed_seconds': elapsed,
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for index in range(COUNT):
        (root / f'file_{index:06}.unknown').touch()
    check_transfer(args.binary, root, output)
    result = run_application_acceptance(args, output, root, exercise,
        '100,000 actual files: bounded descriptor inventory, complete recursive filename search, no implicit indexing')
    # Keep reports/screenshots, release the large owned fixture only after the
    # private app and helpers are retired by the runner.
    if result == 0:
        shutil.rmtree(root)
    return result


if __name__ == '__main__':
    raise SystemExit(main())
