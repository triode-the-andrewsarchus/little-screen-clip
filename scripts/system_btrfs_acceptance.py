#!/usr/bin/env python3
"""Exercise a mounted Btrfs sysfs instance on the private compositor."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def source():
    import uuid
    for path in sorted(Path('/sys/fs/btrfs').iterdir()):
        try:
            uuid.UUID(path.name)
            return path
        except ValueError:
            continue
    raise RuntimeError('No mounted Btrfs sysfs instance available')


def exercise(session, root):
    session.observe('01-mounted-filesystem', ['a-btrfs', 'Btrfs filesystem'])
    session.click(848, 201)
    x, y = session.word('01-mounted-filesystem', 'a-btrfs')
    session.click(x + 32, y + 55)
    session.observe('02-btrfs-inspector', ['Btrfs filesystem', 'Filesystem UUID', 'Filesystem label'])
    x, y = session.word('02-btrfs-inspector', 'Refresh', tree=True, minimum_y=150)
    session.click(x + 20, y)
    session.observe('03-refreshed-inspector', ['Btrfs filesystem', 'Filesystem UUID', 'Filesystem label'])
    session.key('Escape')
    session.observe('04-returned-folder', ['a-btrfs', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-btrfs').symlink_to(source(), target_is_directory=True)
    (root / 'b-notes.txt').write_text('Ordinary file alongside filesystem metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'Btrfs mounted filesystem preview')


if __name__ == '__main__':
    raise SystemExit(main())
