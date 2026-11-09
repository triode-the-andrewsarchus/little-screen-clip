"""Verify native helper delivery without compiling or touching the real target tree."""
import json
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import build_multisample_helper as helper


class HelperDelivery(unittest.TestCase):
    def test_custom_target_keeps_source_licenses_and_replaces_old_java_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'separate-disk'
            release = target / 'release'
            release.mkdir(parents=True)
            (release / 'little-clip-nki').write_bytes(b'owned executable fixture')
            output = release / 'nki-helper'
            (output / 'lib').mkdir(parents=True)
            (output / 'lib/obsolete.jar').write_bytes(b'old fixture')
            with patch.object(helper.subprocess, 'check_output', return_value=json.dumps({'target_directory': str(target)})), \
                    patch.object(helper.subprocess, 'run') as build:
                helper.build(skip_build=True)
                build.assert_not_called()
            self.assertEqual((output / 'little-clip-nki').read_bytes(), b'owned executable fixture')
            self.assertFalse((output / 'lib').exists())
            for name in ['COPYING', 'COPYING.LESSER', 'THIRD_PARTY_NOTICES', 'README.md', 'source/src/main.rs',
                         'source/src/legacy.rs', 'source/Cargo.lock',
                         'source/tests/fixtures/kontakt680.nki', 'source/tests/fixtures/legacy.nki']:
                self.assertTrue((output / name).is_file(), name)
            manifest = tomllib.loads((output / 'source/Cargo.toml').read_text())
            self.assertEqual(manifest['package']['license'], 'LGPL-3.0-only')
            self.assertEqual(manifest['package']['edition'], '2024')
            self.assertEqual(manifest['dependencies']['serde_json'], '1.0')
            self.assertFalse(manifest['profile']['dev']['incremental'])
            self.assertEqual(manifest['workspace'], {})
            self.assertFalse(list(output.rglob('*.jar')))


if __name__ == '__main__':
    unittest.main()
