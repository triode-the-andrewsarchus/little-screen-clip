#!/usr/bin/env python3
"""Exercise ordinary shared-memory files and protected reference drops privately."""
from pathlib import Path
import json
import re
import shlex
import subprocess
import tempfile
import time
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_desktop_acceptance import drag
import ui_acceptance_capture as private


def exercise(session, root):
    session.observe('01-shared-memory', ['DROPBOX', 'a-note.txt', 'b-kernel'])
    session.click(848, 201)
    session.observe('02-ready', ['DROPBOX', 'a-note.txt', 'b-kernel', 'c-device'])
    x, y = session.word('02-ready', 'a-note.txt')
    session.click(x + 20, y)
    session.key('Return')
    session.observe('03-ordinary-text', ['Shared memory remains ordinary', 'Edit'])
    session.key('Escape')
    session.observe('04-text-return', ['DROPBOX', 'a-note.txt', 'b-kernel'])
    source = session.word('04-text-return', 'a-note.txt')
    destination = session.word('04-text-return', 'DROPBOX')
    drag(session, source, destination, copy=True)
    copied = root / 'DROPBOX/a-note.txt'
    wait_for(copied.exists, 'ordinary shared-memory native copy')
    assert copied.read_bytes() == (root / 'a-note.txt').read_bytes() == b'Shared memory remains ordinary.\n'
    session.observe('05-copied', ['1 completed', '0 failed', 'a-note.txt', 'b-kernel'])
    x, y = session.word('05-copied', 'a-note.txt')
    session.click(x + 20, y)
    x, y = session.word('05-copied', 'b-kernel')
    modifier = subprocess.Popen(['wtype', '-M', 'ctrl', '-s', '1200', '-m', 'ctrl'], env=session.env)
    try:
        time.sleep(.15)
        session.click(x + 20, y)
    finally:
        modifier.wait(timeout=3)
    session.observe('06-mixed-selection', ['2 selected', 'a-note.txt', 'b-kernel'])
    source = session.word('06-mixed-selection', 'a-note.txt')
    destination = session.word('06-mixed-selection', 'DROPBOX')
    # Preserve both selected references: the ordinary drag helper starts a new selection.
    private.run_checked([str(session.pointer), 'drag', '1280', '800', *map(str, (*source, *destination))], env=session.env)
    session.observe('07-drop-result', ['0 completed', '1 failed'])
    x, y = session.word('07-drop-result', 'Move:', tree=True, minimum_y=450)
    session.click(x + 20, y)
    session.observe('07-mixed-rejected', ['0 completed', '1 failed', 'Safety admission', 'inspection-only'])
    x, y = session.word('07-mixed-rejected', 'Move:', tree=True, minimum_y=450)
    session.click(x + 20, y)
    assert sorted(path.name for path in (root / 'DROPBOX').iterdir()) == ['a-note.txt']
    assert (root / 'a-note.txt').read_bytes() == copied.read_bytes()
    assert (root / 'b-kernel').readlink() == Path('/proc/uptime')
    assert (root / 'c-device').readlink() == Path('/dev/zero')
    # Copying a kernel interface publishes references, never its bytes.
    session.address(root)
    session.observe('08-reference-source', ['b-kernel', 'c-device'])
    x, y = session.word('08-reference-source', 'b-kernel')
    session.click(x + 20, y)
    session.key('c', 'ctrl')
    payload = private.run_checked(['wl-paste', '--no-newline', '--type', 'text/uri-list'], env=session.env)
    assert (root / 'b-kernel').as_uri() in payload, payload
    (session.output / 'reference-uri.txt').write_text(payload)
    session.observe('09-reference-only', ['b-kernel', 'c-device'])


def main():
    args = application_arguments(__doc__, Path('target/release/clip-file-manager'))
    output, _ = media_fixture_directory(args)
    trace = output / 'files.trace'
    wrapper = output / 'traced-files'
    # Keep the app PID unchanged so the private runner can verify window ownership.
    wrapper.write_text('#!/bin/sh\nexec strace -D -f -yy -e trace=openat,read,write,ioctl -o '
                       + shlex.quote(str(trace)) + ' ' + shlex.quote(str(args.binary.resolve())) + ' "$@"\n')
    wrapper.chmod(0o700)
    args.binary = wrapper
    with tempfile.TemporaryDirectory(prefix='veylar-system-drop-', dir='/dev/shm') as shared:
        root = Path(shared)
        (root / 'DROPBOX').mkdir()
        (root / 'a-note.txt').write_text('Shared memory remains ordinary.\n')
        (root / 'b-kernel').symlink_to('/proc/uptime')
        (root / 'c-device').symlink_to('/dev/zero')
        result = run_application_acceptance(
            args, output, root, exercise, 'shared-memory content and protected native drops',
            prepare_environment=lambda env, _: env.update(WAYLAND_DEBUG='client'))
    # Assert actual content I/O absence, including aliases resolved in FD annotations.
    forbidden = [line for line in trace.read_text().splitlines()
                 if re.search(r'(?:read|write|ioctl)\(\d+<(?:/proc/uptime|/dev/zero)(?:<[^>]+>)?>', line)
                 or ('openat(' in line and any(path in line for path in ('/proc/uptime', '/dev/zero'))
                     and 'O_PATH' not in line)]
    (output / 'forbidden-io.json').write_text(json.dumps(forbidden, indent=2))
    report_path = output / 'report.json'
    report = json.loads(report_path.read_text())
    protocol = (output / 'files.log').read_text()
    native = {'offer': 'offer("text/uri-list")' in protocol,
              'receive': 'receive("text/uri-list"' in protocol}
    report.update(forbidden_content_io=len(forbidden), native_uri_protocol=native,
                  content_io_scope=['/proc/uptime', '/dev/zero'])
    if forbidden or (result == 0 and not all(native.values())):
        report.update(status='failed', error='Forbidden content I/O or missing native URI protocol evidence')
        result = 1
    report_path.write_text(json.dumps(report, indent=2))
    assert not forbidden, forbidden[:10]
    return result


if __name__ == '__main__':
    raise SystemExit(main())
