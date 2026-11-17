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
        for bounds in [(105, 270, 225, 325), (105, 320, 240, 380), (105, 390, 300, 670), (820, 505, 888, 520)]:
            crop = session.output / 'drive-button.png'
            with Image.open(session.output / (shot + '.png')) as screen:
                region = screen.crop(bounds)
                region.resize((region.width * 6, region.height * 6)).save(crop)
            data = private.run_checked(['tesseract', str(crop), 'stdout', '--psm', '6', '-c', 'tessedit_create_tsv=1', '-c', 'tessedit_create_txt=0'], env=session.env)
            for item in csv.DictReader(io.StringIO(data), delimiter='\t', quoting=csv.QUOTE_NONE):
                if item['text'].strip().strip('()[]{}|').lower() == word.lower():
                    position = (bounds[0] + int(item['left']) // 6 + 5, bounds[1] + (int(item['top']) + int(item['height']) // 2) // 6)
                    break
            if position:
                break
    assert position, (shot, word)
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


def cancel_backup_review(session, shot):
    # Verify the bordered button's interior instead of accepting its frame as text.
    x, y = session.word(shot, 'Start', tree=True, required=False) or session.word(shot, 'Start')
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
    cancel_backup_review(session, 'backup-review-cancel')
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
    session.observe('backup-review-start', ['Review partition backup', 'Available', 'Start backup'])
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
    cancel_backup_review(session, 'image-review-cancel')
    assert len(status(env)['backups']) == 1

    return saved


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


def exercise_catalog_boundary(session, root):
    """Verify exclusion presentation without repeating unrelated storage workflows."""
    env = session.env
    drive = status(env)['drives'][0]['id']
    completed(env, request(env, {'Rule': {'drive': drive, 'relative': 'many', 'catalog': False, 'index': False}}))
    completed(env, request(env, {'Catalog': {'drive': drive}}))
    catalog = status(env)['catalogs'][0]
    (root / 'portable').rename(root / 'removed-portable')
    wait_for(lambda: not status(env)['drives'], 'offline catalog source')
    completed(env, request(env, {'MountCatalog': {'catalog': catalog['id']}}))
    try:
        mount = Path(next(item for item in status(env)['catalogs'] if item['id'] == catalog['id'])['mount'])
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


def catalog_context_menu(session, name, expected):
    # Status notices and ancestor inventories move Mounts after navigation.
    # Refresh and retry only this read-only menu opening if the row moved.
    target = name + '-target'
    def visible():
        text = session.capture_text(name, session.output / (name + '.png'), [])
        if 'opensavedcatalog' not in ''.join(text.lower().split()):
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
    drive = status(env)['drives'][0]['id']
    for relative, catalog, index in [('', False, True), ('many', True, False)]:
        completed(env, request(env, {'Rule': {'drive': drive, 'relative': relative, 'catalog': catalog, 'index': index}}))
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


def main():
    repository = Path(__file__).resolve().parent.parent
    def configure(parser):
        scenarios = parser.add_mutually_exclusive_group()
        scenarios.add_argument('--backup-review-only', action='store_true')
        scenarios.add_argument('--catalog-boundary-only', action='store_true')
        scenarios.add_argument('--catalog-redaction-only', action='store_true')
        scenarios.add_argument('--policy-editor-only', action='store_true')
        scenarios.add_argument('--ram-pending-only', action='store_true')
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager', configure=configure)
    output, root = media_fixture_directory(args)
    for name in ['portable', 'local', 'backups']:
        (root / name).mkdir()
    (root / 'portable/readme.txt').write_text('original portable contents\n')
    (root / 'portable/many').mkdir()
    (root / 'portable/many/hidden.txt').write_text('Back up and sync this; exclude catalog/index traversal.\n')
    if args.catalog_boundary_only:
        (root / 'portable/empty').mkdir()
    (root / 'local/local-only.txt').write_text('Union merge keeps this file.\n')
    def prepare(env, children):
        state = output / 'drive-state'
        env['LITTLE_CLIP_DRIVE_STATE_DIR'] = str(state)
        env['LITTLE_CLIP_DRIVE_MOUNT_HELPER'] = str(repository / 'target/debug/clip-drive-mount')
        children.start('drive-fixture', [str(repository / 'target/debug/examples/drive_fixture'), str(state), str(root / 'portable')], env=env, cwd=output, log_path=output / 'drive-fixture.log')
        wait_for(lambda: (Path(env['XDG_RUNTIME_DIR']) / 'little-screen-clip.sock').exists(), 'owned drive fixture IPC')
        if args.policy_editor_only:
            seed_old_policy_rules(env)
    if args.ram_pending_only:
        scenario, scope = exercise_ram_pending, 'RAM pending changes and confirmed drive sync across removal, export and reconnect'
    elif args.policy_editor_only:
        scenario, scope = exercise_policy_editor, 'Whole-drive and folder policy lookup beyond status pagination without resetting saved flags'
    elif args.catalog_redaction_only:
        scenario, scope = exercise_catalog_redaction, 'Catalog reader retention, durable exclusion, redaction dates and reopened FUSE view'
    elif args.catalog_boundary_only:
        scenario, scope = exercise_catalog_boundary, 'Offline catalog exclusion labels, empty folders and navigation errors'
    elif args.backup_review_only:
        scenario, scope = exercise_backup_review, 'Backup review: lazy selection, cancellation, file backup publication and image review'
    else:
        scenario, scope = exercise, 'Drive context menus, RAM writeback, union enrollment, lazy backups, subtree exclusions, offline FUSE and preserved conflicts'
    return run_application_acceptance(args, output, root, scenario, scope, prepare_environment=prepare)


if __name__ == '__main__':
    raise SystemExit(main())
