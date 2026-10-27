#!/usr/bin/env python3
"""Read-only indexing audit of an explicitly supplied real folder in a private app instance.

This is a corpus smoke test, not a semantic retrieval-quality benchmark. It keeps
all generated data in the private run directory and never mutates source files.
"""
from pathlib import Path
import collections
import json
import os
import sqlite3
import time

from file_manager_acceptance import application_arguments, run_application_acceptance
from file_manager_maintenance_acceptance import search_environment
from file_manager_search_acceptance import submit


def observations(root):
    result, pending = {}, [root]
    while pending:
        directory = pending.pop()
        for item in os.scandir(directory):
            if item.name.startswith('.') or item.is_symlink():
                continue
            if item.is_dir(follow_symlinks=False):
                pending.append(Path(item.path))
            elif item.is_file(follow_symlinks=False):
                stat = item.stat(follow_symlinks=False)
                result[item.path] = (stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns)
                if len(result) > 100000:
                    raise AssertionError('Real corpus audit exceeds its 100,000-file bound')
    return result


def wait_index(session, index, timeout, started):
    deadline = started + timeout
    previous = None
    while time.monotonic() < deadline:
        if session.process.poll() is not None:
            raise RuntimeError('File manager exited during real corpus indexing')
        if index.exists():
            try:
                with sqlite3.connect(index) as db:
                    roots = db.execute('SELECT state,error FROM roots').fetchall()
                    states = dict(db.execute('SELECT state,COUNT(*) FROM files GROUP BY state'))
                    modalities = dict(db.execute('SELECT modality,COUNT(*) FROM segments WHERE vector IS NOT NULL GROUP BY modality'))
                    coverage = dict(db.execute("SELECT coalesce(coverage,'pending'),COUNT(*) FROM files GROUP BY coverage"))
                    failures = db.execute("SELECT error,COUNT(*) FROM files WHERE state='failed' GROUP BY error").fetchall()
                current = {'roots': roots, 'states': states, 'modalities': modalities, 'coverage': coverage, 'failures': failures}
                if current != previous:
                    current['elapsed_seconds'] = round(time.monotonic() - started, 3)
                    (session.output / 'index-progress.json').write_text(json.dumps(current, indent=2))
                    previous = {key: value for key, value in current.items() if key != 'elapsed_seconds'}
                if roots and roots[0][0] in ('ready', 'partial') and not states.get('pending'):
                    return previous
                if roots and roots[0][0] == 'error':
                    raise AssertionError(roots)
            except sqlite3.OperationalError as error:
                if 'no such table' not in str(error) and 'locked' not in str(error):
                    raise
        time.sleep(0.2)
    else:
        raise RuntimeError(f'Real corpus indexing exceeded {timeout}s; retained index-progress.json describes its last state')

def exercise(session, root, timeout):
    before = observations(root)
    session.observe('initial', ['Clear held files'])  # Red Index text is unreliable in OCR.
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists(), 'A real folder must still require explicit enrollment'
    started = time.monotonic()
    session.click(993, 118)
    session.key('Escape')
    summary = wait_index(session, index, timeout, started)
    modalities = summary['modalities']
    assert sum(modalities.values()) > 0, modalities
    assert observations(root) == before, 'Indexing changed visible source metadata or membership'
    with sqlite3.connect(index) as db:
        assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
        assert not db.execute('SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL AND length(vector)<>1536').fetchone()[0]
        paths = [os.fsdecode(path) for path, in db.execute('SELECT path FROM paths')]
        assert all(Path(path).is_relative_to(root) for path in paths)
        examples = db.execute('SELECT DISTINCT p.path FROM paths p JOIN files f ON f.id=p.file_id JOIN segments s ON s.file_id=f.id WHERE s.vector IS NOT NULL ORDER BY p.path LIMIT 3').fetchall()
    evidence = {'source_files': len(before), 'source_bytes': sum(item[2] for item in before.values()),
        'source_extensions': dict(collections.Counter(Path(path).suffix.lower() for path in before)),
        'indexed_paths': len(paths), 'elapsed_seconds': round(time.monotonic() - started, 3), **summary}
    (session.output / 'corpus-summary.json').write_text(json.dumps(evidence, indent=2))
    for number, (path,) in enumerate(examples):
        name = Path(os.fsdecode(path)).name
        submit(session, ':emb:' + json.dumps(name, ensure_ascii=False) + ';scope:recursive')
        session.observe(f'exact-name-{number}', [name, 'Semantic candidates'])
    assert observations(root) == before


def main():
    repository = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--source', type=Path, required=True)
        parser.add_argument('--index-timeout', type=int, default=1800)
    args = application_arguments(__doc__, repository / 'target/debug/clip-file-manager', configure)
    root = args.source.resolve(strict=True)
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True)
    return run_application_acceptance(args, output, root,
        lambda session, folder: exercise(session, folder, args.index_timeout),
        'Read-only real corpus indexing, explicit enrollment in private data storage, per-modality/coverage audit and exact filename retrieval', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
