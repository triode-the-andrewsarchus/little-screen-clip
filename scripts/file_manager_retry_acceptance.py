#!/usr/bin/env python3
"""Actual per-file retry timer, manual reset and changed-source recovery."""
import json
from pathlib import Path
import sqlite3
import time
from PIL import Image

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_quality_acceptance import measure_ui_queries
from file_manager_scale_acceptance import model_worker


def state(index, source):
    if not index.exists():
        return None
    with sqlite3.connect(index) as db:
        row = db.execute('''SELECT r.generation,r.state,f.state,f.error,q.failures,
            q.retry_after-unixepoch(),(SELECT count(*) FROM segments s WHERE s.file_id=f.id AND vector IS NOT NULL)
            FROM roots r JOIN memberships m ON m.root_id=r.id JOIN paths p ON p.path=m.path
            JOIN files f ON f.id=p.file_id LEFT JOIN file_retries q ON q.file_id=f.id WHERE p.path=?''', [bytes(source)]).fetchone()
    return dict(zip(['generation','root_state','file_state','error','failures','retry_in_seconds','vectors'], row)) if row else None


def exercise(session, root):
    source = root/'Broken.png'
    original = source.read_bytes()
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    session.observe('initial', ['Broken.png'])
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    def failed(attempt, after=0):
        current = state(index, source)
        return current if current and current['root_state']=='partial' and current['failures']==attempt and current['generation']>after else None
    wait_for(lambda:failed(1), 'initial decoder failure with scheduled retry', 60)
    initial = state(index, source)
    assert initial['file_state']=='failed' and initial['vectors']==0 and initial['error']
    started = time.monotonic()
    # An unrelated directory event must reconcile metadata without re-decoding
    # the failed file before its own deadline.
    (root/'Unrelated').mkdir()
    wait_for(lambda:failed(1, initial['generation']), 'directory event respects file backoff', 20)
    reconciled = state(index, source)
    wait_for(lambda:failed(2), 'real one-minute automatic retry', 80)
    retried = state(index, source)
    observed = time.monotonic()-started
    assert 40 <= observed <= 85, observed
    assert 280 <= retried['retry_in_seconds'] <= 300, retried
    assert source.read_bytes()==original
    session.click(993, 118)
    session.observe('manual-rebuild-menu', ['Rebuild index'])
    session.click(*session.word('manual-rebuild-menu', 'Rebuild', last=True))
    session.key('Escape')
    wait_for(lambda:failed(1, retried['generation']), 'manual rebuild resets automatic retry budget', 30)
    manual = state(index, source)
    Image.new('RGB', (64,64), (8,90,190)).save(source)
    def recovered():
        current = state(index, source)
        return current if current and current['root_state']=='ready' and current['file_state']=='ready' and current['vectors']==1 and current['failures'] is None else None
    wait_for(recovered, 'source revision change recovers without waiting for retry deadline', 60)
    final = state(index, source)
    measure_ui_queries(session, ':emb:"Broken.png"', 1, report_name='recovered-query.json',
                       scope='One real-model/source/UI query after failed image recovery')
    session.observe('recovered-result', ['Broken.png', 'Semantic candidates'])
    worker = model_worker(session)
    assert '--device cuda' in worker, worker
    (session.output/'retry-lifecycle.json').write_text(json.dumps({
        'initial':initial, 'unrelated_event':reconciled, 'automatic_retry':retried,
        'observed_retry_seconds':observed, 'manual_rebuild':manual, 'recovered':final, 'embedding_worker':worker,
        'scope':'Actual initial failure, one-minute retry/five-minute next deadline, unrelated event suppression, manual reset, changed-source recovery and successful CUDA image/search; longer delays/terminal/reopen covered by component tests',
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'Broken.png').write_bytes(b'Owned invalid PNG fixture for decoder retry acceptance.\n')
    return run_application_acceptance(args, output, root, exercise,
        'Per-file backoff and recovery with real decoder/model work; private compositor only', environment)


if __name__ == '__main__':
    raise SystemExit(main())
