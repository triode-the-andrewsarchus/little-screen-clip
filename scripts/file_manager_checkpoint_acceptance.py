#!/usr/bin/env python3
"""Close/reopen a genuinely unfinished indexed document in a private Files session."""
import hashlib
import json
from pathlib import Path
import re
import sqlite3
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_maintenance_acceptance import search_environment
from file_manager_real_corpus_acceptance import wait_index
from file_manager_search_acceptance import submit
import ui_acceptance_capture as private


def verify_segments(rows, source):
    """This fixture is ASCII/LF plain text: extracted offsets are source offsets."""
    covered = 0
    for _, location, input_hash, _ in rows:
        match = re.search(r'UTF-8 bytes (\d+)\.\.(\d+)$', location)
        assert match, location
        start, end = map(int, match.groups())
        assert 0 <= start <= covered < end <= len(source), (start,end,covered)
        assert hashlib.sha256(b'title: none | text: '+source[start:end]).digest() == input_hash
        covered = end
    assert covered == len(source), (covered,len(source))
    return {'source_bytes':len(source), 'covered_bytes':covered, 'verified_segments':len(rows)}


def checkpoint(index):
    if not index.exists():
        return None
    try:
        with sqlite3.connect(index) as db:
            row = db.execute("SELECT c.file_id,c.progress,c.segments,f.coverage,r.state,f.revision FROM file_checkpoints c JOIN files f ON f.id=c.file_id JOIN paths p ON p.file_id=f.id JOIN memberships m ON m.path=p.path JOIN roots r ON r.id=m.root_id").fetchone()
            if not row:
                return None
            prefix = db.execute('SELECT id,location,input_hash,vector FROM segments WHERE file_id=? ORDER BY id', [row[0]]).fetchall()
        return {'file':row[0], 'progress':json.loads(row[1]), 'count':row[2], 'coverage':row[3], 'root_state':row[4], 'revision':row[5], 'prefix':prefix}
    except sqlite3.OperationalError as error:
        if 'no such table' in str(error):
            return None
        raise


def completed_segments(index, saved, replace):
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM file_checkpoints').fetchone()[0] == 0
        final = db.execute('SELECT id,location,input_hash,vector FROM segments WHERE file_id=? ORDER BY id', [saved['file']]).fetchall()
        if replace:
            assert len(final) == 1
            assert final[0][2] not in {row[2] for row in saved['prefix']}
            assert db.execute('SELECT revision FROM files WHERE id=?',[saved['file']]).fetchone()[0] != saved['revision']
        else:
            assert len(final) > saved['count']
            assert final[:saved['count']] == saved['prefix'], 'Saved locations/IDs/canonical vectors changed'
        assert len({row[1] for row in final}) == len(final), 'Restart duplicated a passage location'
        assert db.execute('SELECT coverage FROM files WHERE id=?',[saved['file']]).fetchone()[0] == 'complete'
    return final


def exercise(session, root, children, binary, replace):
    source = root/'long.txt'
    digest = hashlib.sha256(source.read_bytes()).hexdigest()
    session.observe('initial-unindexed', ['long.txt'])
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    wait_for(lambda: checkpoint(index), 'first durable partial batch', 120)
    # No OCR pause here: stop before this deliberately long file finishes.
    session.key('q', 'ctrl')
    session.process.wait(timeout=10)
    assert session.process.returncode == 0
    saved = checkpoint(index)
    assert saved and saved['count'] >= 8, saved
    assert saved['coverage'].startswith('partial: indexing in progress'), saved
    assert saved['root_state'] != 'ready', saved
    assert len(saved['prefix']) == saved['count']
    assert hashlib.sha256(source.read_bytes()).hexdigest() == digest
    assert bytes(saved['progress']['prepared_hash']) == hashlib.sha256(source.read_bytes()).digest()
    (session.output/'closed-checkpoint.json').write_text(json.dumps({
        **{key:value for key,value in saved.items() if key != 'prefix'},
        'source_sha256':digest, 'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
        'prefix': [{'id':row[0], 'location':row[1],
                    'input_sha256':row[2].hex(), 'vector_sha256':hashlib.sha256(row[3]).hexdigest()}
                   for row in saved['prefix']],
    },indent=2))
    if replace:
        # Only this driver's owned synthetic source; no application is running.
        source.write_text('A replacement document describes river geology and sediment deposits.\n')
        digest = hashlib.sha256(source.read_bytes()).hexdigest()

    session.process = children.start('files-resumed', [str(binary.resolve()), str(root)], env=session.env,
        cwd=session.output/'scratch', log_path=session.output/'files-resumed.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    log = session.output/'files-resumed.log'
    if not replace:
        wait_for(lambda: 'resumed verified indexing checkpoint' in log.read_text(), 'verified checkpoint resume', 120)
    summary = wait_index(session,index,180,time.monotonic())
    assert summary['roots'] == [('ready',None)], summary
    resumed = re.search(r'resumed verified indexing checkpoint.*segments=(\d+)', log.read_text())
    if replace:
        assert not resumed, 'Changed source inherited an old cursor'
    else:
        assert resumed and int(resumed.group(1)) == saved['count']
    final = completed_segments(index,saved,replace)
    coverage = verify_segments(final,source.read_bytes())
    submit(session, ':emb:"checkpoint sentinel quasar"')
    session.observe('old-passage-after-source-change' if replace else 'late-passage-after-resume',
                    ['0 matching entries', 'No matching files'] if replace else ['long.txt', 'Keyword match'])
    assert hashlib.sha256(source.read_bytes()).hexdigest() == digest
    (session.output/'checkpoint-checks.json').write_text(json.dumps({
        'source_sha256':digest, 'closed_committed_segments':saved['count'],
        'saved_cursor':saved['progress']['cursor'], 'saved_text_cursor':saved['progress']['text_cursor'],
        'source_replaced_while_closed':replace,
        'resumed_segments':int(resumed.group(1)) if resumed else None, 'completed_segments':len(final),
        'canonical_prefix_unchanged':None if replace else True, 'source_unchanged_after_restart':True,
        'no_duplicate_locations':True, 'late_passage_found':not replace,
        'coverage':coverage,
    },indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__,repo/'target/debug/clip-file-manager',
        lambda parser:parser.add_argument('--replace-before-resume',action='store_true'))
    output, root = media_fixture_directory(args)
    paragraph = 'The laboratory records temperature, calibration, materials, observations, and maintenance procedures for this independent experiment. '
    with (root/'long.txt').open('w') as stream:
        for index in range(1024):
            stream.write(f'Section {index:04d}. '+paragraph*12+'\n\n')
        stream.write('The final checkpoint sentinel quasar confirms the last passage was indexed.\n')
    state = {}
    def environment(env, children):
        state['children'] = children
        env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
        search_environment(env, children)
    return run_application_acceptance(args,output,root,
        lambda session,folder:exercise(session,folder,state['children'],args.binary,args.replace_before_resume),
        'Changed source rejects saved cursor and old passages' if args.replace_before_resume else
        'Explicit enrollment, durable partial batch, real shutdown/restart, verified cursor, canonical prefix, complete late passage',environment)


if __name__ == '__main__':
    raise SystemExit(main())
