#!/usr/bin/env python3
"""Observe shared process/PTY relationships in a private Weston/Niri session.

The fixture owns fresh PTYs; it never injects input into an existing terminal.
"""
from pathlib import Path
import fcntl
import json
import os
import pty
import subprocess
import sys
import termios
import time
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def terminal(root, name, close_terminal=False):
    master, slave = pty.openpty()
    path = os.ttyname(slave)
    def session():
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    code = 'import os, time; ' + ('os.close(0); os.close(1); os.close(2); ' if close_terminal else '') + 'time.sleep(180)'
    child = subprocess.Popen([sys.executable, '-c', code], stdin=slave, stdout=slave, stderr=slave, preexec_fn=session)
    os.close(slave)
    (root / name).symlink_to(path)
    return child, master, path


def exercise(session, root):
    session.observe('01-terminal-rows', ['a-terminal', 'b-terminal'])
    session.click(848, 201)
    session.observe('02-mixed-folder', ['a-terminal', 'b-terminal', 'notes.txt'])
    x, y = session.word('02-mixed-folder', 'a-terminal')
    session.click(x + 32, y + 55)
    session.observe('03-terminal-inspector', ['Possible terminal relationships', 'foreground group', 'Candidate only', 'leader'])
    for _ in range(16):
        private.run_checked([str(session.pointer), 'scroll', '1280', '800', '700', '600', '220'], env=session.env)
        time.sleep(0.2)
        text = session.capture_text('04-process-relationships', session.output / '04-process-relationships.png', ['observed open descriptor'])
        if 'observedopendescriptor' in ''.join(text.lower().split()):
            break
    session.observe('04-process-relationships', ['observed open descriptor'])
    # The provenance tooltip can obscure the link's trailing words. Its leading
    # fixture process name remains visible in the inspector's left column.
    x, y = session.word('04-process-relationships', 'python3', last=True, tree=True)
    session.click(x, y)
    session.observe('05-process-inspector', ['Process identity', 'Process start tick', 'Parent PID'])
    session.click(166, 172)
    session.observe('06-inspector-back', ['Possible terminal relationships', 'Candidate only', 'leader'])
    session.key('Escape')
    session.observe('07-browser-return', ['a-terminal', 'b-terminal', 'notes.txt'])
    x, y = session.word('07-browser-return', 'c-closed-terminal')
    session.click(x + 32, y + 55)
    session.observe('08-closed-terminal-candidate', ['Possible terminal relationships', 'devpts instance unverified'])
    session.key('Escape')
    session.observe('09-final-browser', ['a-terminal', 'b-terminal', 'c-closed-terminal'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, root = media_fixture_directory(args)
    owned = []
    try:
        for name in ['a-terminal', 'b-terminal']:
            owned.append(terminal(root, name))
        owned.append(terminal(root, 'c-closed-terminal', close_terminal=True))
        (root / 'notes.txt').write_text('Ordinary file beside two owned terminal fixtures.\n')
        (output / 'terminal-fixtures.json').write_text(json.dumps([{'pid': child.pid, 'path': path} for child, _, path in owned], indent=2))
        return run_application_acceptance(args, output, root, exercise, 'shared process and terminal relationships')
    finally:
        for child, master, _ in owned:
            child.terminate()
            child.wait(timeout=5)
            os.close(master)


if __name__ == '__main__':
    raise SystemExit(main())
