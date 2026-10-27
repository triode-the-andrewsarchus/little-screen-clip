#!/usr/bin/env python3
"""Verify model-independent search with real cached text on the private compositor."""
from pathlib import Path
import sqlite3
import os
import signal
import subprocess

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


def exercise(session, root, children, binary):
    data = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search'
    index = data / 'index.sqlite'
    session.observe('initial', ['ordinary.txt', 'violet.txt'])
    session.click(647, 160)  # Close Shelf so both provenance footers fit.
    submit(session, ':emb:"violet"')
    session.observe('unindexed-filename', ['violet.txt', 'Filename results only', 'no folders enrolled'])
    assert not index.exists()
    session.key('Escape')
    submit(session, ':emb:"violet";scope:indexed')
    session.observe('empty-indexed-scope', ['No folders enrolled', '0 of 0 matching entries'])
    assert not index.exists(), 'An empty indexed scope must not create a catalog'
    session.key('Escape')
    session.click(993, 118)
    session.key('Escape')
    inventory(index)
    # Retire only the loaded model child of this private application. Its owner
    # must recover current lexical results even when it thought the model warm.
    workers = subprocess.check_output(['ps', '-o', 'pid=,args=', '--ppid', str(session.process.pid)], text=True)
    workers = [int(row.split()[0]) for row in workers.splitlines()
               if 'search_model/worker.py' in row and '--role embedding' in row]
    assert len(workers) == 1, workers
    worker = workers[0]
    handle = os.pidfd_open(worker)
    try:
        status = Path(f'/proc/{worker}/status').read_text()
        assert f'PPid:\t{session.process.pid}\n' in status
        signal.pidfd_send_signal(handle, signal.SIGKILL)
    finally:
        os.close(handle)
    submit(session, ':emb:"violet lighthouse"')
    session.observe('warm-model-failure-keeps-keywords', ['ordinary.txt', 'Keyword match', 'semantic model unavailable'])
    assert 'model_resident=true preliminary_lexical=false' in (session.output / 'files.log').read_text()
    submit(session, ':emb:"violet lighthouse"')
    session.observe('cold-model-recovery', ['ordinary.txt', 'hybrid filename/keyword/meaning'])
    assert 'model_resident=false preliminary_lexical=true' in (session.output / 'files.log').read_text()
    session.key('q', 'ctrl')
    session.process.wait(timeout=5)
    assert session.process.returncode == 0
    (data / 'models').unlink()  # Only the private symlink; leave actual models alone.
    (data / 'models').mkdir()
    session.process = children.start('files-no-model', [str(binary.resolve()), str(root)], env=session.env,
        cwd=session.output / 'scratch', log_path=session.output / 'files-no-model.log')
    private.wait_for_app_window(session.niri, session.env, session.process, session.output)
    session.observe('reopened-no-model', ['ordinary.txt', 'violet.txt'])
    session.click(647, 160)
    submit(session, ':emb:"violet lighthouse"')
    session.observe('cached-keyword-without-model', ['ordinary.txt', 'Keyword match', 'Violet lighthouse', 'semantic model unavailable'])
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL').fetchone()[0] >= 2
    # Original cached keyword text no longer authorizes a changed source.
    (root / 'ordinary.txt').write_text('This changed source does not contain the old keyword.')
    submit(session, ':emb:"violet lighthouse"')
    text = session.observe('changed-cached-source-rejected', ['semantic model unavailable', '0 of 0 matching entries'])
    assert 'ordinary.txt' not in text.lower()
    other = session.output / 'unindexed'
    other.mkdir()
    (other / 'violet-note.txt').write_text('An ordinary note outside enrolled roots.')
    session.key('Escape')
    session.address(other)
    session.observe('other-folder', ['violet-note.txt'])
    submit(session, ':emb:"violet"')
    session.observe('unenrolled-folder-with-existing-index', ['violet-note.txt', 'Filename results only', 'current folder not enrolled'])
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM roots').fetchone()[0] == 1



def main():
    repository = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root / 'ordinary.txt').write_text('Violet lighthouse guides the fishing boats safely home.')
    (root / 'violet.txt').write_text('A grocery shopping list with milk and eggs.')
    state = {}

    def environment(env, children):
        env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
        state['children'] = children
        data = Path(env['XDG_DATA_HOME']) / 'little-screen-clip/search'
        data.mkdir(parents=True)
        for directory in ['runtime', 'models']:
            (data / directory).symlink_to(Path.home() / '.local/share/little-screen-clip/search' / directory, target_is_directory=True)

    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, state['children'], args.binary),
        'Unenrolled filename fallback; failed warm model retains current keywords; cold recovery; persisted keyword search after restart without model; fresh source rejection', environment)


if __name__ == '__main__':
    raise SystemExit(main())
