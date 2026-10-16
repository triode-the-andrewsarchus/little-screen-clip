#!/usr/bin/env python3
"""CPU-node exposure and raw USB bus directories on private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-directory-strips', ['a-cpu', 'b-usb', 'CPU access interfaces'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-cpu', 'b-usb', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-cpu')
    session.click(x + 32, y + 55)
    session.observe('03-cpu-directory', ['CPU access interfaces', 'observed logical CPUs', 'CPUID nodes', 'MSR nodes', 'not opened'])
    session.key('Escape')
    session.observe('04-browser-return', ['a-cpu', 'b-usb'])
    x, y = session.word('04-browser-return', 'b-usb')
    session.click(x + 32, y + 55)
    session.observe('05-usb-directory', ['Raw USB bus interfaces', 'observed USB buses', 'USB vendor ID'])
    session.key('Escape')
    session.observe('06-final-browser', ['a-cpu', 'b-usb', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-cpu').symlink_to('/dev/cpu', target_is_directory=True)
    (root / 'b-usb').symlink_to('/dev/bus/usb', target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary text beside passive CPU and USB directory metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'CPU access and raw USB bus directories')


if __name__ == '__main__':
    raise SystemExit(main())
