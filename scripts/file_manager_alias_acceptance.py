#!/usr/bin/env python3
"""Mixed-format hardlinks, legacy migration and alias removal with real embeddings."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_reuse_acceptance import inventory, rebuild


def snapshot(index, root):
    with sqlite3.connect(index) as db:
        assert db.execute('PRAGMA user_version').fetchone()[0] == 6
        assert not db.execute('PRAGMA foreign_key_check').fetchall()
        rows = db.execute('''SELECT p.path,f.id,f.state,f.coverage,
            (SELECT count(*) FROM segments s WHERE s.file_id=f.id AND s.vector IS NOT NULL)
            FROM paths p JOIN files f ON f.id=p.file_id ORDER BY p.path''').fetchall()
    records = {Path(os.fsdecode(path)).name: dict(file_id=file, state=state, coverage=coverage, vectors=vectors)
               for path, file, state, coverage, vectors in rows}
    assert records['a.txt']['file_id'] == records['alias.TXT']['file_id'], records
    assert records['a.txt']['file_id'] != records['page.html']['file_id'], records
    for name in ['a.txt', 'alias.TXT', 'page.html']:
        assert records[name]['state'] == 'ready' and records[name]['vectors'] == 1, records
    if (root/'z.data').exists():
        assert records['z.data']['state'] == 'unsupported' and records['z.data']['vectors'] == 0, records
        assert len({records[name]['file_id'] for name in ['a.txt', 'page.html', 'z.data']}) == 3
    return records


def legacy_catalog(index):
    """Construct the real v5 identity collision in an owned, closed fixture DB."""
    with sqlite3.connect(index) as db:
        assert not db.execute('PRAGMA foreign_keys').fetchone()[0]
        text = next(file for path, file in db.execute('SELECT path,file_id FROM paths')
                    if os.fsdecode(path).endswith('/a.txt'))
        db.execute('UPDATE paths SET file_id=?', (text,))
        db.execute('DELETE FROM segments WHERE file_id<>?', (text,))
        db.execute('DELETE FROM source_digests WHERE file_id<>?', (text,))
        db.execute('DELETE FROM files WHERE id<>?', (text,))
        db.execute('''CREATE TABLE files_v5(id INTEGER PRIMARY KEY,device INTEGER NOT NULL,
            inode INTEGER NOT NULL,revision INTEGER NOT NULL,pipeline TEXT NOT NULL,
            metadata TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',coverage TEXT,error TEXT,
            extraction_version INTEGER NOT NULL DEFAULT 0,scope TEXT NOT NULL DEFAULT '',
            UNIQUE(device,inode,scope))''')
        db.execute('''INSERT INTO files_v5 SELECT id,device,inode,revision,pipeline,metadata,state,
            coverage,error,extraction_version,scope FROM files''')
        db.execute('DROP TABLE files')
        db.execute('ALTER TABLE files_v5 RENAME TO files')
        db.execute('PRAGMA user_version=5')


def exercise(session, root, binary):
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}
    assert len({p.stat().st_ino for p in root.iterdir()}) == 1
    session.click(647, 160)  # Collapse Shelf so all four text rows fit on the private output.
    index, (generation, paths, vectors) = enroll_folder(session, root)
    assert len(paths) == 4 and vectors == 2, (paths, vectors)
    initial = snapshot(index, root)
    session.observe('mixed-aliases-ready', ['a.txt', 'alias.TXT', 'page.html', 'z.data'])
    session.close()
    legacy_catalog(index)
    session.process = session.children.start('files-migrated', [str(binary.resolve()), str(root)],
        env=session.env, cwd=session.output/'scratch', log_path=session.output/'files-migrated.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    session.key('F24')
    session.click(647, 160)
    generation, paths, vectors = inventory(index, generation)
    assert len(paths) == 4 and vectors == 2, (paths, vectors)
    migrated = snapshot(index, root)
    session.observe('legacy-aliases-repaired', ['a.txt', 'alias.TXT', 'page.html', 'z.data'])
    assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir()}
    (root/'z.data').unlink()
    generation, paths, vectors = rebuild(session, index, generation, 'remove-unsupported-alias')
    assert len(paths) == 3 and vectors == 2, (paths, vectors)
    remaining = snapshot(index, root)
    assert before['a.txt'] == hashlib.sha256((root/'a.txt').read_bytes()).hexdigest()
    session.observe('supported-aliases-survive-removal', ['a.txt', 'alias.TXT', 'page.html'])
    (session.output/'alias-results.json').write_text(json.dumps(dict(
        same_inode=True, initial=initial, migrated=migrated, remaining=remaining,
        source_hashes=before, schema=6), indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'a.txt').write_text('The violet lighthouse guides boats through the harbour.\n')
    for name in ['alias.TXT', 'page.html', 'z.data']:
        os.link(root/'a.txt', root/name)
    return run_application_acceptance(args, output, root,
        lambda session, root: exercise(session, root, args.binary),
        'Real CUDA embeddings across mixed decoder aliases, v5 migration and alias removal', environment)


if __name__ == '__main__':
    raise SystemExit(main())
