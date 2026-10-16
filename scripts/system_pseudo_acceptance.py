#!/usr/bin/env python3
"""Inspect kmsg semantics and log-location links without opening their contents."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-device-strip', ['a-kmsg', 'notes.txt', 'Kernel message interface'])
    session.click(848, 201)
    x, y = session.word('01-device-strip', 'a-kmsg')
    session.click(x + 32, y + 55)
    session.observe('02-pseudo-inspector', ['Kernel log inspection', 'Readability is unverified', 'No log content was read'])
    session.scroll_to_visible('03-log-locations', ['Related kernel objects', 'System journal directory'])
    # These links are metadata-only here; do not browse or preview the host journal.
    session.key('Escape')
    session.observe('04-browser-return', ['a-kmsg', 'notes.txt'])
    x, y = session.word('04-browser-return', 'notes.txt')
    session.click(x + 20, y)
    session.key('Return')
    session.observe('05-ordinary-text', ['Ordinary preview beside static device semantics', 'Edit'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-kmsg', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    if not Path('/dev/kmsg').exists():
        raise RuntimeError('No exposed /dev/kmsg; use the synthetic pseudo-device tests')
    if not any(Path(path).is_dir() for path in ['/var/log/journal', '/run/log/journal']):
        raise RuntimeError('No conventional journal directory; use the synthetic location fixture')
    (root / 'a-kmsg').symlink_to('/dev/kmsg')
    (root / 'notes.txt').write_text('Ordinary preview beside static device semantics.\n')
    return run_application_acceptance(args, output, root, exercise, 'pseudo-device semantics and journal location links')


if __name__ == '__main__':
    raise SystemExit(main())
