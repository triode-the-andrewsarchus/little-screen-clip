#!/usr/bin/env python3
"""A large inventory with controlled per-row SQL cost yields to a healthy query."""
import json
import os
from pathlib import Path
import re
import sqlite3
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_quality_acceptance import measure_ui_queries, search_timings


TRIGGER = 'clip_test_inventory_cost'


def exercise(session, root, count):
    healthy, large = root/'Healthy', root/'Large'
    index, (_, paths, vectors) = enroll_folder(session, healthy)
    assert paths == [str(healthy/'Healthy.txt')] and vectors == 1
    # This fixture-only trigger makes individual admission transactions costly.
    # No runtime delay, test hook or trigger is installed in the application.
    with sqlite3.connect(index) as db:
        db.executescript(f'''CREATE TRIGGER {TRIGGER} BEFORE INSERT ON memberships
            WHEN NEW.root_id IN (SELECT id FROM roots WHERE path=x'{os.fsencode(large).hex()}')
            BEGIN SELECT sum(value) FROM (WITH RECURSIVE cost(value) AS
              (VALUES(1) UNION ALL SELECT value+1 FROM cost WHERE value<5000)
              SELECT value FROM cost); END;''')
    session.address(large)
    session.observe('large-before-enrollment', ['entry_000000.unknown'])
    session.click(993, 118)
    session.key('Escape')
    def applied():
        with sqlite3.connect(index) as db:
            return db.execute('SELECT COUNT(*) FROM paths WHERE parent=?', [os.fsencode(large)]).fetchone()[0]
    wait_for(lambda: applied() > 0, 'large inventory starts catalog application', 60)
    before = applied()
    assert before < count, 'fixture did not catch the application in progress'
    session.address(healthy)
    session.observe('healthy-during-application', ['Healthy.txt'])
    log = session.output/'files.log'
    started = time.monotonic()
    try:
        measure_ui_queries(session, ':emb:"Healthy.txt"', 1,
            report_name='healthy-query.json', fixture_query='healthy folder',
            scope='Controlled costly large-inventory admission, not a natural latency benchmark',
            timeout_seconds=5)
    finally:
        rows = [row for row in search_timings(log) if row['complete']]
        after = applied()
        batches = [dict(rows=int(a), remaining=int(b), elapsed_ms=float(c)) for a,b,c in re.findall(
            r'directory inventory batch rows=(\d+) remaining=(\d+) elapsed_ms=(\S+)', log.read_text())]
        (session.output/'inventory-responsiveness.json').write_text(json.dumps(dict(
            files=count, applied_before=before, applied_after=after,
            submission_seconds=time.monotonic()-started, results=rows, batches=batches,
            controlled_sql_cost=True, full_index_completion_claimed=False), indent=2))
    assert len(rows) == 1 and rows[0]['succeeded'] and rows[0]['entries'] == 1, rows
    assert after < count, 'query must finish before the large admission completes'
    assert batches and all(batch['rows'] <= 64 for batch in batches), batches[-10:]
    assert any(batch['remaining'] > 0 for batch in batches), 'no incomplete inventory slice observed'
    session.observe('healthy-query-during-application', ['Healthy.txt', 'Semantic candidates'])
    with sqlite3.connect(index) as db:
        db.execute(f'DROP TRIGGER {TRIGGER}')
    session.key('Escape')
    session.address(large)
    session.click(993, 118)
    session.observe('remove-partial-index', ['Remove indexed folder'])
    session.click(*session.word('remove-partial-index', 'Remove', last=True))
    def removed():
        with sqlite3.connect(index) as db:
            return db.execute('SELECT COUNT(*) FROM roots').fetchone()[0] == 1
    wait_for(removed, 'partial inventory removal completes', 10)
    with sqlite3.connect(index) as db:
        assert [os.fsdecode(p) for p, in db.execute('SELECT path FROM paths')] == [str(healthy/'Healthy.txt')]
        assert db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == 1
    assert sum(1 for _ in large.iterdir()) == count
    assert (healthy/'Healthy.txt').read_text() == 'Healthy source remains searchable.\n'
    session.address(healthy)
    session.observe('healthy-after-removal', ['Healthy.txt'])


def main():
    repo = Path(__file__).resolve().parent.parent
    def options(parser):
        parser.add_argument('--files', type=int, default=100000)
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager', options)
    if not 1000 <= args.files <= 100000:
        raise ValueError('--files must be between 1000 and 100000')
    output, root = media_fixture_directory(args)
    (root/'Healthy').mkdir()
    (root/'Healthy/Healthy.txt').write_text('Healthy source remains searchable.\n')
    (root/'Large').mkdir()
    for ordinal in range(args.files):
        (root/'Large'/f'entry_{ordinal:06}.unknown').touch()
    return run_application_acceptance(args, output, root, lambda session, root: exercise(session, root, args.files),
        'Real large source inventory with fixture-only slow SQL: bounded admission slices, healthy query before admission completes, partial-root removal, unchanged source; not full-crawl completion or natural latency benchmark', environment)


if __name__ == '__main__':
    raise SystemExit(main())
