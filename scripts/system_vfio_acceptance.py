#!/usr/bin/env python3
"""Passive IOMMU membership previews on private Weston/Niri; no VFIO access."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    groups = sorted(Path('/sys/kernel/iommu_groups').iterdir())
    group = next(path for path in groups if (path / 'devices').is_dir() and any((path / 'devices').iterdir()))
    member = next((group / 'devices').iterdir()).resolve()
    (root / 'a-group').symlink_to(group, target_is_directory=True)
    (root / 'b-member').symlink_to(member, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary file beside passive IOMMU membership metadata.\n')


def exercise(session, root):
    session.observe('01-membership-strips', ['a-group', 'b-member', 'IOMMU'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-group', 'b-member', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-group')
    session.click(x + 32, y + 55)
    session.observe('03-group-inspector', ['IOMMU membership', 'IOMMU group number', 'Driver binding'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-group', 'b-member'])
    x, y = session.word('04-folder-return', 'b-member')
    session.click(x + 32, y + 55)
    session.observe('05-pci-inspector', ['PCI', 'IOMMU membership'])
    session.key('Escape')
    session.observe('06-final-folder', ['a-group', 'b-member', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'passive VFIO/IOMMU group metadata')


if __name__ == '__main__':
    raise SystemExit(main())
