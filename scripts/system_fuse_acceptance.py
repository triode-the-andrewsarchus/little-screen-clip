#!/usr/bin/env python3
"""Exercise a live FUSE control connection on the private compositor."""
import os
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def source():
    for path in sorted(Path('/sys/fs/fuse/connections').iterdir()):
        if path.name.isdigit() and path.stat().st_uid == os.getuid():
            return path
    raise RuntimeError('No owned FUSE control connection available')


def exercise(session, root):
    session.observe('01-fuse-connection', ['a-fuse', 'Outstanding userspace requests'])
    session.click(848, 201)
    x, y = session.word('01-fuse-connection', 'a-fuse')
    session.click(x + 32, y + 55)
    session.observe('02-fuse-inspector', ['FUSE control connections', 'Connection identifier', 'Background request limit'])
    session.key('Escape')
    session.observe('03-returned-folder', ['a-fuse', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-fuse').symlink_to(source(), target_is_directory=True)
    (root / 'b-notes.txt').write_text('Ordinary file alongside FUSE control metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'FUSE passive connection preview')


if __name__ == '__main__':
    raise SystemExit(main())
