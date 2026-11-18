#!/usr/bin/env python3
"""Exercise portable-drive controls and real backend jobs in the private compositor."""
from pathlib import Path
import json
import os
import socket
import sqlite3
import csv
import io
from PIL import Image, ImageOps

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


def completed(env, reply, expected_state='complete'):
    operation = reply['Accepted']
    result = {}
    def ready():
        for item in status(env)['operations']:
            if item['id'] == operation and item['finished'] is not None:
                result.update(item)
                return True
        return False
    wait_for(ready, 'retained drive operation completion')
    assert result['state'] == expected_state, result
    return result


def right_click(session, shot, word, *, tree=False, last=False):
    point = session.word(shot, word, tree=tree, last=last, required=False)
    if point is None and word.endswith(':'):
        point = session.word(shot, word[:-1], tree=tree, last=last, required=False)
    if point is None:
        raise RuntimeError(f'{shot}: no right-click target {word!r}')
    x, y = point
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
        for bounds in [(114, 332, 193, 359), (105, 270, 225, 325), (105, 320, 240, 400), (105, 390, 300, 670), (820, 505, 888, 520), (1030, 440, 1184, 526)]:
            crop = session.output / 'drive-button.png'
            with Image.open(session.output / (shot + '.png')) as screen:
                region = screen.crop(bounds)
                region.resize((region.width * 6, region.height * 6)).save(crop)
            data = private.run_checked(['tesseract', str(crop), 'stdout', '--psm', '6', '-c', 'tessedit_create_tsv=1', '-c', 'tessedit_create_txt=0'], env=session.env)
            for item in csv.DictReader(io.StringIO(data), delimiter='\t', quoting=csv.QUOTE_NONE):
                if item['text'].strip().strip('()[]{}|').rstrip('.…:;').lower() == word.lower():
                    position = (bounds[0] + int(item['left']) // 6 + 5, bounds[1] + (int(item['top']) + int(item['height']) // 2) // 6)
                    break
            if position:
                break
    assert position, (shot, word)
    print(f'{shot}: clicking {word!r} at {position[0] + 8},{position[1]}', flush=True)
    session.click(position[0] + 8, position[1])


def drive_menu(session, label):
    shot = label + '-ready'
    session.observe(shot, ['Portable test drive'])
    ram = session.word(shot, 'RAM:', tree=True, required=False)
    if ram:
        session.scroll(270, ram[1], 35)
        session.observe(shot, ['Portable test drive'])
    right_click(session, shot, 'Portable', tree=True, last=True)
    session.observe(label, ['Back up partition', 'Keep in sync', 'Contents catalog'])


def caption(session, shot, bounds):
    label = session.output / (shot + '-caption-' + str(bounds[1]) + '.png')
    with Image.open(session.output / (shot + '.png')) as screen:
        interior = screen.crop(bounds)
        interior.resize((interior.width * 6, interior.height * 6)).save(label)
    rendered = private.run_checked(['tesseract', str(label), 'stdout', '--psm', '7'], env=session.env)
    return ''.join(character for character in rendered.lower() if character.isalpha())



def assert_address(session, shot, expected):
    # Read only the address field: a filename can also appear in a child preview.
    target = session.output / (shot + '-address.png')
    with Image.open(session.output / (shot + '.png')) as screen:
        crop = screen.crop((230, 95, 959, 140)).convert('L')
        if crop.getpixel((5, 5)) < 128:
            crop = ImageOps.invert(crop)
        crop = ImageOps.expand(crop, border=15, fill=255)
        crop.resize((crop.width * 3, crop.height * 3)).save(target)
    observed = private.run_checked(['tesseract', str(target), 'stdout', '--psm', '7'], env=session.env)
    actual = ''.join(observed.split()).strip('|[]')
    assert actual == ''.join(str(expected).split()), (shot, actual, expected)
    return actual


def click_status_action(session, shot, label):
    # Private fixture geometry, verified against the actual rendered interior.
    bounds = {'Retry': (1041, 448, 1093, 464), 'Dismiss': (1109, 448, 1171, 464)}[label]
    assert caption(session, shot, bounds) == label.lower(), (shot, label)
    session.click((bounds[0] + bounds[2]) // 2, (bounds[1] + bounds[3]) // 2)


def cancel_review(session, shot):
    # Verify the bordered button's interior instead of accepting its frame as text.
    point = session.word(shot, 'Start', tree=True, required=False) or session.word(shot, 'Start', required=False)
    if point is None:
        # Full-window OCR can omit small bordered buttons. The private fixture's
        # settings region retains them; verify Cancel below before clicking.
        bounds = (100, 145, 660, 690)
        controls = session.output / (shot + '-backup-controls.png')
        with Image.open(session.output / (shot + '.png')) as screen:
            region = screen.crop(bounds)
            region.resize((region.width * 3, region.height * 3)).save(controls)
        data = private.run_checked(['tesseract', str(controls), 'stdout', '--psm', '6', '-c', 'tessedit_create_tsv=1', '-c', 'tessedit_create_txt=0'], env=session.env)
        for item in csv.DictReader(io.StringIO(data), delimiter='\t', quoting=csv.QUOTE_NONE):
            if item['text'].strip().strip('|[]').lower() == 'start':
                point = (bounds[0] + int(item['left']) // 3, bounds[1] + (int(item['top']) + int(item['height']) // 2) // 3)
                break
    if point is None:
        raise RuntimeError(f'{shot}: no verified Start button')
    x, y = point
    assert caption(session, shot, (x - 3, y + 43, x + 54, y + 68)) == 'cancel'
    session.click(x + 20, y + 54)
    session.observe(shot + '-closed', ['Mounts'])


def dismiss_completion_notice(session, shot, title):
    session.observe(shot, [title])
    point = session.word(shot, 'Dismiss', required=False) or session.word(shot, 'Dismiss', tree=True, required=False)
    if point is None:
        # The status dock height follows its message. Its action stays at the
        # right, aligned with the first rendered line rather than a fixed y.
        anchor = title.split()[0]
        _, y = session.word(shot, anchor, required=False, last=True) or session.word(shot, anchor, tree=True, last=True)
        assert caption(session, shot, (1108, y - 12, 1171, y + 12)) == 'dismiss'
        session.click(1140, y)
    else:
        session.click(point[0] + 8, point[1])
    session.observe(shot + '-dismissed', ['Mounts'])


def backup_review(session, backups):
    env = session.env
    session.ocr_regions = [('drive-backup-menu-ocr.png', (306, 450, 475, 775))] + getattr(session, 'ocr_regions', [])
    drive_menu(session, 'backup-menu')
    click_word(session, 'backup-menu', 'Back', tree=True)
    session.observe('backup-actions', ['As files', 'As partition image'], preserve_pointer=True)
    click_word(session, 'backup-actions', 'As')
    session.observe('backup-directory', ['Choose backup directory', 'Use current folder'])
    x, y = session.word('backup-directory', 'Use', tree=True)
    session.click(x + 70, y - 27)
    type_text(session, str(backups))
    click_word(session, 'backup-directory', 'Continue')
    session.observe('backup-review-cancel', ['Review partition backup', 'Available', 'Start backup'])
    assert status(env)['backup_directory'] is None
    assert not status(env)['backups']
    click_word(session, 'backup-review-cancel', 'Temporarily')
    session.observe('backup-review-read-only', ['Close open files', 'original access mode is restored', 'Start backup'])
    cancel_review(session, 'backup-review-read-only')
    assert status(env)['backup_directory'] is None
    assert not status(env)['backups']
    drive_menu(session, 'backup-retry-menu')
    click_word(session, 'backup-retry-menu', 'Back', tree=True)
    session.observe('backup-retry-actions', ['As files', 'As partition image'], preserve_pointer=True)
    click_word(session, 'backup-retry-actions', 'As')
    session.observe('backup-retry-directory', ['Choose backup directory', 'Use current folder'])
    x, y = session.word('backup-retry-directory', 'Use', tree=True)
    session.click(x + 70, y - 27)
    type_text(session, str(backups))
    click_word(session, 'backup-retry-directory', 'Continue')
    identity = ['No partition UUID', 'filesystem UUID', 'Saved identity', 'Backup folder'] if status(env)['drives'][0]['part_uuid'] is None else []
    session.observe('backup-review-start', ['Review partition backup', 'Available', 'Start backup'] + identity)
    click_word(session, 'backup-review-start', 'Start')
    wait_for(lambda: bool(status(env)['backups']), 'completed file backup through lazy directory prompt')
    wait_for(lambda: any(job['title'] == 'Back up files' and job['finished'] is not None and job['state'] == 'complete' for job in status(env)['operations']), 'terminal file backup completion')
    wait_for(lambda: status(env)['backup_directory'] == str(backups), 'published backup directory preference')
    dismiss_completion_notice(session, 'backup-completion-notice', 'Back up files')
    saved = status(env)['backups'][0]
    assert Path(status(env)['backup_directory']) == backups

    drive_menu(session, 'image-review-menu')
    click_word(session, 'image-review-menu', 'Back', tree=True)
    session.observe('image-review-actions', ['As files', 'As partition image'], preserve_pointer=True)
    x, y = session.word('image-review-actions', 'As', required=False) or session.word('image-review-actions', 'As', tree=True)
    # Full-window OCR merges the adjacent menus. Verify this fixture's image action
    # separately; its capacity label and mount checkbox occupy the intervening rows.
    assert caption(session, 'image-review-actions', (x - 1, y + 128, x + 143, y + 156)) == 'aspartitionimage'
    session.click(x + 50, y + 142)
    session.observe('image-review-cancel', ['Full independent image estimate', 'Mount again after imaging', 'Start backup'])
    assert len(status(env)['backups']) == 1
    cancel_review(session, 'image-review-cancel')
    assert len(status(env)['backups']) == 1

    return saved



def exercise_fallback_identity(session, root, restart_fixture):
    env = session.env
    portable, local, backups = root / 'portable', root / 'local', root / 'backups'
    session.observe('fallback-initial', ['Portable test drive', 'portable', 'local'])
    observed = status(env)
    assert observed['drives'][0]['part_uuid'] is None
    assert observed['drives'][0]['id'] == 'fs:fixture'
    assert not observed['pairs'] and not observed['backups']
    assert observed['backup_directory'] is None
    session.address(local)
    drive_menu(session, 'fallback-sync-menu')
    click_word(session, 'fallback-sync-menu', 'Keep', tree=True)
    session.observe('fallback-sync-actions', ['Pair with a folder'], preserve_pointer=True)
    click_word(session, 'fallback-sync-actions', 'Pair')
    session.observe('fallback-sync-dialog', ['No partition UUID', 'filesystem UUID', 'Saved identity', 'Both folders', 'Use current folder'])
    click_word(session, 'fallback-sync-dialog', 'Use')
    click_word(session, 'fallback-sync-dialog', 'Continue')
    wait_for(lambda: (portable / 'local-only.txt').exists() and (local / 'readme.txt').exists(), 'explicit fallback enrollment union')
    pair = status(env)['pairs'][0]
    assert pair['drive'] == 'fs:fixture'
    dismiss_completion_notice(session, 'fallback-sync-completion', 'Enroll folder sync')
    saved = backup_review(session, backups)
    bucket = backups / 'fs-fixture'
    assert Path(saved['path']).parent == bucket
    assert not (backups / 'portable-test').exists()
    drive_menu(session, 'fallback-browse-menu')
    click_word(session, 'fallback-browse-menu', 'Back', tree=True)
    session.observe('fallback-browse-actions', ['Browse saved'], preserve_pointer=True)
    click_word(session, 'fallback-browse-actions', 'Browse')
    session.observe('fallback-backup-folder', ['current-files.json'])
    assert_address(session, 'fallback-backup-folder', bucket)
    (root / 'portable').rename(root / 'removed-portable')
    wait_for(lambda: not status(env)['drives'], 'fallback removal')
    (local / 'offline-fallback.txt').write_text('saved by filesystem identity')
    (root / 'removed-portable').rename(root / 'portable')
    wait_for(lambda: (portable / 'offline-fallback.txt').exists(), 'fallback reconnect without re-enrollment')
    restart_fixture()
    restored = status(env)
    assert restored['pairs'][0]['id'] == pair['id']
    assert restored['pairs'][0]['drive'] == pair['drive']
    assert restored['backup_directory'] == str(backups)
    completed(env, request(env, {'Backup': {'drive': pair['drive'], 'kind': 'Files', 'restore_mounted': False}}))
    versions = status(env)['backups']
    assert len(versions) == 2
    assert all(Path(version['path']).parent == bucket and version['drive'] == pair['drive'] for version in versions)
    assert (Path(saved['path']) / 'readme.txt').read_text() == 'original portable contents\n'
    session.observe('fallback-after-restart', ['current-files.json', 'Portable test drive'])
    assert_address(session, 'fallback-after-restart', bucket)


def exercise_backup_review(session, root):
    session.observe('initial', ['Portable test drive', 'portable', 'local'])
    saved = backup_review(session, root / 'backups')
    assert (Path(saved['path']) / 'readme.txt').read_text() == 'original portable contents\n'
    assert not status(session.env)['ram']
    assert not status(session.env)['catalogs']


def exercise_ram_pending(session, root):
    env = session.env
    portable = root / 'portable'
    ram = None
    try:
        drive_menu(session, 'ram-menu')
        session.observe('ram-action', ['Copy to RAM'])
        click_word(session, 'ram-action', 'RAM', tree=True)
        wait_for(lambda: any(item['ready'] for item in status(env)['ram']), 'RAM readiness')
        dismiss_completion_notice(session, 'ram-completion', 'Copy to RAM')
        ram = status(env)['ram'][0]
        ram_path = Path(ram['path'])
        (ram_path / 'readme.txt').write_text('online RAM edit\n')
        wait_for(lambda: (portable / 'readme.txt').read_text() == 'online RAM edit\n', 'online writeback')
        wait_for(lambda: status(env)['ram'][0]['pending'] == {'paths': 0, 'full_check_required': False}, 'acknowledged online edits')
        session.observe('ram-online', ['RAM:', 'pending changes observed'])
        right_click(session, 'ram-online', 'RAM:', tree=True)
        session.observe('ram-online-status', ['Last confirmed drive sync', 'Flush pending changes', 'Save RAM copy elsewhere'])
        session.click(1050, 210)

        portable.rename(root / 'removed-portable')
        wait_for(lambda: not status(env)['ram'][0]['connected'], 'offline status')
        (ram_path / 'readme.txt').write_text('offline RAM edit\n')
        wait_for(lambda: status(env)['ram'][0]['pending']['paths'] >= 1, 'durable observed offline changes')
        before = status(env)['ram'][0]
        session.observe('ram-offline', ['offline', 'pending'])
        right_click(session, 'ram-offline', 'RAM:', tree=True)
        session.observe('ram-offline-status', ['Last confirmed drive sync', 'Flush pending changes', 'Save RAM copy elsewhere'])
        session.click(1050, 210)
        completed(env, request(env, {'SaveRam': {'session': ram['id'], 'destination': str(root / 'saved')}}))
        after = status(env)['ram'][0]
        assert after['last_flushed'] == before['last_flushed']
        assert after['pending']['paths'] >= 1
        assert (Path(after['last_saved']['path']) / 'readme.txt').read_text() == 'offline RAM edit\n'
        dismiss_completion_notice(session, 'ram-export-completion', 'Save RAM copy')
        session.observe('ram-exported-offline', ['offline', 'pending'])
        right_click(session, 'ram-exported-offline', 'RAM:', tree=True)
        session.observe('ram-exported-status', ['Last confirmed drive sync', 'Save RAM copy elsewhere'])
        session.click(1050, 210)

        (root / 'removed-portable').rename(portable)
        wait_for(lambda: (portable / 'readme.txt').read_text() == 'offline RAM edit\n', 'reconnected writeback')
        wait_for(lambda: status(env)['ram'][0]['pending'] == {'paths': 0, 'full_check_required': False}, 'reconnected acknowledgement')
        assert status(env)['ram'][0]['last_flushed'] >= before['last_flushed']
        session.observe('ram-reconnected', ['pending changes observed'])
        right_click(session, 'ram-reconnected', 'RAM:', tree=True)
        session.observe('ram-reconnected-status', ['Last confirmed drive sync', 'Flush pending changes'])
        session.click(1050, 210)
    finally:
        if ram:
            completed(env, request(env, {'DiscardRam': {'session': ram['id'], 'confirmed': True}}))


def exercise(session, root):
    env = session.env
    session.ocr_regions = [('drive-confirm-ocr.png', (105, 270, 225, 325)), ('drive-dialog-buttons-ocr.png', (105, 230, 305, 380)), ('drive-catalog-menu-ocr.png', (305, 670, 475, 770))]
    portable, local, backups = root / 'portable', root / 'local', root / 'backups'
    try:
        initial = session.observe('initial', ['Portable test drive', 'portable', 'local'])
        assert 'Choose backup directory' not in initial
        assert status(env)['backup_directory'] is None
        drive_menu(session, 'drive-menu')
        session.observe('ram-action', ['Copy to RAM'])
        click_word(session, 'ram-action', 'RAM', tree=True)
        wait_for(lambda: any(item['ready'] for item in status(env)['ram']), 'RAM mirror readiness')
        dismiss_completion_notice(session, 'ram-completion-notice', 'Copy to RAM')
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
        wait_for(lambda: all(pair['last_success'] is not None for pair in status(env)['pairs']), 'completed initial reconciliation')
        dismiss_completion_notice(session, 'sync-completion-notice', 'Enroll folder sync')
        session.observe('union-complete', ['local-only.txt', 'readme.txt', 'many'])

        saved = backup_review(session, backups)
        assert (Path(saved['path']) / 'readme.txt').read_text() == 'online RAM edit\n'

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
        dismiss_completion_notice(session, 'policy-completion-notice', 'Drive settings')
        drive_menu(session, 'catalog-menu')
        click_word(session, 'catalog-menu', 'Contents', tree=True)
        session.observe('catalog-actions', ['Save / update'], preserve_pointer=True)
        click_word(session, 'catalog-actions', 'update')
        session.observe('catalog-review', ['Review contents catalog', 'Maximum entries', 'Start catalog'])
        click_word(session, 'catalog-review', 'Start')
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
        session.observe('offline-catalog', ['readme.txt', 'many', 'Catalog', 'Not catalogued'])
        session.address(mount / 'many')
        session.observe('excluded-catalog-folder', ['Cannot open', 'Not catalogued', 'excluded from the saved catalog'])
        session.address(mount)
        session.observe('catalog-after-excluded-navigation', ['readme.txt', 'many', 'Not catalogued'])

        (root / 'removed-portable/readme.txt').write_text('offline portable edit\n')
        (root / 'removed-portable').rename(portable)
        wait_for(lambda: bool(status(env)['conflicts']), 'reconnection conflict preservation')
        conflicts = status(env)['conflicts']
        for conflict in conflicts:
            if conflict['pair'] == ram['id']:
                for folder in [portable, ram_path]:
                    assert (folder / conflict['portable']).read_text() == 'offline portable edit\n'
                    assert (folder / conflict['local']).read_text() == 'offline RAM edit\n'
        dismiss_completion_notice(session, 'reconnected', 'Drive sync conflict')
        session.observe('conflict-jobs', ['Jobs'])
        click_word(session, 'conflict-jobs', 'Jobs', tree=True)
        session.observe('conflict-review', ['Drive operations and conflicts', 'Conflict', 'preserved'])
        session.key('Escape')
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


def mount_catalog_offline(env, root, catalog):
    (root / 'portable').rename(root / 'removed-portable')
    wait_for(lambda: not status(env)['drives'], 'offline catalog source')
    completed(env, request(env, {'MountCatalog': {'catalog': catalog['id']}}))
    current = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
    return Path(current['mount'])


def exercise_catalog_boundary(session, root):
    """Verify exclusion presentation without repeating unrelated storage workflows."""
    env = session.env
    drive = status(env)['drives'][0]['id']
    completed(env, request(env, {'Rule': {'drive': drive, 'relative': 'many', 'catalog': False, 'index': False}}))
    completed(env, request(env, {'Catalog': {'drive': drive}}))
    catalog = status(env)['catalogs'][0]
    mount = mount_catalog_offline(env, root, catalog)
    try:
        session.address(mount)
        text = session.observe('offline-catalog', ['readme.txt', 'many', 'empty', 'Not catalogued'])
        assert 'File changed since enumeration' not in text
        assert list((mount / 'empty').iterdir()) == []
        session.address(mount / 'many')
        session.observe('excluded-catalog-folder', ['Cannot open', 'Not catalogued', 'excluded from the saved catalog'])
        session.address(mount)
        session.observe('catalog-after-excluded-navigation', ['readme.txt', 'many', 'Not catalogued'])
    finally:
        completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}))


def exercise_catalog_coverage(session, root):
    """An incomplete offline folder must disclose its coverage instead of emptiness."""
    env = session.env
    drive = status(env)['drives'][0]['id']
    config = status(env)['config']
    config['catalog_max_depth'] = 1
    completed(env, request(env, {'Configure': {'config': config}}))
    completed(env, request(env, {'Catalog': {'drive': drive}}))
    catalog = status(env)['catalogs'][0]
    assert not catalog['complete']
    assert catalog['details']['directory']['listing_complete']
    mount = mount_catalog_offline(env, root, catalog)
    try:
        session.address(mount)
        session.observe('partial-catalog-root', ['readme.txt', 'many', 'Partial catalog'])
        session.address(mount / 'many')
        session.observe('incomplete-catalog-folder', ['Cannot open', 'Partial catalog', 'not fully inventoried'])
        click_status_action(session, 'incomplete-catalog-folder', 'Retry')
        session.observe('incomplete-catalog-retry', ['Cannot open', 'Partial catalog'])
        click_status_action(session, 'incomplete-catalog-retry', 'Dismiss')
        session.observe('incomplete-catalog-dismissed', ['readme.txt', 'many', 'Partial catalog'])
        session.address(mount)
        session.observe('partial-catalog-return', ['readme.txt', 'many', 'Partial catalog'])
    finally:
        completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}))


def exercise_catalog_types(session, root):
    """Saved special files stay inert and visible after the source is removed."""
    env = session.env
    drive = status(env)['drives'][0]['id']
    completed(env, request(env, {'Catalog': {'drive': drive}}))
    catalog = status(env)['catalogs'][0]
    assert catalog['complete'], catalog['coverage']
    mount = mount_catalog_offline(env, root, catalog)
    try:
        session.address(mount)
        text = session.observe('catalog-inert-file-types', ['pipe.ts', 'socket', 'FIFO metadata', 'Socket metadata', 'readme.txt'])
        assert 'Partial catalog' not in text
        session.address(mount / 'root-link')
        session.observe('catalog-root-link', ['pipe.ts', 'socket', 'FIFO metadata', 'Socket metadata'])
        session.address(mount / 'pipe.ts')
        session.observe('catalog-pipe-navigation-refused', ['Cannot open', 'pipe.ts', 'FIFO metadata'])
        session.address(mount)
        session.observe('catalog-types-return', ['pipe.ts', 'socket', 'FIFO metadata', 'Socket metadata'])
    finally:
        completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}))


def catalog_context_menu(session, name, expected, *, mounted=True):
    # Status notices and ancestor inventories move Mounts after navigation.
    # Refresh and retry only this read-only menu opening if the row moved.
    target = name + '-target'
    def visible():
        text = session.capture_text(name, session.output / (name + '.png'), [])
        action = 'opensavedcatalog' if mounted else 'mountsavedcatalog'
        normalized = ''.join(text.lower().split())
        if action not in normalized or (not mounted and 'unmountsavedcatalog' in normalized):
            return False
        session.observe(name, expected)
        return True
    def opened():
        if visible():
            return True
        session.observe(target, ['Catalog: Portable test drive'])
        right_click(session, target, 'Catalog:', tree=True)
        return visible()
    wait_for(opened, 'catalog context menu after sidebar movement')


def seed_old_policy_rules(env):
    observed = status(env)['drives'][0]
    drive = observed['id']
    cli = Path(__file__).resolve().parent.parent / 'target/debug/clipctl'
    for relative, catalog, index in [('', False, True), ('many//', True, False)]:
        arguments = [str(cli), 'drives', 'policy', observed['device']]
        if relative:
            arguments.append(relative)
        arguments += ['--catalog', str(catalog).lower(), '--index', str(index).lower()]
        completed(env, json.loads(private.run_checked(arguments, env=env)))
        saved = request(env, {'GetRule': {'drive': drive, 'relative': relative}})['Rule']
        assert saved['drive'] == drive and saved['relative'] == relative.rstrip('/'), saved
        assert (saved['catalog'], saved['index']) == (catalog, index), saved
    for number in range(105):
        completed(env, request(env, {'Rule': {'drive': drive, 'relative': f'old-policy-{number}', 'catalog': True, 'index': True}}))
    assert all(rule['relative'] not in ['', 'many'] for rule in status(env)['rules'])


def save_policy_unchanged(session, name, relative, expected):
    env = session.env
    before = {operation['id'] for operation in status(env)['operations']}
    click_word(session, name, 'Save')
    def saved():
        finished = [operation for operation in status(env)['operations'] if operation['id'] not in before and operation['finished'] is not None]
        if not finished:
            return False
        assert all(operation['state'] == 'complete' for operation in finished), finished
        return True
    wait_for(saved, 'unchanged policy save from the actual editor')
    drive = status(env)['drives'][0]['id']
    rule = request(env, {'GetRule': {'drive': drive, 'relative': relative}})['Rule']
    assert (rule['catalog'], rule['index']) == expected, rule
    session.observe(name + '-notice', ['Drive settings'])
    dismiss_completion_notice(session, name + '-notice', 'Drive settings')


def exercise_policy_editor(session, root):
    """Open saved policies older than the first status page and preserve both flags."""
    session.address(root / 'portable')
    session.observe('policy-editor-source', ['many', 'readme.txt'])
    drive_menu(session, 'policy-drive-menu')
    click_word(session, 'policy-drive-menu', 'Contents', tree=True)
    session.observe('policy-catalog-menu', ['indexing settings'])
    click_word(session, 'policy-catalog-menu', 'settings')
    session.observe('policy-old-root', ['Catalog and indexing policy', 'Include metadata', 'Allow explicitly configured'])
    save_policy_unchanged(session, 'policy-old-root', '', (False, True))
    session.observe('policy-folder-source', ['many', 'readme.txt'])
    right_click(session, 'policy-folder-source', 'many')
    session.observe('policy-folder-actions', ['Catalog / indexing'])
    click_word(session, 'policy-folder-actions', 'Catalog')
    session.observe('policy-old-folder', ['Catalog and indexing policy', 'Include metadata', 'Allow explicitly configured'])
    save_policy_unchanged(session, 'policy-old-folder', 'many', (True, False))


def exercise_catalog_redaction(session, root):
    """Keep a real FUSE snapshot open while excluding and compacting its metadata."""
    # Exclude the popup border and surrounding list from paragraph OCR.
    session.ocr_regions = [('catalog-menu-interior-ocr.png', (126, 415, 530, 640))]
    env = session.env
    drive = status(env)['drives'][0]['id']
    completed(env, request(env, {'BackupDirectory': {'path': str(root / 'backups')}}))
    completed(env, request(env, {'Backup': {'drive': drive, 'kind': 'Files', 'restore_mounted': False}}))
    backup = status(env)['backups'][0]
    completed(env, request(env, {'Catalog': {'drive': drive}}))
    catalog = status(env)['catalogs'][0]
    completed(env, request(env, {'MountCatalog': {'catalog': catalog['id']}}))
    mount = Path(next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])['mount'])
    held = (mount / 'many/hidden.txt').open('rb')
    try:
        session.address(root / 'portable')
        session.observe('catalog-source', ['many', 'readme.txt'])
        right_click(session, 'catalog-source', 'many')
        session.observe('catalog-folder-actions', ['Catalog / indexing'])
        click_word(session, 'catalog-folder-actions', 'Catalog')
        session.observe('catalog-redaction-policy', ['Include metadata', 'removes saved descendant catalog records', 'redaction date'])
        click_word(session, 'catalog-redaction-policy', 'Include')
        click_word(session, 'catalog-redaction-policy', 'Allow')
        click_word(session, 'catalog-redaction-policy', 'Save')
        wait_for(lambda: any(item['pending_redactions'] for item in status(env)['catalogs']), 'durable pending redaction')
        session.observe('catalog-policy-notice', ['Catalog exclusion saved'])
        assert (mount / 'many/hidden.txt').stat().st_size > 0
        catalog_context_menu(session, 'catalog-pending-menu', ['Captured', 'Metadata cleanup pending', 'Unmount saved catalog'])
        click_word(session, 'catalog-pending-menu', 'Open', tree=True)
        session.address(root / 'portable')
        session.observe('catalog-before-release', ['many', 'readme.txt'])
        # A still-open FUSE file keeps the old snapshot busy, without losing its bytes.
        completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}), expected_state='failed')
        assert held.read() == (root / 'portable/many/hidden.txt').read_bytes()
        assert next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])['pending_redactions'] > 0
        held.close()
        completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}))
        wait_for(lambda: any(item['id'] == catalog['id'] and item['pending_redactions'] == 0 and item['redacted'] is not None for item in status(env)['catalogs']), 'redaction after snapshot release')
        updated = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
        assert updated['captured'] == catalog['captured']
        with sqlite3.connect(Path(env['LITTLE_CLIP_DRIVE_STATE_DIR']) / 'catalogs' / (catalog['id'] + '.sqlite')) as db:
            assert db.execute("SELECT count(*) FROM entries WHERE scan='catalog' AND path=?", (b'many/hidden.txt',)).fetchone()[0] == 0
        assert (root / 'portable/many/hidden.txt').is_file()
        assert (Path(backup['path']) / 'many/hidden.txt').is_file()
        completed(env, request(env, {'MountCatalog': {'catalog': catalog['id']}}))
        session.address(mount)
        session.observe('catalog-redacted-view', ['many', 'Not catalogued', 'readme.txt'])
        session.address(root / 'portable')
        session.observe('catalog-redacted-source', ['many', 'readme.txt', 'Catalog: Portable test drive'])
        catalog_context_menu(session, 'catalog-redacted-menu', ['Captured', 'Redacted', 'descendant records removed'])
        click_word(session, 'catalog-redacted-menu', 'Open', tree=True)
    finally:
        held.close()
        session.address(root / 'portable')
        session.observe('catalog-cleanup-source', ['many', 'readme.txt'])
        if any(item['id'] == catalog['id'] and item['mount'] for item in status(env)['catalogs']):
            completed(env, request(env, {'UnmountCatalog': {'catalog': catalog['id']}}))
    exercise_catalog_alias_pending(session, root, catalog)


def exercise_catalog_alias_pending(session, root, catalog):
    """Unknown offline spellings retain their rows and disclose why cleanup waits."""
    env = session.env
    source = root / 'portable'
    offline = root / 'offline-portable'
    source.rename(offline)
    try:
        session.address(root)
        wait_for(lambda: not status(env)['drives'], 'offline source for uncertain catalog alias')
        before = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
        completed(env, request(env, {'Rule': {'drive': catalog['drive'], 'relative': 'MANY', 'catalog': False, 'index': True}}))
        def failed_cleanup():
            current = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
            return current['pending_redactions'] > 0 and 'cleanup remains pending' in (current['redaction_error'] or '')
        wait_for(failed_cleanup, 'explicit uncertain-alias cleanup error')
        current = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
        assert current['entries'] == before['entries'] and current['captured'] == catalog['captured'], current
        catalog_context_menu(session, 'catalog-alias-pending', ['Metadata cleanup pending', 'original source is unavailable or changed'], mounted=False)
        text = (session.output / 'catalog-alias-pending.txt').read_text().lower()
        assert 'unmount this catalog' not in text, text
        session.key('Escape')
        completed(env, request(env, {'Rule': {'drive': catalog['drive'], 'relative': '', 'catalog': False, 'index': True}}))
        wait_for(lambda: any(item['id'] == catalog['id'] and item['root_excluded'] and item['pending_redactions'] == 0 for item in status(env)['catalogs']), 'explicit offline root cleanup including uncertain intents')
        current = next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])
        assert current['entries'] == 0 and current['captured'] == catalog['captured'], current
        catalog_context_menu(session, 'catalog-alias-cleared', ['0 entries', 'Redacted'], mounted=False)
        text = (session.output / 'catalog-alias-cleared.txt').read_text().lower()
        assert 'metadata cleanup pending' not in text, text
        session.key('Escape')
        assert (offline / 'many/hidden.txt').read_text() == 'Back up and sync this; exclude catalog/index traversal.\n'
    finally:
        offline.rename(source)
        session.address(source)
        session.observe('catalog-alias-source-restored', ['many', 'readme.txt'])


def exercise_storage(session, root):
    """Measure required sync metadata through Jobs with catalogs/indexing excluded."""
    env = session.env
    initial = session.observe('storage-initial', ['Portable test drive', 'Jobs'])
    assert 'Choose backup directory' not in initial
    assert not any(item['title'] == 'Measure local metadata' for item in status(env)['operations'])
    completed(env, request(env, {'Rule': {'drive': 'part:portable-test', 'relative': '', 'catalog': False, 'index': False}}))
    dismiss_completion_notice(session, 'storage-policy-notice', 'Drive settings')
    completed(env, request(env, {'Pair': {'drive': 'part:portable-test', 'relative': '', 'local': str(root / 'local')}}))
    dismiss_completion_notice(session, 'storage-pair-notice', 'Enroll folder sync')
    assert (root / 'local/many/hidden.txt').read_bytes() == (root / 'portable/many/hidden.txt').read_bytes()
    assert not status(env)['catalogs']
    session.observe('storage-jobs-ready', ['Jobs'])
    click_word(session, 'storage-jobs-ready', 'Jobs', tree=True)
    session.observe('storage-before-measure', ['Drive operations and conflicts', 'Measure local metadata', 'operational metadata'])
    assert not any(item['title'] == 'Measure local metadata' for item in status(env)['operations'])
    click_word(session, 'storage-before-measure', 'Measure')
    result = {}
    def measured():
        for item in status(env)['operations']:
            if item['title'] == 'Measure local metadata' and item['finished'] is not None:
                result.update(item)
                return True
        return False
    wait_for(measured, 'explicit local metadata measurement')
    assert result['state'] == 'complete', result
    assert 'Sync baselines: 4 paths' in result['message'], result
    assert 'Saved catalogs (optional): 0 snapshots' in result['message'], result
    session.observe('storage-report', ['Sync baselines', 'Saved catalogs', 'Finished', 'Registry'])
    assert status(env)['backup_directory'] is None
    assert not (Path(env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite').exists()
    session.key('Escape')

    cli = Path(__file__).resolve().parent.parent / 'target/debug/clipctl'
    reply = json.loads(private.run_checked([str(cli), 'drives', 'storage'], env=env))
    cli_result = completed(env, reply)
    assert cli_result['id'] != result['id']
    assert 'Sync baselines: 4 paths' in cli_result['message'], cli_result


def exercise_catalog_limits(session, root):
    """Review, cancel, reset and explicitly expand this run's bounded capture."""
    env = session.env
    config = status(env)['config']
    config.update(catalog_max_entries=2, catalog_max_bytes=1024 * 1024,
                  catalog_max_depth=8, catalog_timeout_seconds=60, catalog_workers=3)
    completed(env, request(env, {'Configure': {'config': config}}))

    dismiss_completion_notice(session, 'catalog-limits-setup-notice', 'Drive settings')

    def review(name):
        for attempt in range(3):
            # Existing catalog descriptions make this submenu taller and can
            # reposition it above its parent. Retry only opening the review;
            # no capture is submitted until the separately verified Start action.
            if attempt:
                session.click(650, 73)
            drive_menu(session, name + '-menu')
            click_word(session, name + '-menu', 'Contents', tree=True)
            session.observe(name + '-actions', ['Save / update'], preserve_pointer=True)
            # "saved" in the separate mount action can be OCR'd as "Save".
            # The update token identifies only the intended review action.
            click_word(session, name + '-actions', 'update')
            text = session.capture_text(name, session.output / (name + '.png'), ['Review contents catalog'])
            if 'reviewcontentscatalog' in ''.join(text.lower().split()):
                session.observe(name, ['Review contents catalog', 'Maximum entries', 'Database bytes',
                                       'Maximum depth', 'Time limit', '3 independent catalog', 'Start catalog'])
                return
        raise RuntimeError('Could not open the verified catalog review')

    review('catalog-limits-cancel')
    click_word(session, 'catalog-limits-cancel', 'larger')
    session.observe('catalog-limits-expanded-cancel', ['capture only', 'Start catalog'])
    cancel_review(session, 'catalog-limits-expanded-cancel')
    assert not status(env)['catalogs']
    assert status(env)['config'] == config

    review('catalog-limits-reset')
    click_word(session, 'catalog-limits-reset', 'larger')
    session.observe('catalog-limits-reset-expanded', ['Use configured limits'])
    click_word(session, 'catalog-limits-reset-expanded', 'configured')
    session.observe('catalog-limits-reset-ready', ['Start catalog'])
    click_word(session, 'catalog-limits-reset-ready', 'Start')
    wait_for(lambda: bool(status(env)['catalogs']), 'limited capture after resetting review')
    limited = status(env)['catalogs'][0]
    assert not limited['complete'] and limited['entries'] <= 2, limited
    assert limited['limits'] == dict(entries=2, bytes=1024 * 1024, depth=8, timeout_seconds=60)
    dismiss_completion_notice(session, 'catalog-limits-limited-notice', 'Save contents catalog')

    review('catalog-limits-start')
    click_word(session, 'catalog-limits-start', 'larger')
    session.observe('catalog-limits-expanded-ready', ['Start catalog'])
    click_word(session, 'catalog-limits-expanded-ready', 'Start')
    wait_for(lambda: len(status(env)['catalogs']) == 2, 'explicit larger metadata capture')
    expanded = next(item for item in status(env)['catalogs'] if item['id'] != limited['id'])
    assert expanded['complete'], expanded
    assert expanded['limits'] == dict(entries=8, bytes=4 * 1024 * 1024, depth=16, timeout_seconds=180)
    assert status(env)['config'] == config
    assert all(item['mount'] is None for item in status(env)['catalogs'])
    assert (root / 'portable/many/hidden.txt').is_file()
    session.observe('catalog-limits-completed', ['Catalog:', 'Portable test drive'])


def main():
    repository = Path(__file__).resolve().parent.parent
    def configure(parser):
        scenarios = parser.add_mutually_exclusive_group()
        scenarios.add_argument('--backup-review-only', action='store_true')
        scenarios.add_argument('--fallback-identity-only', action='store_true')
        scenarios.add_argument('--catalog-boundary-only', action='store_true')
        scenarios.add_argument('--catalog-coverage-only', action='store_true')
        scenarios.add_argument('--catalog-types-only', action='store_true')
        scenarios.add_argument('--catalog-redaction-only', action='store_true')
        scenarios.add_argument('--policy-editor-only', action='store_true')
        scenarios.add_argument('--ram-pending-only', action='store_true')
        scenarios.add_argument('--catalog-limits-only', action='store_true')
        scenarios.add_argument('--storage-only', action='store_true')
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager', configure=configure)
    output, root = media_fixture_directory(args)
    for name in ['portable', 'local', 'backups']:
        (root / name).mkdir()
    (root / 'portable/readme.txt').write_text('original portable contents\n')
    (root / 'portable/many').mkdir()
    (root / 'portable/many/hidden.txt').write_text('Back up and sync this; exclude catalog/index traversal.\n')
    if args.catalog_boundary_only:
        (root / 'portable/empty').mkdir()
    if args.catalog_types_only:
        os.mkfifo(root / 'portable/pipe.ts')
        with socket.socket(socket.AF_UNIX) as listener:
            listener.bind(str(root / 'portable/socket'))
        (root / 'portable/root-link').symlink_to('.')
    (root / 'local/local-only.txt').write_text('Union merge keeps this file.\n')
    fixture = {}
    def prepare(env, children):
        state = output / 'drive-state'
        env['LITTLE_CLIP_DRIVE_STATE_DIR'] = str(state)
        env['LITTLE_CLIP_DRIVE_MOUNT_HELPER'] = str(repository / 'target/debug/clip-drive-mount')
        arguments = [str(repository / 'target/debug/examples/drive_fixture'), str(state), str(root / 'portable')]
        if args.fallback_identity_only:
            arguments.append('--filesystem-identity')
        fixture['process'] = children.start('drive-fixture', arguments, env=env, cwd=output, log_path=output / 'drive-fixture.log')
        def restart():
            old = fixture['process']
            old.terminate()
            assert old.wait(timeout=20) == 0
            endpoint = Path(env['XDG_RUNTIME_DIR']) / 'little-screen-clip.sock'
            endpoint.unlink()
            fixture['process'] = children.start('drive-fixture', arguments, env=env, cwd=output, log_path=output / 'drive-fixture-restarted.log')
            wait_for(endpoint.exists, 'restarted owned drive fixture IPC')
        fixture['restart'] = restart
        wait_for(lambda: (Path(env['XDG_RUNTIME_DIR']) / 'little-screen-clip.sock').exists(), 'owned drive fixture IPC')
        if args.policy_editor_only:
            seed_old_policy_rules(env)
    if args.fallback_identity_only:
        scenario = lambda session, root: exercise_fallback_identity(session, root, fixture['restart'])
        scope = 'Filesystem UUID fallback disclosure, explicit pairing, lazy backup, exact backup browsing and persistent restart identity'
    elif args.storage_only:
        scenario, scope = exercise_storage, 'Explicit Jobs measurement separates sync baselines from optional catalogs without enabling indexing'
    elif args.catalog_limits_only:
        scenario, scope = exercise_catalog_limits, 'Per-run catalog review, cancellation, configured-limit reset and explicit larger capture without changing defaults'
    elif args.ram_pending_only:
        scenario, scope = exercise_ram_pending, 'RAM pending changes and confirmed drive sync across removal, export and reconnect'
    elif args.policy_editor_only:
        scenario, scope = exercise_policy_editor, 'Whole-drive and folder policy lookup beyond status pagination without resetting saved flags'
    elif args.catalog_redaction_only:
        scenario, scope = exercise_catalog_redaction, 'Catalog reader retention, redaction dates, offline alias errors and explicit whole-drive cleanup'
    elif args.catalog_coverage_only:
        scenario, scope = exercise_catalog_coverage, 'Offline partial catalog labels, typed directory coverage and incomplete-folder navigation errors'
    elif args.catalog_types_only:
        scenario, scope = exercise_catalog_types, 'Offline special-file metadata labels, inert FIFO/socket entries and internal symlinks'
    elif args.catalog_boundary_only:
        scenario, scope = exercise_catalog_boundary, 'Offline catalog exclusion labels, empty folders and navigation errors'
    elif args.backup_review_only:
        scenario, scope = exercise_backup_review, 'Backup review: lazy selection, cancellation, file backup publication and image review'
    else:
        scenario, scope = exercise, 'Drive context menus, RAM writeback, union enrollment, lazy backups, subtree exclusions, offline FUSE and preserved conflicts'
    return run_application_acceptance(args, output, root, scenario, scope, prepare_environment=prepare)


if __name__ == '__main__':
    raise SystemExit(main())
