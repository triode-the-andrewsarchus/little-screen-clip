#!/usr/bin/env python3
"""Inspect visible cgroup ancestor settings in private Weston/Niri."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def exercise(session, root):
    session.observe('01-cgroup-row', ['a-group', 'Cgroup'])
    session.click(848, 201)
    x, y = session.word('01-cgroup-row', 'a-group')
    session.click(x + 32, y + 55)
    session.observe('02-cgroup-inspector', ['Cgroup resource inspector', 'Configured descendant depth limit'])
    for name, expected in [
        ('03-ancestor-settings', ['Ancestor 1', 'Local CPU bandwidth limit']),
        ('04-delegation-context', ['Cgroup v2 mount options', 'Namespace delegation setting']),
    ]:
        session.scroll_to_visible(name, expected)
    session.key('Escape')
    session.observe('05-folder-return', ['a-group', 'b-notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    group = Path('/sys/fs/cgroup/user.slice/user-1000.slice')
    if not (group / 'cpu.max').is_file():
        raise RuntimeError('An existing user-1000.slice is required; no cgroups are created')
    (root / 'a-group').symlink_to(group, target_is_directory=True)
    (root / 'b-notes.txt').write_text('Ordinary text beside cgroup ancestor metadata.\n')
    return run_application_acceptance(args, output, root, exercise, 'Visible cgroup ancestors and reported delegation configuration')


if __name__ == '__main__':
    raise SystemExit(main())
