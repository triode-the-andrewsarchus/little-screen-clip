#!/usr/bin/env python3
"""PTY and stable-device-alias inventories on the private Weston/Niri runner."""
from pathlib import Path
import os
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from system_process_acceptance import terminal


def exercise(session, root):
    session.observe('01-directory-strips', ['a-terminals', 'b-disk-aliases', 'Allocated pseudo-terminals'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-terminals', 'b-disk-aliases', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-terminals')
    session.click(x + 32, y + 55)
    session.observe('03-terminal-directory', ['Allocated pseudo-terminals', 'observed allocated PTYs'])
    session.scroll_to_visible('04-directory-process-scan', ['Process scan coverage', 'FD presence does not establish active use'])
    session.key('Escape')
    session.observe('05-browser-return', ['a-terminals', 'b-disk-aliases'])
    x, y = session.word('05-browser-return', 'b-disk-aliases')
    session.click(x + 32, y + 55)
    session.observe('06-stable-alias-directory', ['Stable device aliases', 'distinct resolved device targets'])
    session.key('Escape')
    session.observe('07-final-browser', ['a-terminals', 'b-disk-aliases', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    child, master, _ = terminal(root, 'c-owned-terminal')
    try:
        (root / 'a-terminals').symlink_to('/dev/pts', target_is_directory=True)
        (root / 'b-disk-aliases').symlink_to('/dev/disk', target_is_directory=True)
        (root / 'notes.txt').write_text('Ordinary text beside bounded device directory inventories.\n')
        return run_application_acceptance(args, output, root, exercise, 'PTY and stable alias directory inventories')
    finally:
        child.terminate()
        child.wait(timeout=5)
        os.close(master)


if __name__ == '__main__':
    raise SystemExit(main())
