#!/usr/bin/env python3
"""Observe network strips and current-namespace address facts on private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    network = Path('/sys/class/net')
    hardware = next(path for path in network.iterdir() if path.name != 'lo')
    (root / 'a-loopback').symlink_to(network / 'lo', target_is_directory=True)
    (root / 'b-interface').symlink_to(hardware, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside read-only network observations.\n')


def exercise(session, root):
    session.observe('01-network-strips', ['a-loopback', 'b-interface', 'Loopback'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-loopback', 'b-interface', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-loopback')
    session.click(x + 32, y + 55)
    session.observe('03-loopback-inspector', ['Network interface', 'Operational state', 'MAC address'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-loopback', 'b-interface', '127.0.0.1/8'])
    x, y = session.word('04-folder-return', 'b-interface')
    session.click(x + 32, y + 55)
    session.observe('05-interface-inspector', ['Network interface', 'Carrier', 'MTU'])
    session.scroll_to_visible('05b-queue-counts', ['Observed queue directories', 'Reported queue counts'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-loopback', 'b-interface', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'network link and address previews')


if __name__ == '__main__':
    raise SystemExit(main())
