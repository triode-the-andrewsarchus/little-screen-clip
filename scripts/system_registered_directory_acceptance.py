#!/usr/bin/env python3
"""GPU, sound and input namespace grouping in the private Weston/Niri runner."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-directory-strips', ['a-gpus', 'b-sound', 'GPU access interfaces'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-gpus', 'b-sound', 'c-input'])
    previous = '02-mixed-folder'
    for label, name, heading, quantity in [
        ('gpu', 'a-gpus', 'GPU access interfaces', 'observed connected outputs'),
        ('sound', 'b-sound', 'Sound-card interfaces', 'observed sound cards'),
        ('input', 'c-input', 'Input-device interfaces', 'observed input-device groups'),
    ]:
        x, y = session.word(previous, name)
        session.click(x + 32, y + 55)
        session.observe('inspector-' + label, [heading, quantity, 'observed interfaces'])
        session.key('Escape')
        previous = 'return-' + label
        session.observe(previous, ['a-gpus', 'b-sound', 'c-input'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    for name, target in [('a-gpus', '/dev/dri'), ('b-sound', '/dev/snd'), ('c-input', '/dev/input')]:
        (root / name).symlink_to(target, target_is_directory=True)
    (root / 'notes.txt').write_text('Ordinary text beside grouped read-only device interfaces.\n')
    return run_application_acceptance(args, output, root, exercise, 'GPU, sound and input directory grouping')


if __name__ == '__main__':
    raise SystemExit(main())
