"""Boundary and aggregation checks for the synthetic retrieval-quality runner."""
import collections
import json
import math
from pathlib import Path
import unittest
import quality


class QualityTests(unittest.TestCase):
    def test_frozen_fixture_counts_and_independent_splits(self):
        data=json.loads((Path(__file__).with_name('retrieval_cases.json')).read_text())
        self.assertEqual(collections.Counter(case['kind'] for case in data['positives']),dict.fromkeys(['text','pdf','image','audio','video'],12))
        self.assertEqual(len(data['negatives']),20)
        self.assertEqual(len(data['exact_queries']),10)
        for kind in ['text','pdf','image','audio','video']:
            self.assertEqual(collections.Counter(case['split'] for case in data['positives'] if case['kind']==kind),{'calibration':6,'held-out':6})

    def test_duplicate_weak_segments_do_not_outrank_a_single_strong_segment(self):
        segments=[{'file':'weak','f32':[0.1,0.9],'location':str(i),'modality':'text'} for i in range(100)]
        segments.append({'file':'strong','f32':[0.9,0.1],'location':'last page','modality':'text'})
        ranked=quality.scores([1.0,0.0],segments,'f32')
        self.assertEqual([row['file'] for row in ranked],['strong','weak'])
        self.assertEqual(ranked[0]['location'],'last page')

    def test_half_storage_components_remain_finite_and_normalized(self):
        vector=[math.sin(index/17) for index in range(768)]
        actual=quality.half_vector(vector)
        self.assertEqual(len(actual),768)
        self.assertTrue(all(math.isfinite(value) for value in actual))
        self.assertAlmostEqual(sum(value*value for value in actual),1.0,places=6)


if __name__=='__main__':
    unittest.main()
