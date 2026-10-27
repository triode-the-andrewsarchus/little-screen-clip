#!/usr/bin/env python3
"""Real-model video indexing, source timestamps, and retained partial segments."""
from pathlib import Path
import json
import os
import sqlite3
import subprocess
import time

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_maintenance_acceptance import search_environment
from file_manager_real_corpus_acceptance import observations, wait_index
from file_manager_search_acceptance import submit


def clip(path, length, offset):
    subprocess.run(['ffmpeg', '-v', 'error', '-f', 'lavfi', '-i',
        f'color=c=blue:s=80x40:r=10:d={length}', '-f', 'lavfi', '-i',
        f'sine=frequency=440:duration={length}', '-c:v', 'ffv1', '-c:a', 'pcm_s16le',
        '-output_ts_offset', str(offset), str(path)], check=True)


def exercise(session, root):
    session.observe('initial', ['Clear held files'])
    before = observations(root)
    index = Path(session.env['XDG_DATA_HOME']) / 'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    session.click(993, 118)
    session.key('Escape')
    summary = wait_index(session, index, 300, time.monotonic())
    assert summary['states'] == {'ready': 2}, summary
    assert summary['modalities'] == {'audio': 1, 'image': 3}, summary
    with sqlite3.connect(index) as db:
        assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
        assert db.execute('SELECT MIN(extraction_version) FROM files').fetchone()[0] == 4
        rows = db.execute('SELECT p.path,f.coverage,s.modality,s.location,length(s.vector) FROM files f JOIN paths p ON p.file_id=f.id JOIN segments s ON s.file_id=f.id ORDER BY p.path,s.id').fetchall()
    data = [{'file': Path(os.fsdecode(path)).name, 'coverage': coverage, 'modality': modality,
             'location': location, 'vector_bytes': size} for path, coverage, modality, location, size in rows]
    (session.output / 'video-segments.json').write_text(json.dumps(data, indent=2))
    offset = [item for item in data if item['file'] == 'offset.mkv']
    partial = [item for item in data if item['file'] == 'partial.mkv']
    assert len(offset) == 2 and all(item['coverage'].startswith('sampled:') for item in offset), data
    assert offset[0]['location'] == 'video frame at seconds 0.500000', offset
    assert offset[1]['location'].startswith('audio seconds 0'), offset
    assert len(partial) == 2 and all(item['coverage'].startswith('partial: Video frame decode failed; retained 2 completed segments; planned coverage: sampled:') for item in partial), data
    assert all(item['modality'] == 'image' for item in partial), partial
    assert all(item['vector_bytes'] == 1536 for item in data), data
    submit(session, ':emb:"a blue image"')
    session.observe('video-visual-search', ['offset.mkv', 'Semantic image match', 'video frame at seconds', '00:01'])
    assert observations(root) == before, 'Video extraction changed source observations'


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    clip(root / 'offset.mkv', 1, 5)
    clip(root / 'partial.mkv', 10, 0)
    # A plausible but overstated Matroska stream duration: early frames decode,
    # then a later sample is absent. Source bytes remain untouched during indexing.
    path = root / 'partial.mkv'
    data = path.read_bytes()
    assert b'00:00:10.000000000' in data
    path.write_bytes(data.replace(b'00:00:10.000000000', b'00:00:20.000000000'))
    return run_application_acceptance(args, output, root, exercise,
        'Explicit real-model video enrollment, visual/audio timestamps and vectors, late decode failure retaining earlier segments, unchanged sources', search_environment)


if __name__ == '__main__':
    raise SystemExit(main())
