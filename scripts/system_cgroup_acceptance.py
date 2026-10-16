#!/usr/bin/env python3
"""Read-only cgroup v2 previews in the private Weston/Niri runner."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def fixtures(root):
    group = Path('/sys/fs/cgroup/user.slice')
    if not (group / 'cpu.max').is_file():
        raise RuntimeError('Acceptance requires an existing v2 user.slice; no cgroups are created')
    (root / 'a-service').symlink_to(group, target_is_directory=True)
    (root / 'b-bandwidth').symlink_to(group / 'cpu.max')
    (root / 'c-reclaim').symlink_to(group / 'memory.reclaim')
    (root / 'notes.txt').write_text('Ordinary text alongside resource metadata.\n')


def exercise(session, root):
    session.observe('01-cgroup-strips', ['a-service', 'b-bandwidth', 'Cgroup'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-service', 'b-bandwidth', 'c-reclaim', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-service')
    session.click(x + 32, y + 55)
    session.observe('03-resource-inspector', ['Cgroup resource inspector', 'Hierarchy position', 'Available controllers', 'Local CPU bandwidth limit'])
    session.scroll_to_visible('03a-memory-limits', ['Memory usage including descendants', 'Local hard memory limit', 'Current task count including descendants'])
    session.scroll_to_visible('03b-accounting-membership', ['Accumulated CPU accounting', 'Observed process membership', 'Observed thread membership'])
    session.scroll_to_visible('03c-hierarchy-links', ['Parent cgroup', '/sys/fs/cgroup', 'Child cgroup'])
    session.key('Escape')
    session.observe('04-browser-return', ['a-service', 'b-bandwidth', 'c-reclaim'])
    x, y = session.word('04-browser-return', 'b-bandwidth')
    session.click(x + 32, y + 55)
    session.observe('05-attribute-inspector', ['Cgroup resource inspector', 'Local CPU bandwidth limit', 'No local bandwidth limit'])
    session.key('Escape')
    session.observe('06-browser-return', ['a-service', 'b-bandwidth', 'c-reclaim'])
    x, y = session.word('06-browser-return', 'c-reclaim')
    session.click(x + 32, y + 55)
    session.observe('07-reclaim-exclusion', ['Cgroup resource inspector', 'Unknown cgroup interface', 'payload not read'])
    session.key('Escape')
    session.observe('08-folder-return', ['a-service', 'b-bandwidth', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'Read-only cgroup resource metadata and excluded reclaim interface')


if __name__ == '__main__':
    raise SystemExit(main())
