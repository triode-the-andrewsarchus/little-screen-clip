#!/usr/bin/env python3
"""Exercise passive VCS metadata and a denied explicit capture in private Weston/Niri."""
import os
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-console-row', ['a-screen', 'VT 1'])
    session.click(848, 201)
    x, y = session.word('01-console-row', 'a-screen')
    session.click(x + 32, y + 55)
    session.observe('02-console-inspector', ['Capture console snapshot', 'Console screen access', 'Unverified'])
    x, y = session.word('02-console-inspector', 'Capture', tree=True)
    session.click(x + 10, y)
    session.observe('03-restricted-screen', ['Screen contents restricted', 'VT 1'])
    x, y = session.word('03-restricted-screen', 'Refresh', tree=True)
    session.click(x + 10, y)
    session.observe('04-metadata-refresh', ['Screen contents restricted', 'Capture console snapshot'])
    session.key('Escape')
    session.observe('05-folder-return', ['a-screen', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    node = Path('/dev/vcsa1')
    stat = node.stat()
    # This acceptance deliberately chooses a denied node; it never captures host screen bytes.
    groups = set(os.getgroups()) | {os.getgid()}
    if stat.st_uid == os.getuid() or stat.st_gid in groups or stat.st_mode & 4:
        raise RuntimeError('Requires a root-owned VCSA node unreadable by this user; no permissions are changed')
    (root / 'a-screen').symlink_to(node)
    (root / 'b-notes.txt').write_text('Ordinary text next to console metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'Passive VCS metadata and explicit restricted capture')


if __name__ == '__main__':
    raise SystemExit(main())
