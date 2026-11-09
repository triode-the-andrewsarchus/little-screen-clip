#!/usr/bin/env python3
"""Restore an owned legacy backup containing forbidden cached path references."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import tomllib

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import backup_through_settings, enroll_folder, search_environment
from file_manager_restore_acceptance import request_restore


def exercise(session, root):
    originals = {name: (root / name).read_bytes() for name in ['Public.txt', 'Private.txt']}
    session.observe('initial', list(originals))
    index, (_, _, vectors) = enroll_folder(session, root)
    assert vectors == 2
    backup = backup_through_settings(session, root)
    forbidden = root / '.mozilla/Profile.txt'
    alias = root / '.env'
    with sqlite3.connect(backup) as db:
        db.execute('PRAGMA foreign_keys=ON')
        for old, new, retain in [(root / 'Private.txt', forbidden, False), (root / 'Public.txt', alias, True)]:
            db.execute('INSERT INTO paths(path,parent,file_id) SELECT ?,?,file_id FROM paths WHERE path=?',
                       [os.fsencode(new), os.fsencode(new.parent), os.fsencode(old)])
            db.execute('INSERT INTO memberships(root_id,path,generation,verified) SELECT root_id,?,generation,verified FROM memberships WHERE path=?',
                       [os.fsencode(new), os.fsencode(old)])
            if not retain:
                db.execute('DELETE FROM memberships WHERE path=?', [os.fsencode(old)])
                db.execute('DELETE FROM paths WHERE path=?', [os.fsencode(old)])
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []
    backup_hash = hashlib.sha256(backup.read_bytes()).hexdigest()
    request_restore(session, backup, 'legacy-policy')

    def restored():
        if not tomllib.loads((session.output / 'scratch/config.toml').read_text())['file_manager']['search']['paused']:
            return False
        with sqlite3.connect(index) as db:
            return (db.execute('SELECT path FROM paths').fetchall() == [(os.fsencode(root / 'Public.txt'),)]
                    and db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == 1)
    wait_for(restored, 'import removes private-only payload and forbidden alias, retaining public vector')
    session.scroll_to_visible('restored-policy', ['Index restored'], position=(500, 450))
    with sqlite3.connect(index) as db:
        for table in ['files', 'paths', 'memberships', 'segments', 'segment_fts']:
            assert db.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0] == 1
        assert db.execute("SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'private_restore_canary'").fetchone()[0] == 0
        assert db.execute('SELECT COUNT(*) FROM roots').fetchone()[0] == 1
        current = db.execute('SELECT pipeline,vector FROM segments').fetchone()
    with sqlite3.connect(backup) as db:
        assert current == db.execute('SELECT s.pipeline,s.vector FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=?', [os.fsencode(root / 'Public.txt')]).fetchone()
    assert hashlib.sha256(backup.read_bytes()).hexdigest() == backup_hash
    assert all((root / name).read_bytes() == payload for name, payload in originals.items())
    assert not forbidden.exists() and not alias.exists()
    (session.output / 'restore-policy-checks.json').write_text(json.dumps(dict(
        forbidden_private_payload_removed=True, forbidden_shared_alias_removed=True,
        public_vector_unchanged=True, backup_and_sources_unchanged=True,
        no_protected_source_required=True, paused_after_restore=True), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'Public.txt').write_text('A telescope records distant stars and galaxies.\n')
    (root / 'Private.txt').write_text('Owned fixture: private_restore_canary.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Owned legacy backup: private-only cached payload and forbidden public-file alias removed before installation; public vector, sources and backup preserved; no actual private folders', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
