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
import shutil
import sqlite3
import tempfile
import time
from types import SimpleNamespace

from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance
from file_manager_quality_acceptance import measure_ui_queries
from file_manager_real_corpus_acceptance import wait_index
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


def exercise(session, fixture, samples):
    session.observe('empty-browser', ['This folder is empty', 'Empty folder'])
    session.click(647, 160)
    index = Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    # Startup deliberately revalidates memberships. Let the ordinary metadata
    # reconciliation complete; never mark them verified from the test driver.
    summary = wait_index(session, index, 600, time.monotonic())
    assert summary['roots'] == [('ready', None)], summary
    assert summary['states'] == {'ready': fixture['files']}, summary
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
    with sqlite3.connect(index) as db:
        assert db.execute('SELECT COUNT(*) FROM segments').fetchone()[0] == fixture['vectors']
        assert db.execute("SELECT COUNT(*) FROM files WHERE state='ready'").fetchone()[0] == fixture['files']
        assert db.execute('SELECT MIN(continuity) FROM source_sessions').fetchone()[0] == 1
    assert vector_fingerprint(index) == fixture['vector_fingerprint'], 'Fixture vectors changed during reconciliation/querying'
    (session.output/'scale-latency.json').write_text(json.dumps({
        'files':fixture['files'], 'vectors':fixture['vectors'], 'synthetic_vectors':True,
        'vector_fingerprint':fixture['vector_fingerprint'],
        'cases':reports, 'full_phase6_acceptance_proven':False,
    }, indent=2))


def main():
    repo = Path(__file__).resolve().parent.parent
    def configure(parser):
        parser.add_argument('--test-binary', type=Path)
        parser.add_argument('--profile', choices=['release', 'debug'], default='release')
        parser.add_argument('--files', type=int, default=100000)
        parser.add_argument('--samples', type=int, default=21)
    args = application_arguments(__doc__, repo/'target/release/clip-file-manager', configure)
    if not 100 <= args.files <= 200000 or not 1 <= args.samples <= 100:
        raise ValueError('Use 100..200000 files and 1..100 samples')
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
                   CLIP_RETRIEVAL_BENCH_FILES=str(args.files), CLIP_RETRIEVAL_UI_PIPELINE=pipeline)
        execute(test_binary, env, 1800, output/'fixture-build.log', test=TEST)
        fixture = json.loads((Path(workspace)/'native-fixture.json').read_text())
        assert fixture['files'] == args.files and fixture['vectors'] == args.files + args.files//2
        fixture['vector_fingerprint'] = vector_fingerprint(Path(fixture['cache'])/'index.sqlite')
        fixture['requested_profile'] = args.profile
        fixture['desktop_sha256'] = digest(args.binary.resolve(strict=True))
        fixture['test_binary_sha256'] = digest(test_binary)
        fixture['source_sha256'] = {str(path.relative_to(repo)):digest(path) for path in
            sorted((repo/'crates/clip-workbench/src/file_manager/search').rglob('*.rs'))}
        (output/'fixture-manifest.json').write_text(json.dumps(fixture, indent=2))
        def environment(env, children):
            env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
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
            lambda session, root:exercise(session, fixture, args.samples),
            'Owned synthetic-vector scale fixture with actual model/source verification and private UI', environment)


if __name__ == '__main__':
    raise SystemExit(main())
