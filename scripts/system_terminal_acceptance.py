#!/usr/bin/env python3
"""Read-only virtual-console activity acceptance in private Weston/Niri."""
from pathlib import Path
import json
import subprocess
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-terminal-folder', ['a-current-vt', 'b-specific-vt'])
    session.click(848, 201)
    x, y = session.word('01-terminal-folder', 'a-current-vt')
    session.click(x + 32, y + 55)
    session.observe('02-current-vt', ['Virtual-console activity', 'Current-console alias'])
    expected = json.loads((session.output / 'active-process-fixture.json').read_text())
    if expected['pid'] is not None:
        session.scroll_to_visible('02b-active-process', ['Observed terminal relationships', f"PID {expected['pid']}"])
    session.scroll_to_visible('03-allocation-observed', ['Virtual-console allocation', 'allocated console'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-current-vt', 'b-specific-vt'])
    x, y = session.word('04-folder-return', 'b-specific-vt')
    session.click(x + 32, y + 55)
    session.observe('05-specific-vt', ['Virtual-console activity', 'tty1'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-current-vt', 'b-specific-vt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    snapshot_path = output / 'process-snapshot.json'
    subprocess.run([str(args.binary.resolve()), '--internal-system-processes', str(snapshot_path)], check=True, timeout=3)
    active = Path('/sys/class/tty/tty0/active').read_text().strip()
    number = int(active.removeprefix('tty'))
    snapshot = json.loads(snapshot_path.read_bytes())
    matching = [process['stat']['pid'] for process in snapshot['processes'] if process['stat']['tty'] == (4 << 8) | number]
    (output / 'active-process-fixture.json').write_text(json.dumps({'vt': active, 'pid': matching[0] if matching else None}))
    for name, source in [('a-current-vt', '/dev/tty0'), ('b-specific-vt', '/dev/tty1')]:
        Path(source).stat()
        (root / name).symlink_to(source)
    (root / 'notes.txt').write_text('Ordinary text beside passive terminal metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'Passive virtual-console activity')


if __name__ == '__main__':
    raise SystemExit(main())
