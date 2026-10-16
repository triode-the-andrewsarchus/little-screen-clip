#!/usr/bin/env python3
"""Exercise ordinary-file loop inspection on the private compositor.

Positive attachment and replacement cases use Rust fixtures; this script needs
no privileged loop creation and does not assume that the host has attachments.
"""
from pathlib import Path
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-ordinary-file', ['a-image.img', 'b-notes.txt'])
    session.click(848, 201)
    x, y = session.word('01-ordinary-file', 'a-image.img')
    private.run_checked([str(session.pointer), 'right-click', '1280', '800', str(x + 10), str(y)], env=session.env)
    session.observe('02-inspection-action', ['Inspect loop associations'])
    x, y = session.word('02-inspection-action', 'Inspect')
    session.click(x, y)
    session.observe('03-file-associations', ['File loop associations', 'Loop inventory coverage'])
    session.key('Escape')
    session.observe('04-restored-folder', ['a-image.img', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-image.img').write_bytes(b'\x00' * 4096)
    (root / 'b-notes.txt').write_text('Ordinary file navigation stays available.\n')
    return run_application_acceptance(args, output, root, exercise, 'ordinary-file reverse loop inspection')


if __name__ == '__main__':
    raise SystemExit(main())
