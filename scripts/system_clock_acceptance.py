#!/usr/bin/env python3
"""Passive RTC previews on private Weston/Niri; no RTC/watchdog device opens."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    clock = next(iter(sorted(Path('/sys/class/rtc').iterdir())))
    (root / 'a-clock').symlink_to(clock, target_is_directory=True)
    access = Path('/dev') / clock.name
    (root / 'b-access').symlink_to(access)
    (root / 'notes.txt').write_text('Ordinary file beside passive RTC metadata.\n')


def exercise(session, root):
    session.observe('01-clock-strips', ['a-clock', 'b-access', 'RTC'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-clock', 'b-access', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-clock')
    session.click(x + 32, y + 55)
    session.observe('03-clock-inspector', ['Clock identity', 'RTC date', 'RTC time', 'RTC seconds since epoch'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-clock', 'b-access'])
    x, y = session.word('04-folder-return', 'b-access')
    session.click(x + 32, y + 55)
    session.observe('05-access-inspector', ['RTC clock metadata', 'Clock identity', 'RTC date', 'RTC time'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-clock', 'b-access', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive RTC clock metadata')


if __name__ == '__main__':
    raise SystemExit(main())
