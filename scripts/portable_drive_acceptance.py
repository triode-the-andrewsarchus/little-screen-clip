#!/usr/bin/env python3
"""Exercise portable-drive controls and real backend jobs in the private compositor."""
from pathlib import Path
import json
import os
import socket
import sqlite3
import csv
import io
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_search_controls_acceptance import type_text
import ui_acceptance_capture as private


def request(env, payload):
    path = Path(env['XDG_RUNTIME_DIR']) / 'little-screen-clip.sock'
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(5)
        stream.connect(str(path))
        stream.sendall(json.dumps({'Drives': payload}).encode() + b'\n')
        line = stream.makefile('rb').readline(4 * 1024 * 1024)
    reply = json.loads(line)
    if 'Error' in reply:
        raise RuntimeError(reply['Error']['error'])
    return reply['Drives']


def status(env):
    return request(env, {'Status': {'offset': 0}})['Status']


def completed(env, reply):
    operation = reply['Accepted']
    result = {}
    def ready():
        for item in status(env)['operations']:
            if item['id'] == operation and item['finished'] is not None:
                result.update(item)
                return True
        return False
    wait_for(ready, 'retained drive operation completion')
    assert result['state'] == 'complete', result
    return result


def right_click(session, shot, word, *, tree=False, last=False):
    x, y = session.word(shot, word, tree=tree, last=last)
    private.run_checked([str(session.pointer), 'right-click', *map(str, session.logical_size), str(x + 5), str(y)], env=session.env)


def click_word(session, shot, word, *, tree=False):
    position = session.word(shot, word, tree=tree, required=False) or session.word(shot, word, tree=not tree, required=False)
    if position is None:
        data = private.run_checked(['tesseract', str(session.output / (shot + '-ocr.png')), 'stdout', '--psm', '11', '-c', 'tessedit_create_tsv=1', '-c', 'tessedit_create_txt=0'], env=session.env)
        for item in csv.DictReader(io.StringIO(data), delimiter='\t', quoting=csv.QUOTE_NONE):
            x = int(item['left']) // 3
            if (x < 330) == tree and item['text'].strip().rstrip('.…:;').lower() == word.lower():
                position = (x + 5, (int(item['top']) + int(item['height']) // 2) // 3)
                break
    if position is None:
        # Rounded small buttons can disappear in full-window segmentation.
        for bounds in [(105, 270, 225, 325), (105, 320, 240, 380)]:
            crop = session.output / 'drive-button.png'
            with Image.open(session.output / (shot + '.png')) as screen:
                region = screen.crop(bounds)
                region.resize((region.width * 6, region.height * 6)).save(crop)
            data = private.run_checked(['tesseract', str(crop), 'stdout', '--psm', '6', '-c', 'tessedit_create_tsv=1', '-c', 'tessedit_create_txt=0'], env=session.env)
            for item in csv.DictReader(io.StringIO(data), delimiter='\t', quoting=csv.QUOTE_NONE):
                if item['text'].strip().lower() == word.lower():
                    position = (bounds[0] + int(item['left']) // 6 + 5, bounds[1] + (int(item['top']) + int(item['height']) // 2) // 6)
                    break
            if position:
                break
    assert position, (shot, word)
    session.click(*position)


def drive_menu(session, label):
    shot = label + '-ready'
    session.observe(shot, ['Portable test drive'])
    ram = session.word(shot, 'RAM:', tree=True, required=False)
    if ram:
        session.scroll(270, ram[1], 35)
        session.observe(shot, ['Portable test drive'])
    right_click(session, shot, 'Portable', tree=True, last=True)
    session.observe(label, ['Back up partition', 'Keep in sync', 'Contents catalog'])


def exercise(session, root):
    env = session.env
    session.ocr_regions = [('drive-confirm-ocr.png', (105, 270, 225, 325)), ('drive-dialog-buttons-ocr.png', (105, 230, 305, 380))]
    portable, local, backups = root / 'portable', root / 'local', root / 'backups'
    try:
        initial = session.observe('initial', ['Portable test drive', 'portable', 'local'])
        assert 'Choose backup directory' not in initial
        assert status(env)['backup_directory'] is None
        drive_menu(session, 'drive-menu')
        session.observe('ram-action', ['Copy to RAM'])
        click_word(session, 'ram-action', 'RAM', tree=True)
        wait_for(lambda: any(item['ready'] for item in status(env)['ram']), 'RAM mirror readiness')
        ram = next(item for item in status(env)['ram'] if item['ready'])
        ram_path = Path(ram['path'])
        assert (ram_path / 'readme.txt').read_text() == 'original portable contents\n'
        (ram_path / 'readme.txt').write_text('online RAM edit\n')
        wait_for(lambda: (portable / 'readme.txt').read_text() == 'online RAM edit\n', 'prompt RAM writeback')

        session.address(local)
        session.observe('local-folder', ['local-only.txt'])
        drive_menu(session, 'sync-menu')
        click_word(session, 'sync-menu', 'Keep', tree=True)
        session.observe('sync-actions', ['Pair with a folder'], preserve_pointer=True)
        click_word(session, 'sync-actions', 'Pair')
        session.observe('sync-dialog', ['Both folders', 'Use current folder'])
        click_word(session, 'sync-dialog', 'Use')
        session.observe('sync-destination', ['Use current folder'])
        click_word(session, 'sync-destination', 'Continue')
        wait_for(lambda: (portable / 'local-only.txt').exists() and (local / 'readme.txt').exists(), 'initial union through UI enrollment')

        drive_menu(session, 'backup-menu')
        click_word(session, 'backup-menu', 'Back', tree=True)
        session.observe('backup-actions', ['As files', 'As partition image'], preserve_pointer=True)
        click_word(session, 'backup-actions', 'As')
        session.observe('backup-directory', ['Choose backup directory', 'Use current folder'])
        x, y = session.word('backup-directory', 'Use', tree=True)
        session.click(x + 70, y - 27)
        type_text(session, str(backups))
        click_word(session, 'backup-directory', 'Continue')
        wait_for(lambda: bool(status(env)['backups']), 'completed file backup through lazy directory prompt')
        saved = status(env)['backups'][0]
        assert (Path(saved['path']) / 'readme.txt').read_text() == 'online RAM edit\n'
        assert Path(status(env)['backup_directory']) == backups

        session.address(portable)
        session.observe('portable-folder', ['many', 'readme.txt'])
        right_click(session, 'portable-folder', 'many')
        session.observe('folder-menu', ['Catalog / indexing'])
        click_word(session, 'folder-menu', 'Catalog')
        session.observe('folder-policy', ['Include metadata', 'Allow explicitly configured'])
        click_word(session, 'folder-policy', 'Include')
        click_word(session, 'folder-policy', 'Allow')
        click_word(session, 'folder-policy', 'Save')
        wait_for(lambda: any(rule['relative'] == 'many' and not rule['catalog'] and not rule['index'] for rule in status(env)['rules']), 'independent subtree policy')
        drive_menu(session, 'catalog-menu')
        click_word(session, 'catalog-menu', 'Contents', tree=True)
        session.observe('catalog-actions', ['Save / update'], preserve_pointer=True)
        click_word(session, 'catalog-actions', 'Save')
        wait_for(lambda: bool(status(env)['catalogs']), 'saved contents catalog')
        catalog = status(env)['catalogs'][0]
        with sqlite3.connect(Path(env['LITTLE_CLIP_DRIVE_STATE_DIR']) / 'catalogs' / (catalog['id'] + '.sqlite')) as db:
            paths = [bytes(row[0]) for row in db.execute("SELECT path FROM entries WHERE scan='catalog'")]
        assert b'many' in paths and not any(path.startswith(b'many/') for path in paths)
        assert (Path(saved['path']) / 'many/hidden.txt').exists(), 'No-index must not omit backup payloads'

        portable.rename(root / 'removed-portable')
        wait_for(lambda: not status(env)['drives'], 'removal observation')
        assert ram_path.is_dir()
        (ram_path / 'readme.txt').write_text('offline RAM edit\n')
        completed(env, request(env, {'MountCatalog': {'catalog': catalog['id']}}))
        catalog = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
        mount = Path(catalog['mount'])
        assert (mount / 'readme.txt').read_text() == 'online RAM edit\n'
        session.address(mount)
        session.observe('offline-catalog', ['readme.txt', 'many', 'Catalog'])

        (root / 'removed-portable/readme.txt').write_text('offline portable edit\n')
        (root / 'removed-portable').rename(portable)
        wait_for(lambda: bool(status(env)['conflicts']), 'reconnection conflict preservation')
        conflicts = status(env)['conflicts']
        for conflict in conflicts:
            if conflict['pair'] == ram['id']:
                for folder in [portable, ram_path]:
                    assert (folder / conflict['portable']).read_text() == 'offline portable edit\n'
                    assert (folder / conflict['local']).read_text() == 'offline RAM edit\n'
        session.observe('reconnected', ['Drive sync conflict'])
        click_word(session, 'reconnected', 'Dismiss')
        session.observe('conflict-jobs', ['Jobs'])
        click_word(session, 'conflict-jobs', 'Jobs', tree=True)
        session.observe('conflict-review', ['Drive operations and conflicts', 'Conflict', 'preserved'])
        click_word(session, 'conflict-review', 'Close')
        assert not (Path(env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite').exists()
    finally:
        try:
            snapshot = status(env)
            for item in snapshot['catalogs']:
                if item['mount']:
                    completed(env, request(env, {'UnmountCatalog': {'catalog': item['id']}}))
            for item in snapshot['ram']:
                completed(env, request(env, {'DiscardRam': {'session': item['id'], 'confirmed': True}}))
        except Exception as error:
            (session.output / 'drive-cleanup-error.txt').write_text(str(error))


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name in ['portable', 'local', 'backups']:
        (root / name).mkdir()
    (root / 'portable/readme.txt').write_text('original portable contents\n')
    (root / 'portable/many').mkdir()
    (root / 'portable/many/hidden.txt').write_text('Back up and sync this; exclude catalog/index traversal.\n')
    (root / 'local/local-only.txt').write_text('Union merge keeps this file.\n')
    def prepare(env, children):
        state = output / 'drive-state'
        env['LITTLE_CLIP_DRIVE_STATE_DIR'] = str(state)
        env['LITTLE_CLIP_DRIVE_MOUNT_HELPER'] = str(repository / 'target/debug/clip-drive-mount')
        children.start('drive-fixture', [str(repository / 'target/debug/examples/drive_fixture'), str(state), str(root / 'portable')], env=env, cwd=output, log_path=output / 'drive-fixture.log')
        wait_for(lambda: (Path(env['XDG_RUNTIME_DIR']) / 'little-screen-clip.sock').exists(), 'owned drive fixture IPC')
    return run_application_acceptance(args, output, root, exercise,
        'Drive context menus, RAM writeback, union enrollment, lazy backups, subtree exclusions, offline FUSE and preserved conflicts', prepare_environment=prepare)


if __name__ == '__main__':
    raise SystemExit(main())
