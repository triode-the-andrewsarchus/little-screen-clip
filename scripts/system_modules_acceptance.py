#!/usr/bin/env python3
"""Passive module/driver previews on private Weston/Niri; no binding writes."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    module = Path('/sys/module/nvme')
    driver = Path('/sys/bus/pci/drivers/nvme')
    if not module.is_dir() or not driver.is_dir():
        raise RuntimeError('This live acceptance case requires registered NVMe objects; use module fixtures otherwise')
    (root / 'a-module').symlink_to(module, target_is_directory=True)
    (root / 'b-driver').symlink_to(driver, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive module and driver metadata.\n')


def exercise(session, root):
    session.observe('01-module-driver-strips', ['a-module', 'b-driver', 'observed'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-module', 'b-driver', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-module')
    session.click(x + 32, y + 55)
    session.observe('03-module-inspector', ['Module name', 'Module presence', 'Reference count'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-module', 'b-driver'])
    x, y = session.word('04-folder-return', 'b-driver')
    session.click(x + 32, y + 55)
    session.observe('05-driver-inspector', ['Driver name', 'Driver bus', 'Associated module', 'Bound device inventory'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-module', 'b-driver', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive module/driver metadata')


if __name__ == '__main__':
    raise SystemExit(main())
