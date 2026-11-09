#!/usr/bin/env python3
"""Explicit private Files latency probe: real source metadata, synthetic stored vectors.

Build the release desktop binary and library test harness first. The enclosing
owned source workspace is removed only after the private application exits.
This is a performance fixture, never a relevance/embedding-quality benchmark.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import tempfile
import time
from types import SimpleNamespace
from PIL import Image

import ui_acceptance_capture as private
from file_manager_acceptance import application_arguments, media_fixture_directory, preserve_file_clipboard, run_application_acceptance
from file_manager_quality_acceptance import measure_ui_queries, search_timings
from file_manager_real_corpus_acceptance import wait_index
from file_manager_search_acceptance import submit
from search_retrieval_benchmark import execute, resolve_binary
from search_model.setup import digest


TEST = 'file_manager::search::retrieval::benchmark::export_native_scale_fixture'
SCOPE = 'Real model/coordinator/current source verification/UI acceptance with synthetic stored vectors and broad relevance floor; all cases use hybrid retrieval; excludes frame presentation; not a quality evaluation'


def vector_fingerprint(index):
    digest = hashlib.sha256()
    with sqlite3.connect(index) as db:
        for row in db.execute('SELECT id,input_hash,pipeline,vector FROM segments ORDER BY id'):
            for value in (row[0].to_bytes(8, 'little'), row[1], row[2].encode(), row[3]):
                digest.update(len(value).to_bytes(8, 'little'))
                digest.update(value)
    return digest.hexdigest()


def exercise_cancellation(session, fixture):
    assert fixture['vectors'] >= 187500, 'Cancellation exercise requires an above-cache corpus'
    log = session.output/'files.log'
    command = ':emb:"abstract meaning";scope:indexed'
    cancelled = []
    for attempt in range(2):
        session.key('Escape')
        session.observe(f'before-cancel-{attempt}', ['This folder is empty'])
        before = set(re.findall(r'semantic search scheduling request_id=(\d+)', log.read_text()))
        submit(session, command)
        # Capture while the scan runs, then cancel before doing slow OCR.
        shot = session.output/f'active-scan-{attempt}.png'
        shot.unlink(missing_ok=True)
        with preserve_file_clipboard(session.env):
            private.screenshot(session.niri, session.env, shot)
        scheduled = set(re.findall(r'semantic search scheduling request_id=(\d+)', log.read_text())) - before
        assert len(scheduled) == 1, scheduled
        request = int(scheduled.pop())
        assert not any(row['request_id'] == request and row['complete'] for row in search_timings(log))
        session.key('Escape')
        session.observe(f'cancelled-scan-{attempt}', ['This folder is empty'])
        crop = session.output/f'active-scan-{attempt}-status.png'
        with Image.open(shot) as screen:
            region = screen.crop((90, 140, 1180, 310))
            region.resize((region.width*3, region.height*3)).save(crop)
        text = private.run_checked(['tesseract', str(crop), 'stdout', '--psm', '6'], env=session.env)
        (session.output/f'active-scan-{attempt}.txt').write_text(text)
        match = re.search(r'(\d[\d,]*)\s+indexed\s+parts\s+checked', text, re.IGNORECASE)
        assert match, text
        checked = int(match[1].replace(',', ''))
        assert 0 < checked < fixture['vectors'], text
        recovery = measure_ui_queries(session, command, 1,
            report_name=f'cancel-recovery-{attempt}.json', scope=SCOPE)
        assert recovery['samples'][0]['entries'] > 0, recovery
        assert not any(row['request_id'] == request for row in search_timings(log)), 'Cancelled request replaced current results'
        cancelled.append({'request_id':request, 'visible_checked_references':checked,
                          'recovered_request_id':recovery['samples'][0]['request_id']})
    (session.output/'stream-cancellation.json').write_text(json.dumps({
        'vectors':fixture['vectors'], 'synthetic_vectors':True, 'cancelled':cancelled,
        'scope':'Private UI above-cache corpus: visible nonzero scan progress, Escape restores browser, fresh query completes, no obsolete request accepted; uncached-row cancellation separately proved by component benchmark',
    }, indent=2))


def model_worker(session, role="embedding"):
    workers = private.run_checked(['ps', '--ppid', str(session.process.pid), '-o', 'pid=,args='], env=session.env)
    models = [line.strip() for line in workers.splitlines()
              if 'worker.py' in line and f'--role {role}' in line]
    assert len(models) == 1, workers
    return models[0]


def exercise(session, fixture, samples, cancellation=False, cpu_fallback=False):
    session.observe('empty-browser', ['This folder is empty', 'Empty folder'])
    session.click(647, 160)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    # Startup deliberately revalidates memberships. Let the ordinary metadata
    # reconciliation complete; never mark them verified from the test driver.
    summary = wait_index(session, index, 600, time.monotonic(), after_generation=fixture['generation'])
    assert summary['roots'] == [('ready', None)], summary
    assert summary['states'] == {'ready': fixture['files']}, summary
    assert summary['unverified_memberships'] == 0, summary
    cases = [('exact_filename', f'item_{fixture["files"] // 2:06}.txt'),
             ('rare_keyword', 'raresignalidentifier'),
             ('common_keyword', 'commonterm'),
             ('hybrid_broad_floor', 'abstract meaning')]
    reports = []
    for label, text in cases:
        command = ':emb:' + json.dumps(text) + ';scope:indexed'
        # Load the real model and materialize the vector cache outside the warm
        # sample set. The final source verification/UI path remains unchanged.
        warm = measure_ui_queries(session, command, 1,
                                  report_name=label+'-first.json', fixture_query=label, scope=SCOPE)
        assert warm['samples'][0]['entries'] > 0, warm
        report = measure_ui_queries(session, command, samples,
            report_name=label+'-warm.json', fixture_query=label,
            scope=SCOPE)
        reports.append({'name': label, **report})
        session.observe(label+'-results', ['hybrid filename/keyword/meaning'])
    worker = model_worker(session)
    if cpu_fallback:
        assert '--device cpu' in worker, worker
    if cancellation:
        exercise_cancellation(session, fixture)
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == fixture['vectors']
        assert db.execute("SELECT COUNT(*) FROM files WHERE state='ready'").fetchone()[0] == fixture['files']
        assert db.execute('SELECT MIN(continuity) FROM source_sessions').fetchone()[0] == 1
    assert vector_fingerprint(index) == fixture['vector_fingerprint'], 'Fixture vectors changed during reconciliation/querying'
    (session.output/'scale-latency.json').write_text(json.dumps({
        'files':fixture['files'], 'vectors':fixture['vectors'], 'synthetic_vectors':True,
        'vector_fingerprint':fixture['vector_fingerprint'],
        'cases':reports, 'embedding_worker':worker, 'cpu_fallback':cpu_fallback,
        'full_phase6_acceptance_proven':False,
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--test-binary', type=Path)
        parser.add_argument('--profile', choices=['release', 'debug'], default='release')
        parser.add_argument('--files', type=int, default=100000)
        parser.add_argument('--samples', type=int, default=21)
        parser.add_argument('--exercise-cancellation', action='store_true')
        parser.add_argument('--cpu-fallback', action='store_true', help='Hide CUDA/Vulkan only from the owned application to exercise ordinary CPU fallback')
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager', configure)
    if not 100 <= args.files <= 100000 or not 1 <= args.samples <= 100:
        raise ValueError('Use 100..100000 files (the bounded directory inventory) and 1..100 samples')
    extra_segments = 2 if args.exercise_cancellation else 0
    expected_vectors = args.files * (1 + extra_segments) + args.files//2
    if args.exercise_cancellation and expected_vectors < 187500:
        raise ValueError('Cancellation exercise requires at least 187500 vectors')
    test_binary = resolve_binary(SimpleNamespace(binary=args.test_binary, profile=args.profile), repo)
    output, browser = media_fixture_directory(args)
    installed = Path.home()/'.local/share/little-screen-clip/search'
    manifest = json.loads((installed/'runtime/runtime.json').read_text())
    pipeline = 'search-v2:' + json.dumps({key:manifest[key] for key in
        ('model_revision', 'llama_revision', 'shim_sha256')}, sort_keys=True, separators=(',', ':'))
    # Ordinary Btrfs data, outside excluded cache/private/generated directories.
    parent = Path.home()/'Documents/type_testing'
    with tempfile.TemporaryDirectory(prefix='clip-search-scale-', dir=parent) as workspace:
        env = dict(os.environ, CLIP_RETRIEVAL_BENCH_ROOT=workspace,
                   CLIP_RETRIEVAL_BENCH_FILES=str(args.files), CLIP_RETRIEVAL_UI_PIPELINE=pipeline,
                   CLIP_RETRIEVAL_UI_EXTRA_SEGMENTS=str(extra_segments))
        execute(test_binary, env, 1800, output/'fixture-build.log', test=TEST)
        fixture = json.loads((Path(workspace)/'native-fixture.json').read_text())
        assert fixture['files'] == args.files and fixture['vectors'] == expected_vectors
        with sqlite3.connect(Path(fixture['cache'])/'index.sqlite') as db:
            fixture['generation'] = db.execute('SELECT min(generation) FROM roots').fetchone()[0]
        fixture['vector_fingerprint'] = vector_fingerprint(Path(fixture['cache'])/'index.sqlite')
        fixture['requested_profile'] = args.profile
        fixture['desktop_sha256'] = digest(args.binary.resolve(strict=True))
        fixture['test_binary_sha256'] = digest(test_binary)
        fixture['source_sha256'] = {str(path.relative_to(repo)):digest(path) for path in
            sorted((repo/'crates/clip-workbench/src/file_manager/search').rglob('*.rs'))}
        (output/'fixture-manifest.json').write_text(json.dumps(fixture, indent=2))
        def environment(env, children):
            env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
            if args.cpu_fallback:
                # Compositors already run with their original environment.
                # Disable accelerators only for this owned application/helper.
                env['CUDA_VISIBLE_DEVICES'] = ''
                env['VK_DRIVER_FILES'] = str(output/'no-vulkan-driver.json')
                env['VK_ICD_FILENAMES'] = env['VK_DRIVER_FILES']
            data = Path(env['XDG_DATA_HOME'])/'little-screen-clip/search'
            data.mkdir(parents=True)
            shutil.copy2(Path(fixture['cache'])/'index.sqlite', data/'index.sqlite')
            for name in ['runtime', 'models']:
                (data/name).symlink_to(installed/name, target_is_directory=True)
            with (output/'scratch/config.toml').open('a') as config:
                config.write('\n[file_manager.search]\npaused = false\nroots = ['+
                             json.dumps(fixture['source'])+']\n')
                config.write('[file_manager.search.semantic_floors]\ntext = -1.0\n')
        return run_application_acceptance(args, output, browser,
            lambda session, root:exercise(session, fixture, args.samples, args.exercise_cancellation, args.cpu_fallback),
            'Owned synthetic-vector scale fixture with actual model/source verification and private UI', environment)


if __name__ == '__main__':
    raise SystemExit(main())
