#!/usr/bin/env python3
"""Retire an owned legacy profile index on restart, including its saved enrollment."""
import json
import os
from pathlib import Path
import sqlite3
import tomllib
import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment, enroll_folder
from file_manager_search_acceptance import submit


def retired(index):
    with sqlite3.connect(index) as db:
        return (db.execute('SELECT state,available FROM roots').fetchall() == [('disabled', 0)]
                and all(db.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0] == 0
                        for table in ['paths', 'memberships', 'files', 'segments', 'segment_fts']))


def exercise(session, root, children, binary):
    ordinary, profile = root/'Enrolled', root/'.mozilla'
    session.address(ordinary)
    session.observe('ordinary-unindexed', ['profile.txt'])
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    _, (_, paths, vectors) = enroll_folder(session, ordinary)
    assert paths == [str(ordinary/'profile.txt')] and vectors == 1
    session.close()
    # Simulate a pre-policy catalog, using only this owned fixture. Constraints
    # stay enabled; source bytes remain unchanged and the closed app cannot race.
    ordinary.rename(profile)
    with sqlite3.connect(index) as db:
        db.execute('PRAGMA foreign_keys=ON')
        old, new = os.fsencode(ordinary/'profile.txt'), os.fsencode(profile/'profile.txt')
        db.execute('INSERT INTO paths(path,parent,file_id) SELECT ?,?,file_id FROM paths WHERE path=?',
                   [new, os.fsencode(profile), old])
        db.execute('UPDATE memberships SET path=? WHERE path=?', [new, old])
        db.execute('DELETE FROM paths WHERE path=?', [old])
        db.execute('UPDATE roots SET path=?', [os.fsencode(profile)])
        db.execute('UPDATE directories SET path=? WHERE path=?', [os.fsencode(profile), os.fsencode(ordinary)])
        assert db.execute('PRAGMA foreign_key_check').fetchall() == []
    changed = 0
    for config in [session.output/'scratch/config.toml', Path(session.env['XDG_CONFIG_HOME'])/'little-screen-clip/config.toml']:
        text = config.read_text()
        if str(ordinary) in text:
            text = text.replace(json.dumps(str(ordinary)), json.dumps(str(profile)))
            config.write_text(text)
            assert tomllib.loads(text)['file_manager']['search']['roots'] == [str(profile)]
            changed += 1
    assert changed
    session.process = children.start('files-resumed', [str(binary.resolve()), str(root)], env=session.env,
        cwd=session.output/'scratch', log_path=session.output/'files-resumed.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    session.key('F24')
    wait_for(lambda: retired(index), 'legacy private paths/text/vectors retired before source access', 20)
    session.observe('legacy-disabled', ['Public.txt'])
    session.click(1125, 118)  # Browsing hidden files cannot override content safety.
    submit(session, ':content:"profile_canary_7329";scope:recursive')
    session.observe('private-content-blocked', ['0 matching entries'])
    session.key('Escape')
    session.address(profile)
    session.observe('private-browser', ['profile.txt'])
    session.click(993, 118)
    session.observe('private-cannot-reenroll', ['profile.txt'])
    assert retired(index)
    assert (profile/'profile.txt').read_text() == 'Owned fixture: profile_canary_7329.\n'
    (session.output/'private-checks.json').write_text(json.dumps(dict(
        legacy_config_root_disabled=True, private_cache_rows_removed=True,
        hidden_content_search_blocked=True, direct_reenrollment_blocked=True,
        source_bytes_unchanged=True, secure_erasure_claimed=False), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'Enrolled').mkdir()
    (root/'Enrolled/profile.txt').write_text('Owned fixture: profile_canary_7329.\n')
    (root/'Public.txt').write_text('Ordinary public fixture.\n')
    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, session.children, args.binary),
        'Owned legacy profile catalog/config restart: disabled root, purged cached paths/FTS/vectors, hidden recursive content and direct enrollment denied; no real profiles', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
