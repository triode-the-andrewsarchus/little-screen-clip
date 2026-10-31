#!/usr/bin/env python3
"""Guarded, read-only real-folder metadata extraction checks; no enrollment/index."""
import argparse
import json
import subprocess
import time
from pathlib import Path

from quality import Worker, reader, write_check_report


def extract(binary, request):
    output = subprocess.run([str(binary), '--internal-file-extraction-plan'],
                            input=json.dumps(request).encode(), capture_output=True, timeout=3, check=True)
    result = json.loads(output.stdout)
    if 'Err' in result:
        raise RuntimeError(result['Err'])
    return result['Ok']


def metadata_inputs(args, origin, entries):
    for entry in entries:
        if not entry['regular_file'] or entry['symlink'] or not entry['name'].endswith('.md'):
            continue
        source = reader(args.binary, {'Header':{'root':origin, 'entry':entry}})['Header']
        header = extract(args.binary, {'operation':'inspect', 'source':source}).get('header')
        if not header:
            continue
        expected = next((field['value'] for field in header['fields'] if field['path'] == args.expected_field), None)
        yield entry, source, header, expected


def check_files(args, origin, entries, worker):
    plans, checks = {}, []
    for entry, source, header, expected in metadata_inputs(args, origin, entries):
        before = time.monotonic()
        hit = header['schema'] in plans
        if not hit:
            plans[header['schema']] = worker.request({'op':'extraction_plan', 'question':args.question,
                'type':args.type, 'fields':header['fields']})['plan']['path']
        path = plans[header['schema']]
        result = extract(args.binary, {'operation':'execute', 'source':source, 'plan':{
            'operation':'frontmatter_field', 'schema':header['schema'], 'path':path}}) if path else None
        verified = reader(args.binary, {'Verify':{'root':origin, 'entry':entry}}) == 'Verified'
        value = (result or {}).get('value')
        correct = value['value'] == expected if value else expected is None and path is None
        checks.append({'case':entry['name'], 'passed':bool(correct and verified),
                       'value':value['value'] if value else None, 'expected':expected, 'path':path,
                       'cache_hit':hit, 'seconds':time.monotonic()-before})
        if len(checks) == args.limit:
            break
    if len(checks) != args.limit:
        raise RuntimeError('Folder did not supply the requested count of metadata fixtures')
    return checks


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('runtime', 'models', 'binary', 'source', 'output-dir'):
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--device', default='cuda')
    parser.add_argument('--question', default='What is the publication date?')
    parser.add_argument('--type', choices=['date', 'number', 'boolean', 'text'], default='date')
    parser.add_argument('--expected-field', nargs='+', default=['date'])
    parser.add_argument('--limit', type=int, choices=range(1, 33), default=8)
    args = parser.parse_args()
    args.binary, args.source = args.binary.resolve(), args.source.resolve()
    output = args.output_dir.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    origin = reader(args.binary, {'Probe':str(args.source)})['Root']
    listing = reader(args.binary, {'List':{'root':origin, 'path':str(args.source), 'hidden':False, 'exclusions':[]}})['Directory']['directory']
    if listing['skipped']:
        raise RuntimeError('Source inventory is incomplete')
    worker = Worker(args, output, ('--role', 'answer', '--answer-provider', 'qwen3.5-9b'))
    started = time.monotonic()
    try:
        checks = check_files(args, origin, listing['entries'], worker)
        return write_check_report(worker, checks, started, output,
            'Real files: guarded bounded headers, schema-only Q8 plan selection, isolated execution, identity verification. Expected field supplied by operator; mapping/execution component check only; excludes production semantic review/full-document fallback, UI latency and general semantic accuracy. No index/enrollment/source writes.')
    finally:
        worker.close()


if __name__ == '__main__':
    raise SystemExit(main())
