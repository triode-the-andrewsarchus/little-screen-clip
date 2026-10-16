#!/usr/bin/env python3
"""Kernel security/tracing metadata, using only private Weston/Niri input."""
from pathlib import Path
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, private


def fixtures(root):
    for name, target in [('a-security', '/sys/kernel/security'), ('b-lockdown', '/sys/kernel/security/lockdown'), ('c-modules', '/sys/kernel/security/lsm'), ('d-tracing', '/sys/kernel/tracing'), ('e-debug', '/sys/kernel/debug')]:
        path = Path(target)
        (root / name).symlink_to(path, target_is_directory=path.is_dir())
    (root / 'notes.txt').write_text('Ordinary text alongside passive kernel metadata.\n')


def exercise(session, root):
    session.observe('01-security-strips', ['a-security', 'd-tracing', 'Security subsystem state'])
    session.click(848, 201)
    session.observe('02-mixed-directories', ['a-security', 'd-tracing', 'e-debug'])
    x, y = session.word('02-mixed-directories', 'a-security')
    session.click(x + 32, y + 55)
    session.observe('03-security-inspector', ['Security subsystem state', 'complete security posture', 'Kernel-reported security modules', 'Kernel lockdown selection'])
    session.key('Escape')
    session.observe('04-folder-return', ['a-security', 'd-tracing', 'e-debug'])
    x, y = session.word('04-folder-return', 'd-tracing')
    session.click(x + 32, y + 55)
    session.observe('05-tracing-inspector', ['Tracing facilities', 'Ring-buffer recording', 'Permission denied'])
    session.key('Escape')
    session.observe('06-folder-return', ['a-security', 'd-tracing', 'e-debug'])
    x, y = session.word('06-folder-return', 'e-debug')
    session.click(x + 32, y + 55)
    session.observe('07-debug-inspector', ['Debug interfaces', 'metadata only', 'Interface directory inventory'])
    session.key('Escape')
    session.observe('08-folder-return', ['a-security', 'd-tracing', 'e-debug'])
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '1050', '600', '400'], env=session.env)
    private.run_checked([str(session.pointer), 'scroll', '1280', '800', '1050', '600', '400'], env=session.env)
    session.observe('09-mixed-attributes', ['b-lockdown', 'c-modules', 'notes.txt'])
    x, y = session.word('09-mixed-attributes', 'b-lockdown')
    session.click(x + 32, y + 55)
    session.observe('10-lockdown-attribute', ['Security subsystem state', 'Kernel lockdown selection', 'available'])
    session.key('Escape')
    session.observe('11-folder-return', ['b-lockdown', 'c-modules', 'notes.txt'])
    x, y = session.word('11-folder-return', 'c-modules')
    session.click(x + 32, y + 55)
    session.observe('12-modules-attribute', ['Security subsystem state', 'Kernel-reported security modules'])
    session.key('Escape')
    session.observe('13-final-folder', ['b-lockdown', 'c-modules', 'notes.txt'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    fixtures(root)
    return run_application_acceptance(args, output, root, exercise, 'Limited security state and unreadable tracing/debug configuration')


if __name__ == '__main__':
    raise SystemExit(main())
