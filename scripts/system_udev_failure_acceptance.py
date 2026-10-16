#!/usr/bin/env python3
"""Observe app-local udev subscription failure, overflow and recovery privately."""
from pathlib import Path
import json
import re
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-mixed-folder', ['a-network', 'notes.txt'])
    session.click(848, 201)
    x, y = session.word('01-mixed-folder', 'a-network')
    session.click(x + 32, y + 55)
    session.observe('02-initial-unavailable', ['Network interface', 'udev unavailable', 'Carrier'])
    session.observe('03-subscribed', ['Network interface', 'udev monitoring plus visible revalidation'])
    session.observe('04-overflow-gap', ['Network interface', 'udev unavailable', 'Carrier'])
    session.observe('05-resumed', ['Network interface', 'udev monitoring plus visible revalidation', 'Carrier'])
    session.key('Escape')
    session.observe('06-browser-return', ['a-network', 'notes.txt'])
    x, y = session.word('06-browser-return', 'notes.txt')
    session.click(x + 20, y)
    session.key('Return')
    session.observe('07-ordinary-text', ['Ordinary preview survives event monitor faults', 'Edit'])
    session.key('Escape')
    session.observe('08-final-folder', ['a-network', 'notes.txt'])
    lines = (session.output / 'faults.log').read_text().splitlines()
    records = [(int(ms), int(pid), event, int(attempt)) for ms, pid, event, attempt in map(str.split, lines)]
    assert all(pid == session.process.pid for _, pid, _, _ in records), records
    attempts = [row for row in records if row[2] != 'overflow']
    assert [row[3] for row in attempts] == list(range(8)), records
    assert [row[2] for row in attempts] == ['denied'] * 4 + ['enabled', 'denied', 'denied', 'enabled'], records
    assert sum(row[2] == 'overflow' for row in records) == 1, records
    assert all(right[0] - left[0] >= 1800 for left, right in zip(attempts, attempts[1:])), records
    generations = {}
    for stage in ['02-initial-unavailable', '04-overflow-gap', '05-resumed']:
        text = ''.join((session.output / f'{stage}.txt').read_text().lower().split())
        match = re.search(r'objectgeneration(\d+)', text)
        assert match, stage
        generations[stage] = int(match.group(1))
    assert generations['04-overflow-gap'] > generations['02-initial-unavailable'], generations
    assert generations['05-resumed'] >= generations['04-overflow-gap'], generations
    (session.output / 'fault-proof.json').write_text(json.dumps({'status': 'passed', 'records': records, 'generations': generations,
        'scope': 'app-local libudev faults; no kernel events or device payloads manufactured'}, indent=2))


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-network').symlink_to('/sys/class/net/lo', target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary preview survives event monitor faults.\n')
    library = output / 'udev-fault.so'
    source = Path(__file__).resolve().parent / 'fixtures/system_udev_fault.c'
    subprocess.run(['cc', '-shared', '-fPIC', '-std=c11', '-Wall', '-Wextra', '-Werror', '-o', str(library), str(source), '-ldl'], check=True)
    def environment(env, _children):
        env['LD_PRELOAD'] = str(library)
        env['VEYLAR_UDEV_FAULT_LOG'] = str(output / 'faults.log')
        env.pop('VEYLAR_UDEV_FAULT_OWNER', None)
    return run_application_acceptance(args, output, root, exercise, 'udev failure, overflow and reconnect', environment)


if __name__ == '__main__':
    raise SystemExit(main())
