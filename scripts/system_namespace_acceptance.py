#!/usr/bin/env python3
"""Exercise namespace-handle aliases in the private Weston/Niri compositor."""
import os
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-namespace-strip', ['a-namespace.png', 'Namespace handle'])
    session.click(848, 201)
    x, y = session.word('01-namespace-strip', 'a-namespace.png')
    session.click(x + 32, y + 55)
    session.observe('02-namespace-inspector', ['Namespace handle', 'Namespace descriptor', 'Namespace inode'])
    session.key('Escape')
    session.observe('03-returned-folder', ['a-namespace.png', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-namespace.png').symlink_to(f'/proc/{os.getpid()}/ns/mnt')
    (root / 'b-notes.txt').write_text('Ordinary text alongside a namespace handle.\n')
    return run_application_acceptance(args, output, root, exercise, 'Metadata-only nsfs alias preview')


if __name__ == '__main__':
    raise SystemExit(main())
