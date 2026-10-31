#!/usr/bin/env python3
"""Overlapping-root ownership and failed-inventory retention on private Weston/Niri."""
import json
import os
from pathlib import Path
import sqlite3

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment
from file_manager_search_acceptance import submit


def catalog(index):
    if not index.exists():
        return None
    with sqlite3.connect(index) as db:
        db.execute('BEGIN')  # All assertions describe one committed catalog snapshot.
        roots = {os.fsdecode(path): dict(id=id, generation=generation, state=state, error=error)
                 for id, path, generation, state, error in db.execute('SELECT id,path,generation,state,error FROM roots')}
        paths = {os.fsdecode(path): file for path, file in db.execute('SELECT path,file_id FROM paths')}
        memberships = [dict(root=os.fsdecode(root), path=os.fsdecode(path), verified=bool(verified))
                       for root, path, verified in db.execute('SELECT r.path,m.path,m.verified FROM memberships m JOIN roots r ON r.id=m.root_id')]
        vectors = [dict(file=file, input=bytes(input).hex(), vector=bytes(vector).hex(), text=text)
                   for file, input, vector, text in db.execute('SELECT file_id,input_hash,vector,text FROM segments WHERE vector IS NOT NULL ORDER BY file_id,modality,input_hash')]
    return dict(roots=roots, paths=paths, memberships=memberships, vectors=vectors)


def expect_catalog(session, index, label, predicate):
    observed = None
    def ready():
        nonlocal observed
        observed = catalog(index)
        return observed is not None and predicate(observed)
    wait_for(ready, label, 90)
    (session.output / f'{label}.json').write_text(json.dumps(observed, indent=2))
    return observed


def remove_current_root(session, label):
    session.click(993, 118)
    session.observe(label, ['Remove indexed folder'])
    session.click(*session.word(label, 'Remove', last=True))


def exercise(session, root):
    shared = root / 'Shared'
    one, twin, other = shared / 'One.txt', shared / 'Twin.txt', root / 'Quiet/Other.txt'
    original = {str(path): path.read_bytes() for path in [one, twin, other]}
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    session.observe('initial', ['Shared', 'Quiet'])
    assert not index.exists()
    session.address(shared)
    session.observe('child-before-enrollment', ['One.txt', 'Twin.txt'])
    session.click(993, 118)
    session.key('Escape')
    first = expect_catalog(session, index, 'child-indexed', lambda c:
        len(c['roots']) == 1 and all(r['state'] == 'ready' for r in c['roots'].values()) and len(c['paths']) == 2)
    assert len(first['vectors']) == 1
    assert first['paths'][str(one)] == first['paths'][str(twin)]
    session.address(root)
    session.observe('parent-before-enrollment', ['Shared', 'Quiet'])
    session.click(993, 118)
    session.key('Escape')
    both = expect_catalog(session, index, 'overlapping-roots', lambda c:
        len(c['roots']) == 2 and all(r['state'] == 'ready' for r in c['roots'].values()) and len(c['memberships']) == 5)
    assert len(both['paths']) == 3 and len(both['vectors']) == 2, both
    assert both['paths'][str(one)] == both['paths'][str(twin)] == first['paths'][str(one)]
    check_failed_inventory(session, root, index, both)
    check_owner_retirement(session, root, index, both)
    assert other.read_bytes() == original[str(other)]
    assert twin.read_bytes() == b'A newly edited surviving sample describes an arctic fox.\n'


def check_failed_inventory(session, root, index, both):
    shared, other = root / 'Shared', root / 'Quiet/Other.txt'
    mode = shared.stat().st_mode & 0o777
    try:
        shared.chmod(0)
        failed = expect_catalog(session, index, 'unreadable-shared-inventory', lambda c:
            len(c['roots']) == 2 and all(r['state'] in ('error', 'offline') and r['error'] for r in c['roots'].values())
            and all(not m['verified'] for m in c['memberships'] if m['path'].startswith(str(shared) + '/')))
        assert failed['paths'] == both['paths'], 'Failed scans must retain every old path'
        assert failed['vectors'] == both['vectors'], 'Failed scans must retain canonical vectors'
        assert len(failed['memberships']) == 5
        assert next(m for m in failed['memberships'] if m['path'] == str(other))['verified']
        submit(session, ':emb:"Other.txt";scope:indexed')
        session.observe('healthy-branch-searchable', ['Other.txt', '1 matching entries'])
        session.key('Escape')
    finally:
        shared.chmod(mode)
    expect_catalog(session, index, 'permission-recovery', lambda c:
        len(c['roots']) == 2 and all(r['state'] == 'ready' for r in c['roots'].values()) and all(m['verified'] for m in c['memberships']))


def check_owner_retirement(session, root, index, both):
    shared = root / 'Shared'
    one, twin = shared / 'One.txt', shared / 'Twin.txt'
    one.unlink()
    removed = expect_catalog(session, index, 'confirmed-hardlink-removal', lambda c:
        all(r['state'] == 'ready' for r in c['roots'].values()) and str(one) not in c['paths'])
    assert len(removed['paths']) == 2 and len(removed['memberships']) == 3 and len(removed['vectors']) == 2
    assert removed['vectors'] == both['vectors'], 'Removing one name must preserve unchanged canonical inputs'
    remove_current_root(session, 'remove-enclosing-root')
    survivor = expect_catalog(session, index, 'child-owner-survives', lambda c:
        set(c['roots']) == {str(shared)} and set(c['paths']) == {str(twin)})
    assert len(survivor['vectors']) == len(survivor['memberships']) == 1
    session.address(shared)
    session.observe('surviving-child-browser', ['Twin.txt'])
    twin.write_text('A newly edited surviving sample describes an arctic fox.\n')
    edited = expect_catalog(session, index, 'surviving-watch-edited', lambda c:
        c['roots'][str(shared)]['generation'] > survivor['roots'][str(shared)]['generation']
        and c['roots'][str(shared)]['state'] == 'ready'
        and any('arctic fox' in v['text'] for v in c['vectors']))
    assert len(edited['vectors']) == 1 and edited['vectors'] != survivor['vectors']
    submit(session, ':emb:"arctic fox";scope:indexed')
    session.observe('surviving-child-search', ['Twin.txt', '1 matching entries'])
    session.key('Escape')
    remove_current_root(session, 'remove-final-root')
    empty = expect_catalog(session, index, 'last-owner-cleanup', lambda c: not c['roots'])
    assert not empty['paths'] and not empty['memberships'] and not empty['vectors']
    with sqlite3.connect(index) as db:
        remaining = {table: db.execute(f'SELECT COUNT(*) FROM {table}').fetchone()[0]
                     for table in ('files', 'directories', 'source_sessions', 'source_digests', 'file_checkpoints', 'segment_fts')}
    assert not any(remaining.values()), remaining
    (session.output / 'derived-cleanup.json').write_text(json.dumps(remaining, indent=2))
    session.observe('cleanup-retains-source', ['Twin.txt'])


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    assert os.geteuid() != 0, 'Permission failure requires an ordinary user'
    (root / 'Shared').mkdir()
    (root / 'Quiet').mkdir()
    (root / 'Shared/One.txt').write_text('A shared sample describes a violet lighthouse.\n')
    os.link(root / 'Shared/One.txt', root / 'Shared/Twin.txt')
    (root / 'Quiet/Other.txt').write_text('Independent healthy content about oak trees.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Same-path parent/child memberships and hard-link vector reuse; unreadable inventories retain records; healthy branch remains searchable; recovery, confirmed deletion and removing one owner preserve remaining watches; last-owner cleanup', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
