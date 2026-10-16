#!/usr/bin/env python3
"""Observe storage strips and inspector navigation on private Weston/Niri.

Uses read-only sysfs metadata from an available disk with partitions. Synthetic
Rust fixtures cover the cases without corresponding local hardware.
"""
from pathlib import Path
import json
import subprocess
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def disk_fixture(root, binary):
    candidates = sorted(Path('/sys/class/block').iterdir())
    for candidate in candidates:
        parts = [child for child in candidate.iterdir() if (child / 'partition').is_file()]
        if len(parts) < 2:
            continue
        output = root / '.storage-observation.json'
        subprocess.run([str(binary.resolve()), '--internal-system-inspector', str(candidate), str(output)], check=True, timeout=3)
        observation = json.loads(output.read_bytes())
        if not observation.get('storage') or len(observation['storage']['partitions']) < 2:
            output.unlink()
            continue
        (root / 'a-disk').symlink_to(candidate, target_is_directory=True)
        (root / 'b-partition').symlink_to(parts[0], target_is_directory=True)
        (root / 'notes.txt').write_text('Ordinary browsing remains available.\n')
        return observation
    raise RuntimeError('No local disk with at least two observed partitions; run synthetic Rust storage tests instead')


def exercise(session, root):
    observation = json.loads((root / '.storage-observation.json').read_bytes())
    session.observe('01-storage-rows', ['a-disk', 'b-partition', observation['storage']['class']])
    session.click(848, 201)
    session.observe('02-storage-painted', ['a-disk', 'b-partition', 'notes.txt'])
    x, y = session.word('02-storage-painted', 'a-disk')
    session.click(x + 32, y + 55)
    session.observe('03-storage-inspector', ['Storage topology', 'Partition inventory', 'Capacity'])
    for _ in range(3):
        private.run_checked([str(session.pointer), 'scroll', '1280', '800', '700', '600', '900'], env=session.env)
    session.observe('04-storage-relationships', ['Related kernel objects', 'Partition'])
    # Escape returns to the same folder without navigating into the sysfs disk.
    session.key('Escape')
    session.observe('05-restored-folder', ['a-disk', 'b-partition', 'notes.txt'])
    x, y = session.word('05-restored-folder', 'b-partition')
    session.click(x + 32, y + 55)
    session.observe('06-partition-inspector', ['Storage topology', 'Partition inventory', 'Partition number'])
    session.key('Escape')
    session.observe('07-final-folder', ['a-disk', 'b-partition'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    disk_fixture(root, args.binary)
    return run_application_acceptance(args, output, root, exercise, 'block storage diagrams and metadata')


if __name__ == '__main__':
    raise SystemExit(main())
