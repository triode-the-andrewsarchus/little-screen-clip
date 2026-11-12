"""Verify the isolated worker's companion layout and failed-build preservation."""
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import build_media_helper as helper


class MediaDelivery(unittest.TestCase):
    def test_custom_target_carries_minimal_workspace_fixtures_and_vendoring_config(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'target'
            binary = target / 'debug/clip-media-worker'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'worker executable fixture')

            def output(command, **_):
                if command[1] == 'metadata':
                    return json.dumps({'target_directory': str(target)})
                vendor = Path(command[-1])
                vendor.mkdir()
                return '[source.crates-io]\nreplace-with="vendored-sources"\n[source.vendored-sources]\ndirectory=' + json.dumps(str(vendor)) + '\n'

            with patch.object(helper.subprocess, 'check_output', side_effect=output), \
                    patch.object(helper.subprocess, 'run') as run:
                helper.build(skip_build=True, profile='debug')
                self.assertEqual(run.call_args.args[0][1], 'metadata')
            package = binary.parent / 'media-helper'
            self.assertEqual((package / 'clip-media-worker').read_bytes(), binary.read_bytes())
            for name in ['COPYING', 'README.md', 'source/Cargo.lock',
                         'source/crates/clip-media-worker/src/video/timeline.rs',
                         'source/crates/clip-media-protocol/src/lib.rs',
                         'source/crates/little-clip-core/tests/fixtures/video/timed-colors.gif']:
                self.assertTrue((package / name).is_file(), name)
            workspace = tomllib.loads((package / 'source/Cargo.toml').read_text())
            self.assertEqual(len(workspace['workspace']['members']), 2)
            self.assertNotIn('little-clip-core', str(workspace['workspace']['members']))
            for name in ['clip-media-worker', 'clip-media-protocol']:
                manifest = tomllib.loads((package / f'source/crates/{name}/Cargo.toml').read_text())
                for dependency, specification in manifest.get('dependencies', {}).items():
                    if isinstance(specification, dict) and specification.get('workspace'):
                        self.assertIn(dependency, workspace['workspace']['dependencies'])
            config = tomllib.loads((package / 'source/.cargo/config.toml').read_text())
            self.assertEqual(config['source']['vendored-sources']['directory'], 'vendor')

    def test_failed_source_preparation_keeps_the_previous_companion(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary)
            (target / 'release').mkdir()
            (target / 'release/clip-media-worker').write_bytes(b'worker')
            package = target / 'release/media-helper'
            package.mkdir()
            sentinel = package / 'previous'
            sentinel.write_text('keep')
            with patch.object(helper.subprocess, 'check_output', return_value=json.dumps({'target_directory': str(target)})), \
                    patch.object(helper.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'cargo')):
                with self.assertRaises(subprocess.CalledProcessError):
                    helper.build(skip_build=True)
            self.assertEqual(sentinel.read_text(), 'keep')


if __name__ == '__main__':
    unittest.main()
