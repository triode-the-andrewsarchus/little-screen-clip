"""Boundary and aggregation checks for the synthetic retrieval-quality runner."""
import collections
import json
import math
import hashlib
import os
from pathlib import Path
import unittest
import tempfile
from unittest.mock import patch
import quality
from quality_fixtures import copy_frozen, frozen_bytes, validation_path


class QualityTests(unittest.TestCase):
    def test_preparation_unwraps_inventory_and_rejects_incomplete_or_missing_coverage(self):
        entry={'name':'owned.txt'}
        prepared={'text':'owned fixture','next':None}
        root={'path':'/owned'}
        inventory={'directory':{'entries':[entry],'skipped':0},'storage_bytes':1024}
        with patch.object(quality,'reader',side_effect=[{'Root':root},{'Directory':inventory},{'Prepared':prepared}]) as reader:
            self.assertEqual(list(quality.prepared(Path('/binary'),Path('/owned'))),[('owned.txt',prepared)])
            self.assertEqual(reader.call_count,3)
        for directory, error in [({'entries':[entry],'skipped':1},RuntimeError),({'entries':[entry]},KeyError)]:
            with patch.object(quality,'reader',side_effect=[{'Root':root},{'Directory':{'directory':directory,'storage_bytes':1024}}]) as reader:
                with self.assertRaises(error):
                    list(quality.prepared(Path('/binary'),Path('/owned')))
                self.assertEqual(reader.call_count,2,'Incomplete coverage must reject before extraction')

    def test_frozen_corpus_copy_checks_bytes_inventory_and_ordinary_file_type(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary);source=root/'source';source.mkdir()
            payload=b'Independent fixture content'
            (source/'opaque.txt').write_bytes(payload)
            record={'name':'opaque.txt','bytes':len(payload),'sha256':hashlib.sha256(payload).hexdigest()}
            manifest={'files':[record],'positives':[{'id':'query','file':'opaque.txt'}]}
            self.assertEqual(copy_frozen(source,root/'copy',manifest),manifest['positives'])
            self.assertEqual((root/'copy/opaque.txt').read_bytes(),payload)
            (source/'opaque.txt').write_bytes(b'X'*len(payload))
            with self.assertRaisesRegex(ValueError,'content changed'):
                copy_frozen(source,root/'changed',manifest)
            (source/'extra').write_bytes(b'extra')
            with self.assertRaisesRegex(ValueError,'inventory'):
                copy_frozen(source,root/'extra',manifest)
            (source/'extra').unlink()
            (source/'opaque.txt').unlink()
            (source/'opaque.txt').symlink_to(root/'copy/opaque.txt')
            with self.assertRaises(OSError):
                frozen_bytes(source/'opaque.txt',record)
            (source/'opaque.txt').unlink()
            os.mkfifo(source/'opaque.txt')
            with self.assertRaisesRegex(ValueError,'file type'):
                frozen_bytes(source/'opaque.txt',record)

    def test_frozen_corpus_rejects_path_escape_and_unbounded_size(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            for name in ['../outside','/absolute','.', '..']:
                with self.assertRaisesRegex(ValueError,'basenames'):
                    copy_frozen(root,root/'copy',{'files':[{'name':name}]})
            for size in [0,-1,256*1024*1024+1,'12']:
                with self.assertRaisesRegex(ValueError,'bounded'):
                    frozen_bytes(root/'absent',{'bytes':size})

    def test_validation_manifest_is_local_when_present_and_legacy_otherwise(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            self.assertEqual(validation_path(root).name,'retrieval_validation.json')
            local=root/'validation-manifest.json';local.write_text('{}')
            self.assertEqual(validation_path(root),local)

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
