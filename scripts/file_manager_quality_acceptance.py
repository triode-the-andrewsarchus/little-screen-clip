#!/usr/bin/env python3
"""Frozen relevance checks through actual enrollment, model work and native Files UI."""
import json
import math
import re
import statistics
from pathlib import Path
import sys
import time
from file_manager_acceptance import application_arguments, media_fixture_directory, run_application_acceptance, wait_for
from file_manager_real_corpus_acceptance import wait_index
from file_manager_search_acceptance import submit
from file_manager_mount_acceptance import indicator


def search_timings(path):
    pattern = r'little_clip_search_timing: search results accepted request_id=(\d+) complete=(true|false) succeeded=(true|false) elapsed_ms=(\S+) entries=(\d+)'
    rows = []
    for match in re.finditer(pattern, path.read_text()):
        request, complete, succeeded, elapsed, entries = match.groups()
        elapsed = float(elapsed)
        if not math.isfinite(elapsed) or elapsed < 0:
            raise ValueError('Invalid search timing observation')
        rows.append({'request_id': int(request), 'complete': complete == 'true',
                     'succeeded': succeeded == 'true', 'elapsed_ms': elapsed, 'entries': int(entries)})
    return rows


def measure_ui_queries(session, command, count, *, report_name='search-latency.json',
                       fixture_query='image-01',
                       scope='Warm actual-model/coordinator/source-verification/UI acceptance and sorting on the 60-file frozen corpus; excludes GPU frame presentation, cold load and large-index acceptance'):
    log = session.output / 'files.log'
    seen = {row['request_id'] for row in search_timings(log) if row['complete']}
    measurements = []
    for _ in range(count):
        submit(session, command)
        def completed():
            return [row for row in search_timings(log) if row['complete'] and row['request_id'] not in seen]
        wait_for(completed, 'new complete search timing')
        rows = completed()
        assert len(rows) == 1 and rows[0]['succeeded'], rows
        measurements.append(rows[0])
        seen.add(rows[0]['request_id'])
    elapsed = sorted(row['elapsed_ms'] for row in measurements)
    report = {'scope': scope,
              'fixture_query': fixture_query, 'samples': measurements,
              'median_ms': statistics.median(elapsed),
              'p95_ms': elapsed[math.ceil(0.95 * len(elapsed)) - 1] if len(elapsed) >= 20 else None,
              'full_phase6_acceptance_proven': False}
    (session.output / report_name).write_text(json.dumps(report, indent=2))
    return report


def exercise(session, root, cases, timing_samples=0):
    session.observe('initial-unindexed', ['item_000.txt'])
    index=Path(session.env['XDG_DATA_HOME'])/'little-screen-clip/search/index.sqlite'
    assert not index.exists()
    indicator(session,'index-red-unenrolled',(225,80,80))
    # Release the full-height list viewport before checking media rows.
    session.click(647,160)
    started=time.monotonic()
    session.click(993,118)
    session.key('Escape')
    indicator(session,'index-yellow-building',(225,180,55))
    summary=wait_index(session,index,180,started)
    assert sum(summary['modalities'].values())>=60,summary
    indicator(session,'index-green-ready',(70,185,95))
    targets={case['id']:case for case in cases}
    for label,identifier in [('image','image-01'),('spoken-audio','audio-01'),('late-video','video-11')]:
        case=targets[identifier]
        submit(session,':emb:'+json.dumps(case['query'])+';scope:recursive')
        session.observe(label+'-relevance',[case['file'],'hybrid filename/keyword/meaning'])
    submit(session,':emb:"window invoice";scope:recursive')
    session.observe('short-query-keyword-rescue',[targets['pdf-05']['file'],'Keyword match','hybrid filename/keyword/meaning'])
    submit(session,':emb:"evidence of dark matter from gravitational lensing";scope:recursive')
    session.observe('unrelated-query-omitted',['0 matching entries','No matching files','hybrid filename/keyword/meaning'])
    submit(session,':emb:"item_000.txt";scope:recursive')
    session.observe('exact-filename-tier',['item_000.txt','Filename match','hybrid filename/keyword/meaning'])
    if timing_samples:
        measure_ui_queries(session, ':emb:'+json.dumps(targets['image-01']['query'])+';scope:recursive', timing_samples)
    (session.output/'quality-native.json').write_text(json.dumps({
        'explicit_enrollment_only':True,'coordinator_index_summary':summary,
        'cross_modal_relevance':True,'short_keyword_rescue':True,
        'negative_query_omitted':True,'exact_filename_tier':True,
        'full_phase6_acceptance_proven':False,
    },indent=2))


def main():
    repo=Path(__file__).resolve().parent.parent
    args=application_arguments(__doc__,repo/'target/debug/clip-file-manager', lambda parser: parser.add_argument('--timing-samples', type=int, default=0))
    if not 0 <= args.timing_samples <= 100:
        raise ValueError('Use 0..100 timing samples')
    output,root=media_fixture_directory(args)
    root.rmdir()  # The fixture generator creates its own new directory.
    sys.path.insert(0,str(repo/'scripts/search_model'))
    from quality_fixtures import build
    cases=build(json.loads((repo/'scripts/search_model/retrieval_cases.json').read_text())['positives'],root)
    def environment(env,children):
        if args.timing_samples:
            env['LITTLE_CLIP_SEARCH_TIMINGS'] = '1'
        data=Path(env['XDG_DATA_HOME'])/'little-screen-clip/search'
        data.mkdir(parents=True)
        for name in ['runtime','models']:
            (data/name).symlink_to(Path.home()/'.local/share/little-screen-clip/search'/name,target_is_directory=True)
    return run_application_acceptance(args,output,root,lambda session,root:exercise(session,root,cases,args.timing_samples),
        'Frozen mixed-media corpus: actual opt-in indexing, image/audio/late-video meaning queries, short keyword recovery, negative omission and exact filename tier',environment)


if __name__=='__main__':
    raise SystemExit(main())
