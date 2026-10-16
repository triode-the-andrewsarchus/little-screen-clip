#!/usr/bin/env python3
"""VFIO/mapper inventories and available IOMMU membership on private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-directory-strips', ['a-vfio', 'b-mapper', 'VFIO access interfaces'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-vfio', 'b-mapper', 'notes.txt'])
    previous = '02-mixed-folder'
    for label, name, expected in [
        ('vfio', 'a-vfio', ['VFIO access interfaces', 'observed IOMMU groups']),
        ('mapper', 'b-mapper', ['Device-mapper interfaces', 'observed mapped block devices']),
    ]:
        x, y = session.word(previous, name)
        session.click(x + 32, y + 55)
        session.observe('inspector-' + label, expected)
        session.key('Escape')
        previous = 'return-' + label
        session.observe(previous, ['a-vfio', 'b-mapper'])
    if (root / 'c-members').exists():
        x, y = session.word(previous, 'c-members')
        session.click(x + 32, y + 55)
        session.observe('iommu-members', ['observed members', 'binding do not', 'prove VM assignment', 'exclusive ownership'])
        session.key('Escape')
        session.observe('final-browser', ['a-vfio', 'b-mapper', 'c-members'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    (root / 'a-vfio').symlink_to('/dev/vfio', target_is_directory=True)
    (root / 'b-mapper').symlink_to('/dev/mapper', target_is_directory=True)
    groups = Path('/sys/kernel/iommu_groups')
    if groups.is_dir():
        selected = None
        try:
            candidates = sorted(groups.iterdir())
        except OSError:
            candidates = []
        for path in candidates:
            try:
                if path.is_dir() and any((path / 'devices').iterdir()):
                    selected = path
                    break
            except OSError:
                continue
        if selected is not None:
            (root / 'c-members').symlink_to(selected, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary text beside passive VFIO and mapper namespace summaries.\n')
    return run_application_acceptance(args, output, root, exercise, 'VFIO and device-mapper directory inventories')


if __name__ == '__main__':
    raise SystemExit(main())
