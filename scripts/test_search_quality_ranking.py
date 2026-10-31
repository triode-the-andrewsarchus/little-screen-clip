"""The storage gate compares matched production ranks, not vector cosine alone."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from search_quality_ranking import compare_storage_precision, summarize
from search_backend_quality import mixed_inputs, compare_pair, evaluate_pair
from search_model.quality_fixtures import validation_path
from search_model_precision import compare_variants


class StoragePrecisionTests(unittest.TestCase):
    def cases(self):
        return [{'id': str(index), 'expected_file': 'target' if index < 100 else None,
                 'kind': 'text', 'query': f'query-{index}', 'split': 'unit', 'rank': 1 if index < 100 else None,
                 'semantic_only_result': False, 'has_results': index < 100,
                 'top': [{'file': 'target', 'segments': [{'id': 1, 'keyword': False, 'score': 0.8}]}] if index < 100 else []}
                for index in range(102)]

    def test_one_percentage_point_boundary_and_top_ten_recall_are_enforced(self):
        reference = self.cases()
        stored = copy.deepcopy(reference)
        stored[0]['rank'] = 10  # Still recalled.
        self.assertEqual(compare_storage_precision(stored, reference)['recall_loss_percentage_points'], 0)
        stored[0]['rank'] = 11
        self.assertTrue(compare_storage_precision(stored, reference)['within_one_percentage_point_storage_target'])
        stored[1]['rank'] = None
        self.assertFalse(compare_storage_precision(stored, reference)['within_one_percentage_point_storage_target'])

    def test_ranking_and_score_changes_are_reported_even_without_recall_loss(self):
        reference = self.cases()
        stored = copy.deepcopy(reference)
        stored[0]['top'][0]['file'] = 'other'
        stored[1]['rank'] = 2
        stored[1]['top'][0]['segments'][0]['score'] = 0.79
        report = compare_storage_precision(stored, reference)
        self.assertEqual(report['changed_top10_file_orders'], ['0'])
        self.assertEqual(report['changed_expected_file_ranks'], ['1'])
        self.assertAlmostEqual(report['max_returned_segment_score_difference'], 0.01)
        self.assertTrue(report['within_one_percentage_point_storage_target'])

    def test_mismatched_query_sets_cannot_pass_the_gate(self):
        reference = self.cases()
        with self.assertRaisesRegex(ValueError, 'identical ordered queries'):
            compare_storage_precision(reference, list(reversed(reference)))
        changed = copy.deepcopy(reference)
        changed[0]['expected_file'] = 'different-target'
        with self.assertRaisesRegex(ValueError, 'identical ordered queries'):
            compare_storage_precision(reference, changed)

    def test_weight_gate_requires_matched_requests_and_enforces_five_point_limit(self):
        def variant(rows):
            return {'component': {'model_request_sha256': ['same-request'],
                    'pipeline': {'backend': 'cuda', 'device': 'fixture-device'}},
                'ranking': {'results': [{'name': 'defaults', 'cases': rows}],
                    'float32_reference': {'cases': rows}, 'exact_filename_tier_passed': True,
                    'storage_precision_comparison': {'within_one_percentage_point_storage_target': True,
                        'reference_exact_filename_tier_passed': True}}}
        q8, bf16 = variant(self.cases()), variant(self.cases())
        for row in q8['ranking']['float32_reference']['cases'][:5]:
            row['rank'] = None
        self.assertTrue(compare_variants(q8, bf16)['within_frozen_targets'])
        q8['ranking']['float32_reference']['cases'][5]['rank'] = None
        report = compare_variants(q8, bf16)
        self.assertFalse(report['within_frozen_targets'])
        self.assertFalse(report['gates']['q8_float32_recall_loss_at_most_five_points'])
        q8['component']['model_request_sha256'] = ['different-request']
        report = compare_variants(q8, bf16)
        self.assertFalse(report['within_frozen_targets'])
        self.assertNotIn('float32_weight_comparison', report)


    def test_backend_comparison_requires_no_recall_loss_and_product_targets(self):
        cases = self.cases()
        reference = {'results': [{'name': 'defaults', 'cases': cases, 'summary': summarize(cases)}],
                     'float32_reference': {'cases': copy.deepcopy(cases)},
                     'exact_filename_tier_passed': True,
                     'storage_precision_comparison': {'within_one_percentage_point_storage_target': True,
                                                      'reference_exact_filename_tier_passed': True}}
        self.assertTrue(compare_pair(reference, reference)['passed'])
        current = copy.deepcopy(reference)
        current['float32_reference']['cases'][0]['rank'] = None
        self.assertFalse(compare_pair(current, reference)['gates']['no_float32_recall_loss'])
        current = copy.deepcopy(reference)
        current['results'][0]['summary']['within_targets'] = False
        self.assertFalse(compare_pair(current, reference)['passed'])

    def test_backend_pairs_preserve_declared_or_default_validation_cases(self):
        for custom in (False, True):
            with self.subTest(custom=custom), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary)
                indexed = output / 'cuda'
                (indexed / 'fixture').mkdir(parents=True)
                for name in ('fixture-manifest.json', 'report.json'):
                    (indexed / name).write_text('{}')
                if custom:
                    (indexed / 'validation-manifest.json').write_text('{"declared":"real-corpus"}')
                expected = validation_path(indexed).read_bytes()
                def rank(command, **kwargs):
                    folder = Path(command[command.index('--folder') + 1])
                    self.assertEqual((folder / 'validation-manifest.json').read_bytes(), expected)
                    Path(command[command.index('--report') + 1]).write_text('{"checked":true}')
                args = SimpleNamespace(output_dir=output, test_binary=output / 'test-binary')
                with patch('search_backend_quality.mixed_inputs', return_value={'segments':[], 'queries':{}}), \
                     patch('search_backend_quality.subprocess.run', side_effect=rank):
                    self.assertEqual(evaluate_pair(args, 'cuda', 'cpu'), {'checked':True})

    def test_backend_inputs_reject_unmatched_requests_pipelines_and_segment_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            folders = [Path(temporary) / name for name in ('index', 'query')]
            report = {'model_request_sha256': ['same'], 'fixture_sha256': 'fixture',
                      'validation_sha256': 'validation', 'pipeline': {'pipeline': 'same-model'}}
            vectors = {'segments': [{'file': 'a', 'modality': 'text', 'location': 'first',
                                    'text': 'content', 'f32': [1.0]}], 'queries': {'q': [1.0]}}
            for folder in folders:
                folder.mkdir()
                (folder / 'report.json').write_text(json.dumps(report))
                (folder / 'vectors.json').write_text(json.dumps(vectors))
            query_vectors = copy.deepcopy(vectors)
            query_vectors['queries']['q'] = [0.9]
            (folders[1] / 'vectors.json').write_text(json.dumps(query_vectors))
            combined = mixed_inputs(*folders)
            self.assertEqual(combined['segments'], vectors['segments'])
            self.assertEqual(combined['queries'], query_vectors['queries'])
            for key, replacement in [('model_request_sha256', ['different']),
                                     ('pipeline', {'pipeline': 'different'}),
                                     ('fixture_sha256', 'different')]:
                changed = {**report, key: replacement}
                (folders[1] / 'report.json').write_text(json.dumps(changed))
                with self.assertRaises(ValueError):
                    mixed_inputs(*folders)
            (folders[1] / 'report.json').write_text(json.dumps(report))
            query_vectors['segments'][0]['location'] = 'different'
            (folders[1] / 'vectors.json').write_text(json.dumps(query_vectors))
            with self.assertRaises(ValueError):
                mixed_inputs(*folders)


if __name__ == '__main__':
    unittest.main()
