#!/usr/bin/env python3
"""Frozen Q8 retrieval with all CUDA/Vulkan/CPU index/query combinations."""
import copy
import json
from pathlib import Path
import shutil
import subprocess
import sys
from search_model import setup
from search_model_precision import evaluate_variant, evaluation_parser
from search_quality_ranking import compare_rankings

HERE = Path(__file__).resolve().parent


def mixed_inputs(index, query):
    """Refuse comparisons whose model inputs, segment identities or pipeline differ."""
    reports = [json.loads((folder / 'report.json').read_text()) for folder in (index, query)]
    if not reports[0].get('model_request_sha256') or reports[0]['model_request_sha256'] != reports[1].get('model_request_sha256'):
        raise ValueError('Backend comparison requires identical ordered model requests')
    for key in ('fixture_sha256', 'validation_sha256'):
        if reports[0][key] != reports[1][key]:
            raise ValueError('Backend comparison requires identical frozen cases')
    if reports[0]['pipeline']['pipeline'] != reports[1]['pipeline']['pipeline']:
        raise ValueError('Backend comparison requires identical model/runtime pipelines')
    vectors = [json.loads((folder / 'vectors.json').read_text()) for folder in (index, query)]
    identities = [[{key: segment[key] for key in ('file', 'modality', 'location', 'text')}
                   for segment in data['segments']] for data in vectors]
    if identities[0] != identities[1] or vectors[0]['queries'].keys() != vectors[1]['queries'].keys():
        raise ValueError('Backend comparison requires identical segments and query IDs')
    return {'segments': vectors[0]['segments'], 'queries': vectors[1]['queries']}


def evaluate_pair(args, index, query):
    folder = args.output_dir / f'{index}-to-{query}'
    folder.mkdir(mode=0o700)
    indexed, queried = [args.output_dir / name for name in (index, query)]
    inputs = mixed_inputs(indexed, queried)
    # Independent ordinary source paths keep identity admission identical to the
    # existing production runner; no application index or root is enrolled.
    shutil.copytree(indexed / 'fixture', folder / 'fixture')
    for name in ('fixture-manifest.json', 'report.json'):
        shutil.copy2(indexed / name, folder / name)
    (folder / 'vectors.json').write_text(json.dumps(inputs))
    report = folder / 'ranking.json'
    with (folder / 'ranking.log').open('w') as log:
        subprocess.run([sys.executable, str(HERE / 'search_quality_ranking.py'),
            '--folder', str(folder), '--binary', str(args.test_binary),
            '--report', str(report)], stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
    return json.loads(report.read_text())


def compare_pair(ranking, reference):
    defaults = [next(row for row in data['results'] if row['name'] == 'defaults')
                for data in (ranking, reference)]
    canonical = compare_rankings(defaults[0]['cases'], defaults[1]['cases'])
    raw = compare_rankings(ranking['float32_reference']['cases'], reference['float32_reference']['cases'])
    storage = ranking['storage_precision_comparison']
    gates = {
        'product_targets': defaults[0]['summary']['within_targets'],
        'no_canonical_recall_loss': canonical['recall_loss_percentage_points'] <= 1e-9,
        'no_float32_recall_loss': raw['recall_loss_percentage_points'] <= 1e-9,
        'exact_names': ranking['exact_filename_tier_passed'],
        'storage_precision': storage['within_one_percentage_point_storage_target'],
        'reference_exact_names': storage['reference_exact_filename_tier_passed'],
    }
    return {'gates': gates, 'canonical_vs_reference': canonical, 'float32_vs_reference': raw,
            'storage_precision': storage, 'passed': all(gates.values())}


def main():
    parser = evaluation_parser(__doc__)
    args = parser.parse_args()
    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        devices = ('cuda', 'vulkan', 'cpu')
        variants = {}
        for device in devices:
            run = copy.copy(args)
            run.device = device
            variants[device] = evaluate_variant(run, device, args.models, setup.FILES)
            actual = variants[device]['component']['pipeline']['backend']
            if actual != device:
                raise ValueError(f'Requested {device}, but worker selected {actual}')
        pairs, reference = [], None
        for index in devices:
            for query in devices:
                ranking = evaluate_pair(args, index, query)
                if reference is None:
                    reference = ranking  # CUDA index and CUDA queries.
                comparison = compare_pair(ranking, reference)
                pairs.append({'indexed_backend': index, 'query_backend': query, **comparison})
                print(json.dumps({'index': index, 'query': query, 'gates': comparison['gates']}), flush=True)
        report = {'scope': 'Frozen synthetic model inputs and production SQLite/f16/lexical/RRF ranking; excludes representative quality and coordinator/UI lifecycle',
                  'variants': variants, 'pairs': pairs, 'all_passed': all(row['passed'] for row in pairs),
                  'driver_sha256': setup.digest(Path(__file__)), 'full_phase6_acceptance_proven': False}
        (args.output_dir / 'report.json').write_text(json.dumps(report, indent=2))
        return int(not report['all_passed'])
    except BaseException as error:
        (args.output_dir / 'failure.json').write_text(json.dumps({'error': str(error), 'full_phase6_acceptance_proven': False}))
        raise


if __name__ == '__main__':
    raise SystemExit(main())
