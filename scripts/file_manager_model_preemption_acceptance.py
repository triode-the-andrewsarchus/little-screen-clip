#!/usr/bin/env python3
"""A real embedding request yields to a query without losing the interrupted passage."""
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_quality_acceptance import search_timings
from file_manager_reuse_acceptance import inventory
from file_manager_search_acceptance import submit


CANARY = 'MODEL_PREEMPT_CANARY_7319'


def exercise(session, root):
    healthy, slow = root/'Healthy', root/'Slow'
    source = slow/'Slow.txt'
    original = hashlib.sha256(source.read_bytes()).hexdigest()
    index, (_, paths, vectors) = enroll_folder(session, healthy)
    assert paths == [str(healthy/'Healthy.txt')] and vectors == 1
    session.address(slow)
    session.observe('slow-before-enrollment', ['Slow.txt'])
    session.click(993, 118)
    session.key('Escape')
    marker, release = session.output/'held-model.pid', session.output/'release-model'
    try:
        wait_for(marker.exists, 'owned background vector request reaches its user-space gate', 30)
        pid = int(marker.read_text())
        assert Path(f'/proc/{pid}').exists() and not release.exists()
        session.address(healthy)
        session.observe('healthy-during-stall', ['Healthy.txt'])
        log = session.output/'files.log'
        previous = {row['request_id'] for row in search_timings(log)}
        started = time.monotonic()
        submit(session, ':emb:"Healthy.txt"')
        def completed():
            return [row for row in search_timings(log) if row['complete'] and row['request_id'] not in previous]
        try:
            wait_for(completed, 'query preempts stalled inference and completes with a replacement model', 10)
        finally:
            rows = completed()
            reaped = not Path(f'/proc/{pid}').exists()
            (session.output/'preemption.json').write_text(json.dumps(dict(
                held_pid=pid, child_reaped=reaped, gate_released=release.exists(),
                query_seconds=time.monotonic()-started, results=rows), indent=2))
        assert reaped and not release.exists()
        assert len(rows) == 1 and rows[0]['succeeded'] and rows[0]['entries'] == 1, rows
        session.observe('query-after-preemption', ['Healthy.txt', 'Semantic candidates'])
        def both_ready():
            with sqlite3.connect(index) as db:
                return db.execute("SELECT COUNT(*),SUM(state='ready') FROM roots").fetchone() == (2, 2)
        wait_for(both_ready, 'interrupted root completes all passages on retry', 60)
        _, paths, vectors = inventory(index)
        assert set(paths) == {str(healthy/'Healthy.txt'), str(source)} and vectors >= 3
        with sqlite3.connect(index) as db:
            coverage, text = db.execute('SELECT f.coverage,group_concat(s.text, char(10)) FROM files f JOIN paths p ON p.file_id=f.id JOIN segments s ON s.file_id=f.id WHERE p.path=? GROUP BY f.id', [bytes(source)]).fetchone()
        assert coverage == 'complete' and CANARY in text, (coverage, text[:100])
        assert hashlib.sha256(source.read_bytes()).hexdigest() == original
        assert not release.exists(), 'retry must finish without releasing the abandoned helper'
        session.key('Escape')
        session.address(slow)
        submit(session, ':content:"'+CANARY+'"')
        session.observe('interrupted-first-passage-retained', ['Slow.txt', '1 matching entries'])
    finally:
        release.touch()


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    for name in ['Healthy', 'Slow']:
        (root/name).mkdir()
    (root/'Healthy/Healthy.txt').write_text('Healthy ordinary content for a quick search.\n')
    (root/'Slow/Slow.txt').write_text(CANARY+'\n'+'An owned document with multiple passages of ordinary text.\n'*400)
    library = output/'model-stall.so'
    subprocess.run(['cc', '-shared', '-fPIC', '-O2', '-Wall', '-Wextra', '-Werror', str(repo/'scripts/fixtures/search_reader_stall.c'), '-ldl', '-o', str(library)], check=True)
    def prepare(env, children):
        environment(env, children)
        env.update(LD_PRELOAD=str(library), CLIP_TEST_STALL_MODEL_TEXT=CANARY,
                   CLIP_TEST_STALL_MARKER=str(output/'held-model.pid'), CLIP_TEST_STALL_RELEASE=str(output/'release-model'))
    return run_application_acceptance(args, output, root, exercise,
        'One controlled background model request: interactive query preempts/reaps child, retries preserve the first passage and source bytes; real local embeddings, private compositor only', prepare)


if __name__ == '__main__':
    raise SystemExit(main())
