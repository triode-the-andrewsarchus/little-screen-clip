#!/usr/bin/env python3
"""Passive DMI/EFI previews on private Weston/Niri; no firmware payloads."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    for name, path in [('a-dmi', '/sys/class/dmi/id'), ('b-efi', '/sys/firmware/efi'), ('c-firmware', '/sys/firmware')]:
        if not Path(path).is_dir():
            raise RuntimeError(f'This live check requires {path}; use boot firmware fixtures otherwise')
        (root / name).symlink_to(path, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive boot firmware metadata.\n')


def exercise(session, root):
    session.observe('01-firmware-strips', ['a-dmi', 'b-efi'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-dmi', 'b-efi', 'c-firmware', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-dmi')
    session.click(x + 32, y + 55)
    session.observe('03-dmi-inspector', ['System manufacturer', 'System product/model', 'BIOS/UEFI vendor', 'BIOS/UEFI version'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-dmi', 'b-efi'])
    x, y = session.word('04-folder-return', 'b-efi')
    session.click(x + 32, y + 55)
    session.observe('05-efi-inspector', ['EFI presence', 'EFI firmware ABI width', 'EFI variable interface', 'EFI runtime-service availability'])
    session.key('Escape')
    session.observe('06-folder-return', ['b-efi', 'c-firmware'])
    x, y = session.word('06-folder-return', 'c-firmware')
    session.click(x + 32, y + 55)
    session.observe('07-firmware-inspector', ['Kernel firmware interfaces', 'DMI/SMBIOS', 'EFI', 'ACPI firmware'])
    session.key('Escape')
    session.observe('08-final-folder', ['a-dmi', 'b-efi', 'c-firmware', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive DMI/EFI firmware metadata')


if __name__ == '__main__':
    raise SystemExit(main())
