#!/usr/bin/env python3
"""Literal-name search and draft command controls in the private compositor."""
from pathlib import Path
import json
import os
from datetime import datetime, timezone
import tomllib
import subprocess

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance


def click_word(session, shot, word):
    session.click(*session.word(shot, word, last=True))


def type_text(session, text):
    session.key('a', 'ctrl')
    # wtype assigns '*' physical keycode 1, which egui falls back to Escape.
    # Paste field contents to exercise real TextEdit input without that fake key.
    subprocess.run(['wl-copy', '--type', 'text/plain'], input=text.encode(),
                   env=session.env, check=True, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, timeout=5)
    session.key('v', 'ctrl')


def exercise(session, root):
    session.observe('initial', ['special', 'Nested'])
    session.key('f', 'ctrl')
    session.observe('search-open', ['Literal', 'Options'])
    click_word(session, 'search-open', 'Literal')
    private.run_checked(['wtype', ':special'], env=session.env)
    session.key('Return')
    session.observe('literal-colon', ['special', '1 matching entries'])
    click_word(session, 'literal-colon', 'Options')
    session.observe('options', ['Build a search command', 'Filters', 'Apply to draft'])
    # Field positions are derived from their labels, within this private runner.
    _, y = session.word('options', 'question', last=True)
    session.click(970, y + 26)
    type_text(session, 'landmark')
    session.observe('options-payload', ['landmark', 'Build a search command'])
    _, y = session.word('options', 'Filters', last=True)
    session.click(970, y + 26)
    type_text(session, '*.txt')
    session.observe('options-filters', ['landmark', 'Build a search command'])
    click_word(session, 'options', 'Subfolders')
    session.observe('options-scope', ['landmark', 'Build a search command'])
    click_word(session, 'options', 'Apply')
    session.observe('draft-not-executed', ['special', 'scope:recursive'])
    session.key('Return')
    session.observe('recursive-result', ['landmark.txt', '1 matching entries', 'Explicit search scope'])
    click_word(session, 'recursive-result', 'Options')
    session.observe('edit-options', ['Filters', 'Apply to draft'])
    _, y = session.word('edit-options', 'Filters', last=True)
    session.click(970, y + 26)
    type_text(session, '10KB..')
    click_word(session, 'edit-options', 'Apply')
    session.observe('invalid-controls', ['landmark.txt'], expected_any=('size', 'range', 'bound'))
    # Closing the popup leaves the last applied result intact.
    session.key('Escape')
    session.observe('last-valid-result', ['landmark.txt', '1 matching entries'])
    assert not (Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite').exists()

    # Show the applied instants, rather than only the input's offset/date text.
    session.address(root)
    for label, command, expected in [
        ('date-offset', ':modified:>=2026-01-01T02:00:00+02:00 special', ['UTC', '00:00:00Z', '1 matching entries']),
        ('date-negated', ':!modified:<2000-01-01T02:00:00+02:00 special', ['exclude UTC', '00:00:00Z', '1 matching entries']),
        ('date-invalid-keeps-bounds', ':modified:2026-02-30', ['exclude UTC', 'special', 'timestamp']),
    ]:
        session.key('f', 'ctrl')
        type_text(session, command)
        session.key('Return')
        session.observe(label, expected)

    session.key('Escape')
    for label, command, expected in [
        ('unknown-family-positive', ':image,*.jpg', ['0 matching entries']),
        ('unknown-family-negative', ':!image,*.jpg', ['0 matching entries']),
        ('unknown-file-negative', ':!file,*.jpg', ['0 matching entries']),
        ('unknown-name-known', ':*.jpg', ['broken.jpg', '1 matching entries']),
    ]:
        session.key('f', 'ctrl')
        type_text(session, command)
        session.key('Return')
        session.observe(label, expected)
    session.key('Escape')

    # Command controls must never make protected locations enrollable. Use only
    # owned private credential/repository fixtures and metadata navigation for
    # real kernel namespaces; clicking Index must create no database or roots.
    for name in ('.ssh', '.git', '.mozilla', '.thunderbird'):
        (root / name).mkdir()
        (root / name / 'secret.txt').write_text('must never be indexed')
    alias = root / 'kernel-alias'
    alias.symlink_to('/proc', target_is_directory=True)
    protected = [Path('/proc'), Path('/sys'), Path('/dev'), root / '.ssh', root / '.git', root / '.mozilla', root / '.thunderbird', alias]
    for number, folder in enumerate(protected):
        session.address(folder)
        expected_entry = 'Folders' if folder in (Path('/proc'), Path('/sys'), Path('/dev'), alias) else 'secret.txt'
        session.observe(f'protected-{number}-before', [expected_entry, 'Clear held files'])
        # Read the address through its real clipboard shortcut; OCR confuses
        # the leading slash in /sys with an uppercase I.
        session.key('l', 'ctrl')
        session.key('a', 'ctrl')
        session.key('c', 'ctrl')
        address = private.run_checked(['wl-paste', '--no-newline'], env=session.env)
        assert address == str(folder.resolve()), (address, str(folder))
        session.key('Escape')
        session.click(993, 118)
        session.observe(f'protected-{number}-after', ['Clear held files'])
        data = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search'
        assert not (data / 'index.sqlite').exists(), str(folder)
        assert not (data / 'runtime').exists(), str(folder)
        assert not (data / 'models').exists(), str(folder)
        config = (session.output / 'scratch/config.toml').read_text()
        assert not tomllib.loads(config).get('file_manager', {}).get('search', {}).get('roots'), str(folder)
    (session.output / 'protected-enrollment.json').write_text(json.dumps({
        'locations': list(map(str, protected)), 'database_created': False,
        'roots_enrolled': [], 'model_installed': False,
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / ':special.txt').write_text('literal colon name')
    (root / 'broken.jpg').symlink_to('missing-target')
    stamp = datetime(2026, 6, 15, tzinfo=timezone.utc).timestamp()
    os.utime(root / ':special.txt', (stamp, stamp))
    (root / 'Nested').mkdir()
    (root / 'Nested/landmark.txt').write_text('a nested document')
    (root / 'Nested/landmark.md').write_text('a same-name file that the extension filter must omit')
    return run_application_acceptance(args, output, root, exercise,
        'Leading-colon literal filename, structured filter/scope controls editing only the visible draft, invalid options retain results, resolved UTC date bounds/negation survive invalid drafts, protected namespaces/private/repository paths and a kernel alias cannot enroll, no model or database')


if __name__ == '__main__':
    raise SystemExit(main())
