#!/usr/bin/env python3
"""Registered I2C bus/client previews on private Weston/Niri; no bus probing."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    adapters = sorted(Path('/sys/bus/i2c/devices').glob('i2c-*'))
    adapter = next(path for path in adapters if any(child.name.split('-', 1)[0].isdigit() and '-' in child.name for child in path.iterdir()))
    client = next(child for child in sorted(adapter.iterdir()) if '-' in child.name and child.name.split('-', 1)[0].isdigit())
    (root / 'a-adapter').symlink_to(adapter, target_is_directory=True)
    (root / 'b-client').symlink_to(client, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive registered-bus metadata.\n')


def exercise(session, root):
    session.observe('01-registration-maps', ['a-adapter', 'b-client', 'kernel registrations'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-adapter', 'b-client', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-adapter')
    session.click(x + 32, y + 55)
    session.observe('03-adapter-inspector', ['Registered bus number', 'Registered device name', 'Adapter/controller parent'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-adapter', 'b-client'])
    x, y = session.word('04-folder-return', 'b-client')
    session.click(x + 32, y + 55)
    session.observe('05-client-inspector', ['Registered bus number', 'Registered I', 'Registered device driver'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-adapter', 'b-client', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'registered I2C bus and device previews')


if __name__ == '__main__':
    raise SystemExit(main())
