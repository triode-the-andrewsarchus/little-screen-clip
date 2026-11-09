#!/usr/bin/env python3
"""Explicit enrollment and Index colors across a private same-filesystem bind mount."""
from pathlib import Path
import argparse
import os
import sqlite3
import subprocess
import sys

from PIL import Image
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment
from file_manager_search_acceptance import submit, pause_indexing


def indicator(session, label, expected):
    screenshot = session.output / f'{label}.png'
    def matches():
        session.capture_text(label, screenshot, [])
        with Image.open(screenshot) as image:
            data = image.convert('RGB').crop((967, 106, 1020, 131)).tobytes()
            pixels = zip(data[0::3], data[1::3], data[2::3])
            return sum(all(abs(a - b) < 8 for a, b in zip(pixel, expected)) for pixel in pixels) >= 4
    wait_for(matches, f'{label}: Index indicator RGB {expected}')


def inventory(index, count):
    def ready():
        if not index.exists():
            return False
        with sqlite3.connect(index) as db:
            roots = db.execute('SELECT state FROM roots').fetchall()
            paths = db.execute('SELECT path FROM paths').fetchall()
            return len(roots) == count and all(state == 'ready' for state, in roots) and len(paths) == count
    wait_for(ready, f'{count} explicitly enrolled ready roots')
    with sqlite3.connect(index) as db:
        return {os.fsdecode(path) for path, in db.execute('SELECT path FROM paths')}


def exercise(session, root):
    session.observe('initial', ['local.txt', 'mounted'])
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    indicator(session, 'parent-red', (225, 80, 80))
    pause_indexing(session, True, 'pause-before-enrollment')
    assert not index.exists(), 'Saving preferences alone must not enroll a root'
    session.click(993, 118)
    session.key('Escape')
    indicator(session, 'parent-yellow-paused', (225, 180, 55))
    pause_indexing(session, False, 'resume-enrolled-root')
    assert inventory(index, 1) == {str(root / 'local.txt')}
    indicator(session, 'parent-green', (70, 185, 95))
    submit(session, ':content:"mount-boundary-canary";scope:recursive')
    session.observe('nested-content-excluded', ['0 matching entries'])
    session.key('Escape')
    session.address(root / 'mounted')
    session.observe('nested-browsed', ['canary.txt'])
    indicator(session, 'nested-red', (225, 80, 80))
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM roots').fetchone()[0] == 1
        assert db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == 1
    session.click(993, 118)
    session.key('Escape')
    assert inventory(index, 2) == {str(root / 'local.txt'), str(root / 'mounted/canary.txt')}
    indicator(session, 'nested-green', (70, 185, 95))
    submit(session, ':content:"mount-boundary-canary";scope:indexed')
    session.observe('explicit-nested-searchable', ['canary.txt', '1 matching entries'])
    session.click(993, 118)
    session.observe('remove-nested-menu', ['Remove indexed folder'])
    session.click(*session.word('remove-nested-menu', 'Remove', last=True))
    text = session.observe('removed-retains-browser', ['canary.txt', 'Clear held files'])
    assert 'explicit search scope' not in text.lower(), 'Removing a root must retire indexed search results'
    indicator(session, 'removed-nested-red', (225, 80, 80))
    assert inventory(index, 1) == {str(root / 'local.txt')}
    # Rebind the very same source (same device/inode, new mount) after explicit
    # removal. Re-enrollment must release the previous runtime mount authority.
    subprocess.run(['umount', '--lazy', str(root / 'mounted')], check=True)
    subprocess.run(['mount', '--bind', str(session.output / 'outside'), str(root / 'mounted')], check=True)
    session.key('F5')
    session.observe('rebound-after-removal', ['canary.txt'])
    indicator(session, 'rebound-red', (225, 80, 80))
    session.click(993, 118)
    session.key('Escape')
    assert inventory(index, 2) == {str(root / 'local.txt'), str(root / 'mounted/canary.txt')}
    indicator(session, 'rebound-green', (70, 185, 95))


def main():
    repo = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--namespace-child', action='store_true', help=argparse.SUPPRESS)
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager', configure)
    if not args.namespace_child:
        return subprocess.run(['unshare', '--user', '--map-root-user', '--mount', '--propagation', 'private',
            '--', sys.executable, str(Path(__file__).resolve()), *sys.argv[1:], '--namespace-child']).returncode
    output, root = media_fixture_directory(args)
    (root / 'local.txt').write_text('Ordinary local file')
    (root / 'mounted').mkdir()
    outside = output / 'outside'
    outside.mkdir()
    (outside / 'canary.txt').write_text('mount-boundary-canary')
    assert outside.stat().st_dev == root.stat().st_dev
    subprocess.run(['mount', '--bind', str(outside), str(root / 'mounted')], check=True)
    return run_application_acceptance(args, output, root, exercise,
        'Private same-filesystem bind mount excluded from enclosing index; red/green indicator follows actual mount; separate explicit enrollment enables guarded content search', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
