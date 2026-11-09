#!/usr/bin/env python3
"""Recover a genuinely unreadable active cache, then restore its prior backup."""
from pathlib import Path
import hashlib
import sqlite3
import tomllib

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import backup_through_settings, search_environment, scroll, word
from file_manager_restore_acceptance import request_restore
from file_manager_reuse_acceptance import inventory
from file_manager_search_settings_acceptance import click_word


def exercise(session, root, children, binary):
    session.observe('initial', ['One.txt'])
    source = (root / 'One.txt').read_bytes()
    data = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search'
    index = data / 'index.sqlite'
    session.click(993, 118)
    session.key('Escape')
    inventory(index)
    backup = backup_through_settings(session, root)
    backup_hash = hashlib.sha256(backup.read_bytes()).hexdigest()
    session.close()
    damaged = {'index.sqlite': b'corrupted active database', 'index.sqlite-wal': b'preserve old WAL bytes', 'index.sqlite-shm': b'preserve old shared memory'}
    for name, payload in damaged.items():
        (data / name).write_bytes(payload)
    session.process = children.start('files-corrupt', [str(binary.resolve()), str(root)], env=session.env,
        cwd=session.output / 'scratch', log_path=session.output / 'files-corrupt.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    session.observe('unreadable-cache-browser', ['One.txt'])
    session.click(151, 70)
    session.observe('recovery-edit-menu', ['Search settings'])
    click_word(session, 'recovery-edit-menu', 'Search')
    session.scroll_to_visible('recovery-maintenance-header', ['Index maintenance'], position=(500, 450))
    click_word(session, 'recovery-maintenance-header', 'maintenance')
    # The error banner itself mentions Recover unreadable index. Anchor the
    # actual maintenance controls below Restore source instead of that banner.
    session.scroll_to_visible('recovery-controls', ['Restore source'], position=(500, 450))
    scroll(session, 140)
    session.observe('recovery-controls-visible', ['Restore source'])
    anchor = word(session, 'recovery-controls-visible', 'source')
    session.click(220, anchor[1] + 106)
    session.scroll_to_visible('recovery-confirmation', ['Preserve old cache and reset'], position=(500, 450))
    anchor = word(session, 'recovery-confirmation', 'Preserve')
    session.click(anchor[0] + 15, anchor[1])
    config = session.output / 'scratch/config.toml'

    def recovered():
        settings = tomllib.loads(config.read_text())['file_manager']['search']
        if not settings.get('paused'):
            return False
        try:
            with sqlite3.connect(index) as db:
                return db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == 0
        except sqlite3.Error:
            return False
    wait_for(recovered, 'recover corrupt cache and persist pause')
    session.scroll_to_visible('recovery-completed', ['Empty index created', 'Previous cache preserved'], position=(500, 450))
    archives = list(data.glob('.search-recovery-*'))
    assert len(archives) == 1 and archives[0].is_dir(), archives
    for name, payload in damaged.items():
        assert (archives[0] / name).read_bytes() == payload
    assert not (data / '.search-recovery-pending').exists()
    assert tomllib.loads(config.read_text())['file_manager']['search']['roots'] == [str(root)]
    assert (root / 'One.txt').read_bytes() == source
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    request_restore(session, backup, 'after-recovery')

    def restored():
        with sqlite3.connect(index) as db:
            return db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == 1
    wait_for(restored, 'restore prior backup after cache recovery')
    session.scroll_to_visible('restored-after-recovery', ['Index restored'], position=(500, 450))
    assert (root / 'One.txt').read_bytes() == source
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    assert (data / 'models').is_dir()


def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'One.txt').write_text('Cats have whiskers and soft fur.')
    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, session.children, args.binary),
        'Explicit unreadable-cache recovery preserves DB/WAL/SHM, models, source and enrollment; persistent pause and subsequent backup restore', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
