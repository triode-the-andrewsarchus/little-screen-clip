#!/usr/bin/env python3
"""Real-model video indexing, source timestamps, and retained partial segments."""
from pathlib import Path
import json
import os
import shutil
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
    failures = [json.loads(line) for line in (session.output/'late-frame-failures.jsonl').read_text().splitlines()]
    assert failures == [{'source':'partial.mkv','offset':12.5}], failures


def main():
    repo = Path(__file__).resolve().parent.parent
    args = application_arguments(__doc__, repo / 'target/debug/clip-file-manager')
    output, root = media_fixture_directory(args)
    clip(root / 'offset.mkv', 1, 5)
    clip(root / 'partial.mkv', 20, 0)
    # A controlled decoder failure remains a failure with sparse-seek fallback;
    # an overstated duration alone no longer guarantees that late frames fail.
    wrappers = output/'tools'
    wrappers.mkdir()
    wrapper = wrappers/'clip-media-worker'
    wrapper.write_text(r'''#!/usr/bin/env python3
import json,os,sys
args=sys.argv[1:]
if len(args)==3 and args[0]=='video-frame':
 settings=json.loads(args[1])
 source=os.path.basename(os.readlink(args[2]))
 offset=float(settings['seconds'])
 if source=='partial.mkv' and offset > 10:
  with open(os.environ['CLIP_TEST_VIDEO_FAILURE_MARKER'],'a') as marker:
   marker.write(json.dumps({'source':source,'offset':offset})+'\n')
  print('Controlled late frame failure',file=sys.stderr)
  sys.exit(1)
# exec preserves the guarded source descriptors inherited from CommandRequest.
os.execv(os.environ['CLIP_TEST_REAL_MEDIA_WORKER'],['clip-media-worker',*args])
''')
    wrapper.chmod(0o700)
    real_worker = os.environ.get('LITTLE_CLIP_MEDIA_WORKER') or str(args.binary.resolve().with_name('clip-media-worker'))
    if not Path(real_worker).is_file():
        real_worker = shutil.which('clip-media-worker')
    assert real_worker, 'Build clip-media-worker before running media acceptance'
    real_worker = str(Path(real_worker).resolve())
    def environment(env, children):
        search_environment(env, children)
        env.update(LITTLE_CLIP_MEDIA_WORKER=str(wrapper),
                   CLIP_TEST_REAL_MEDIA_WORKER=real_worker,
                   CLIP_TEST_VIDEO_FAILURE_MARKER=str(output/'late-frame-failures.jsonl'))
    return run_application_acceptance(args, output, root, exercise,
        'Explicit real-model video enrollment, visual/audio timestamps and vectors, late decode failure retaining earlier segments, unchanged sources', environment)


if __name__ == '__main__':
    raise SystemExit(main())
