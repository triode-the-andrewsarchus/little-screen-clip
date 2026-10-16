#!/usr/bin/env python3
"""Passive WMI/ACPI/platform previews using private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    wmi = next(path for path in sorted(Path('/sys/bus/wmi/devices').iterdir()) if (path / 'guid').is_file())
    platform = next(path for path in wmi.resolve().parents if (path / 'firmware_node').is_symlink())
    acpi = (platform / 'firmware_node').resolve()
    (root / 'a-wmi').symlink_to(wmi, target_is_directory=True)
    (root / 'b-acpi').symlink_to(acpi, target_is_directory=True)
    (root / 'c-platform').symlink_to(platform, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive cached firmware metadata.\n')


def exercise(session, root):
    session.observe('01-firmware-strips', ['a-wmi', 'b-acpi'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-wmi', 'b-acpi', 'c-platform', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-wmi')
    session.click(x + 32, y + 55)
    session.observe('03-wmi-inspector', ['WMI semantics', 'WMI interface GUID', 'WMI instance count'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-wmi', 'b-acpi'])
    x, y = session.word('04-folder-return', 'b-acpi')
    session.click(x + 32, y + 55)
    session.observe('05-acpi-inspector', ['ACPI hardware ID', 'ACPI namespace path', 'ACPI cached enumeration'])
    session.key('Escape')
    session.observe('06-folder-return', ['b-acpi', 'c-platform'])
    x, y = session.word('06-folder-return', 'c-platform')
    session.click(x + 32, y + 55)
    session.observe('07-platform-inspector', ['Platform', 'Registered driver binding', 'ACPI hardware ID'])
    session.key('Escape')
    session.observe('08-final-folder', ['a-wmi', 'b-acpi', 'c-platform', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive WMI/ACPI/platform metadata')


if __name__ == '__main__':
    raise SystemExit(main())
