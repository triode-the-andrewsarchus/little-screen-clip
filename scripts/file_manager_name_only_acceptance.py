#!/usr/bin/env python3
"""Name-only indexing keeps filenames searchable without reading unsupported payloads."""
import hashlib
import json
from pathlib import Path
import re
import shlex
import sqlite3

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_frontmatter_acceptance import environment
from file_manager_maintenance_acceptance import enroll_folder
from file_manager_search_acceptance import submit


CANARY = 'UNSUPPORTED_PAYLOAD_CANARY_6412'
NAMES = ['archive.zip', 'program.exe', 'scene.stl', 'score.mid', 'database.sqlite', 'unknown.data']


def payload_reads(output, targets):
    helpers, reads = [], []
    for trace in sorted(output.glob('source-trace.*')):
        text = trace.read_text(errors='replace')
        if '--internal-file-search-reader' not in text:
            continue
        helpers.append(trace.name)
        for line in text.splitlines():
            for target in targets:
                label = re.escape(str(target))
                if re.search(r'\b(?:read|pread64|readv|preadv)\(\d+<'+label+r'>', line) or re.search(r'\bmmap\(.*\d+<'+label+r'>', line):
                    reads.append(dict(trace=trace.name, path=str(target), syscall=line))
    return helpers, reads


def exercise(session, root):
    targets = [root/'Unsupported'/name for name in NAMES]
    before = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in targets}
    session.observe('initial', ['Supported.txt', 'Unsupported'])
    index, (_, paths, vectors) = enroll_folder(session, root)
    assert len(paths) == 7 and vectors == 1
    with sqlite3.connect(index) as db:
        rows = db.execute('SELECT p.path,f.state,f.coverage,(SELECT COUNT(*) FROM segments s WHERE s.file_id=f.id) FROM files f JOIN paths p ON p.file_id=f.id').fetchall()
        unsupported = [row for row in rows if Path(bytes(row[0]).decode()).parent == root/'Unsupported']
        assert len(unsupported) == 6
        assert all(row[1:] == ('unsupported', 'name-only: extractor unavailable', 0) for row in unsupported), unsupported
    submit(session, ':emb:"archive.zip";scope:indexed')
    session.observe('name-only-filename-search', ['archive.zip', 'Semantic candidates'])
    session.key('Escape')
    submit(session, ':content:"'+CANARY+'";scope:recursive')
    session.observe('unsupported-content-not-invented', ['0 matching entries', 'unsupported/incomplete reads'])
    session.key('Escape')
    session.address(root/'Unsupported')
    submit(session, ':q:"What numeric score is explicitly listed?";type:number')
    session.observe('unsupported-question-keeps-its-own-state', ['Session answers', 'archive.zip', 'Unsupported'])
    session.close()
    assert not any('\"--role\", \"answer\"' in p.read_text(errors='replace') for p in session.output.glob('source-trace.*')), 'unsupported questions must not launch an answer model'
    helpers, reads = payload_reads(session.output, targets+[root/'Supported.txt'])
    unsupported_reads = [row for row in reads if row['path'] in before]
    positive = [row for row in reads if row['path'] == str(root/'Supported.txt')]
    (session.output/'name-only-checks.json').write_text(json.dumps(dict(
        helpers=helpers, unsupported_payload_reads=unsupported_reads,
        supported_positive_control_reads=positive,
        name_only_files=len(unsupported), vectors=vectors), indent=2))
    assert helpers and positive, 'trace must observe a real supported-source body read'
    assert not unsupported_reads, unsupported_reads[:3]
    assert before == {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in targets}


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager')
    output, root = media_fixture_directory(args)
    (root/'Supported.txt').write_text('Ordinary supported text contributes one embedding.\n')
    (root/'Unsupported').mkdir()
    body = (CANARY+'\n').encode()*32768
    for name in NAMES:
        (root/'Unsupported'/name).write_bytes(body)
    binary = args.binary.resolve()
    wrapper = output/'trace-files.sh'
    wrapper.write_text('#!/bin/sh\nexec strace -D -ff -qq -yy -s 128 -e trace=execve,openat,openat2,read,pread64,readv,preadv,mmap -o '+shlex.quote(str(output/'source-trace'))+' '+shlex.quote(str(binary))+' "$@"\n')
    wrapper.chmod(0o700)
    args.binary = wrapper
    return run_application_acceptance(args, output, root, exercise,
        'Archives/executables/3D/MIDI/unknown types stay zero-vector name-only; filename retrieval, no invented content, reader syscall traces with a supported positive control, unchanged sources', environment)


if __name__ == '__main__':
    raise SystemExit(main())
