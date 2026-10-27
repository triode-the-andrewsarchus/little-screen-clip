#!/usr/bin/env python3
"""Evaluate frozen model outputs through production SQLite/lexical/RRF retrieval."""
import argparse
import hashlib
import json
import os
from pathlib import Path
from search_retrieval_benchmark import resolve_binary, execute

TEST = 'file_manager::search::retrieval::quality::production_quality_fixture'


def summarize(rows):
    positive = [row for row in rows if row['expected_file']]
    negative = [row for row in rows if not row['expected_file']]
    def found(row):
        return row['rank'] is not None and row['rank'] <= 10
    per_kind = {kind: sum(found(row) for row in positive if row['kind'] == kind) /
                sum(row['kind'] == kind for row in positive)
                for kind in sorted({row['kind'] for row in positive})}
    recall = sum(found(row) for row in positive) / len(positive)
    noise = sum(row['semantic_only_result'] for row in negative) / len(negative)
    any_result = sum(row['has_results'] for row in negative) / len(negative)
    return {'positive_count': len(positive), 'negative_count': len(negative),
            'recall_at_10': recall, 'per_kind': per_kind,
            'negative_semantic_only_rate': noise, 'negative_any_result_rate': any_result,
            'within_targets': recall >= 0.9 and min(per_kind.values()) >= 0.8 and noise <= 0.2 and any_result <= 0.2}


def compare_rankings(stored, reference):
    if [(row['id'], row['query'], row['kind'], row['split'], row['expected_file']) for row in stored] != [
            (row['id'], row['query'], row['kind'], row['split'], row['expected_file']) for row in reference]:
        raise ValueError('Ranking comparisons require identical ordered queries and expectations')
    before, after = summarize(stored), summarize(reference)
    order_changes, rank_changes, deltas = [], [], []
    for left, right in zip(stored, reference):
        if [hit['file'] for hit in left['top']] != [hit['file'] for hit in right['top']]:
            order_changes.append(left['id'])
        if left['rank'] != right['rank']:
            rank_changes.append(left['id'])
        scores = [{(hit['file'], segment['id'], segment['keyword']): segment['score']
                   for hit in row['top'] for segment in hit['segments']}
                  for row in (left, right)]
        deltas.extend(abs(scores[0][key] - scores[1][key]) for key in scores[0].keys() & scores[1].keys())
    loss = 100 * (after['recall_at_10'] - before['recall_at_10'])
    return {'available': True, 'stored_summary': before, 'reference_summary': after,
            'recall_loss_percentage_points': loss,
            'changed_top10_file_orders': order_changes,
            'changed_expected_file_ranks': rank_changes,
            'matched_returned_segment_scores': len(deltas),
            'max_returned_segment_score_difference': max(deltas, default=None)}


def compare_storage_precision(stored, reference):
    comparison = compare_rankings(stored, reference)
    comparison['within_one_percentage_point_storage_target'] = comparison['recall_loss_percentage_points'] <= 1.0 + 1e-9
    return comparison


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--folder', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--profile', choices=['debug', 'release'], default='debug')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--require-targets', action='store_true')
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    binary = resolve_binary(args, repo)
    report = args.report.resolve()
    env = dict(os.environ, CLIP_SEARCH_QUALITY_FOLDER=str(args.folder.resolve()),
               CLIP_SEARCH_QUALITY_REPORT=str(report))
    execute(binary, env, 120, report.with_suffix('.stdout.log'), TEST)
    data = json.loads(report.read_text())
    for result in data['results']:
        result['summary'] = summarize(result['cases'])
        result['split_summaries'] = {split: summarize([row for row in result['cases'] if row['split'] == split])
            for split in ['calibration', 'held-out', 'validation']}
        print(json.dumps({'profile': result['name'], **result['summary']}))
    data['exact_filename_tier_passed'] = all(case['rank'] == 1 for case in data['exact'])
    reference = data.get('float32_reference')
    if reference:
        defaults = next(result for result in data['results'] if result['name'] == 'defaults')
        comparison = compare_storage_precision(defaults['cases'], reference['cases'])
        comparison['reference_exact_filename_tier_passed'] = all(case['rank'] == 1 for case in reference['exact'])
        data['storage_precision_comparison'] = comparison
        print(json.dumps({'storage_precision_comparison': comparison}))
    else:
        data['storage_precision_comparison'] = {'available': False,
            'reason': 'Rebuild the current library tests to include the float32 reference'}
        print(json.dumps({'storage_precision_comparison': data['storage_precision_comparison']}))
    data['binary_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
    data['comparison_driver_sha256'] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    data['input_sha256'] = {name: hashlib.sha256((args.folder / name).read_bytes()).hexdigest()
        for name in ['vectors.json', 'fixture-manifest.json', 'report.json']}
    data['validation_sha256'] = hashlib.sha256((repo / 'scripts/search_model/retrieval_validation.json').read_bytes()).hexdigest()
    data['source_sha256'] = {name: hashlib.sha256((repo/'crates/clip-workbench/src/file_manager/search'/name).read_bytes()).hexdigest()
        for name in ['retrieval.rs', 'retrieval/quality.rs', 'store.rs', 'store/vectors.rs', 'policy.rs']}
    data['test_binary'] = str(binary)
    report.write_text(json.dumps(data, indent=2))
    checked = [result for result in data['results'] if result['name'] in {'candidate', 'defaults'}]
    precision = data.get('storage_precision_comparison', {})
    return int(args.require_targets and not (all(result['summary']['within_targets'] for result in checked)
        and data['exact_filename_tier_passed']
        and precision.get('within_one_percentage_point_storage_target', False)
        and precision.get('reference_exact_filename_tier_passed', False)))


if __name__ == '__main__':
    raise SystemExit(main())
