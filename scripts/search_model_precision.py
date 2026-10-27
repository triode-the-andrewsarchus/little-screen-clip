#!/usr/bin/env python3
"""Matched Q8/BF16 frozen-corpus evaluation; never changes application models or roots."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from search_model import setup
from search_quality_ranking import compare_rankings


BF16_FILES = (
    ('embeddinggemma-2-BF16.gguf', 557950176,
     '68bae29d62fb8c7d23e98d21fd4662753ddd636e6b62b8a70dfe059a9844f216'),
    ('mmproj-embeddinggemma-2-BF16.gguf', 982074784,
     'd2033b3cd0223cfb2e7a2b50dc80a664c78c816d375f5b017ec3ef80ee0bc766'),
)
HERE = Path(__file__).resolve().parent


def verify_weights(folder, specifications):
    records = []
    for name, size, checksum in specifications:
        path = (folder / name).resolve(strict=True)
        if path.stat().st_size != size or setup.digest(path) != checksum:
            raise ValueError(f'Pinned model size/checksum mismatch: {path}')
        records.append({'name': name, 'path': str(path), 'bytes': size, 'sha256': checksum})
    return records


def evaluate_variant(args, label, folder, specifications):
    weights = verify_weights(folder, specifications)
    output = args.output_dir / label
    ranking = args.output_dir / f'{label}-ranking.json'
    # The shared helper has fixed Q8 basenames. Only this private directory
    # aliases the verified BF16 bytes; manifests retain their real names/hashes.
    with tempfile.TemporaryDirectory(prefix=f'{label}-model-aliases-', dir=args.output_dir) as aliases:
        for (expected, _, _), record in zip(setup.FILES, weights, strict=True):
            (Path(aliases) / expected).symlink_to(record['path'])
        commands = [
            ('embedding', [sys.executable, str(HERE / 'search_model/quality.py'),
                '--runtime', str(args.runtime), '--models', aliases, '--binary', str(args.binary),
                '--device', args.device, '--output-dir', str(output)]),
            ('ranking', [sys.executable, str(HERE / 'search_quality_ranking.py'),
                '--folder', str(output), '--binary', str(args.test_binary),
                '--profile', 'release', '--report', str(ranking)]),
        ]
        for stage, command in commands:
            print(json.dumps({'variant': label, 'stage': stage}), flush=True)
            with (args.output_dir / f'{label}-{stage}.log').open('w') as log:
                subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1800)
    return {'weights': weights, 'component': json.loads((output / 'report.json').read_text()),
            'ranking': json.loads(ranking.read_text())}


def compare_variants(q8, bf16):
    components = [variant['component'] for variant in (q8, bf16)]
    requests_match = bool(components[0].get('model_request_sha256')) and (
        components[0].get('model_request_sha256') == components[1].get('model_request_sha256'))
    devices_match = all(components[0]['pipeline'][key] == components[1]['pipeline'][key]
                        for key in ('backend', 'device'))
    gates = {'identical_model_requests': requests_match, 'identical_device': devices_match}
    if not (requests_match and devices_match):
        return {'gates': gates, 'within_frozen_targets': False,
                'reason': 'Unmatched inputs/device: no model-precision claim is made'}
    rankings = [variant['ranking'] for variant in (q8, bf16)]
    defaults = [next(row for row in ranking['results'] if row['name'] == 'defaults') for ranking in rankings]
    canonical = compare_rankings(defaults[0]['cases'], defaults[1]['cases'])
    raw = compare_rankings(rankings[0]['float32_reference']['cases'], rankings[1]['float32_reference']['cases'])
    gates.update({
        'q8_product_targets': canonical['stored_summary']['within_targets'],
        'q8_float32_recall_loss_at_most_five_points': raw['recall_loss_percentage_points'] <= 5.0 + 1e-9,
        'q8_canonical_recall_loss_at_most_five_points': canonical['recall_loss_percentage_points'] <= 5.0 + 1e-9,
        'exact_names_both_models': all(ranking['exact_filename_tier_passed'] for ranking in rankings),
        'storage_precision_both_models': all(ranking['storage_precision_comparison']['within_one_percentage_point_storage_target']
            and ranking['storage_precision_comparison']['reference_exact_filename_tier_passed'] for ranking in rankings),
    })
    return {'gates': gates, 'float32_weight_comparison': raw, 'canonical_f16_comparison': canonical,
            'within_frozen_targets': all(gates.values())}


def evaluation_parser(description):
    parser = argparse.ArgumentParser(description=description)
    for name in ('runtime', 'models', 'binary', 'test-binary', 'output-dir'):
        parser.add_argument('--' + name, type=Path, required=True)
    return parser


def main():
    parser = evaluation_parser(__doc__)
    parser.add_argument('--bf16-models', type=Path, required=True)
    parser.add_argument('--device', choices=['cuda', 'vulkan', 'cpu'], default='cuda')
    parser.add_argument('--download-reference', action='store_true', help='Explicitly fetch checksum-pinned BF16 weights into --bf16-models')
    args = parser.parse_args()
    args.output_dir = args.output_dir.resolve()
    args.output_dir.mkdir(parents=True, exist_ok=False, mode=0o700)
    try:
        if args.download_reference:
            setup.download(args.bf16_models, BF16_FILES)
        q8 = evaluate_variant(args, 'q8', args.models, setup.FILES)
        bf16 = evaluate_variant(args, 'bf16', args.bf16_models, BF16_FILES)
        comparison = compare_variants(q8, bf16)
        report = {'scope': 'Frozen synthetic real-model inputs and production ranking; not representative general quality, cold/end-to-end latency or complete Phase 6 acceptance',
                  'comparison': comparison, 'variants': {'q8': q8, 'bf16': bf16},
                  'repository': 'ggml-org/embeddinggemma-2-GGUF', 'revision': setup.REVISION,
                  'driver_sha256': setup.digest(Path(__file__)),
                  'representative_quality_proven': False, 'full_phase6_acceptance_proven': False}
        (args.output_dir / 'report.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(comparison), flush=True)
        return int(not comparison['within_frozen_targets'])
    except BaseException as error:
        (args.output_dir / 'failure.json').write_text(json.dumps({'error': str(error), 'full_phase6_acceptance_proven': False}))
        raise


if __name__ == '__main__':
    raise SystemExit(main())
