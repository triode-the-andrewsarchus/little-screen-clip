#!/usr/bin/env python3
"""Passive GPIO registration previews on private Weston/Niri; no chip opens."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    chip = next(path for path in sorted(Path('/sys/bus/gpio/devices').iterdir()) if path.name.startswith('gpiochip'))
    (root / 'a-controller').symlink_to(chip, target_is_directory=True)
    access = next(path for path in sorted(Path('/dev').glob('gpiochip*')) if path.name == chip.name)
    (root / 'b-access').symlink_to(access)
    (root / 'notes.txt').write_text('Ordinary file beside passive GPIO registration metadata.\n')


def exercise(session, root):
    session.observe('01-gpio-strips', ['a-controller', 'b-access', 'GPIO'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-controller', 'b-access', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-controller')
    session.click(x + 32, y + 55)
    session.observe('03-controller-inspector', ['GPIO device number', 'GPIO observation policy', 'GPIO/PWM registration'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-controller', 'b-access'])
    x, y = session.word('04-folder-return', 'b-access')
    session.click(x + 32, y + 55)
    session.observe('05-access-inspector', ['GPIO device number', 'Permissions', 'Unverified; no device opened'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-controller', 'b-access', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive GPIO registration metadata')


if __name__ == '__main__':
    raise SystemExit(main())
