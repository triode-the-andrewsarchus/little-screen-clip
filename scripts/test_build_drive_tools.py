"""Keep gated test helpers separate from production delivery and cached builds."""
import hashlib
import json
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch

import build_drive_tools as helper


class DriveHelperDelivery(unittest.TestCase):
    def test_failed_patch_keeps_the_previous_source_and_retry_applies_every_patch(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cache = root / 'cache'
            archive = root / helper.ARCHIVE
            pristine = root / ('e2fsprogs-' + helper.VERSION)
            pristine.mkdir()
            (pristine / 'fixture').write_text('pristine source')
            with tarfile.open(archive, 'w:xz') as bundle:
                bundle.add(pristine, arcname=pristine.name)
            fingerprint = {'patch_sha256': 'a' * 64, 'test_io_gate_sha256': 'b' * 64}
            source = cache / ('build-' + 'a' * 16 + '-gate-' + 'b' * 16)
            source.mkdir(parents=True)
            (source / 'previous').write_text('unverified previous cache')
            patches = [root / 'base.patch', root / 'gate.patch']
            def fail_gate(command, **kwargs):
                if command[0] == 'patch' and command[-1] == str(patches[1]):
                    raise RuntimeError('interrupted patch')
            with patch.object(helper.subprocess, 'run', side_effect=fail_gate):
                with self.assertRaisesRegex(RuntimeError, 'interrupted patch'):
                    helper.prepare_source(cache, archive, patches, fingerprint)
            self.assertEqual((source / 'previous').read_text(), 'unverified previous cache')
            self.assertFalse((source / '.patches-applied').exists())
            with patch.object(helper.subprocess, 'run') as run:
                tree = helper.prepare_source(cache, archive, patches, fingerprint)
            self.assertEqual([call.args[0][-1] for call in run.call_args_list if call.args[0][0] == 'patch'], [str(item) for item in patches])
            self.assertEqual((tree / 'fixture').read_text(), 'pristine source')
            self.assertEqual(json.loads((source / '.patches-applied').read_text()), fingerprint)
            self.assertFalse((source / 'previous').exists())

    def test_cache_identity_distinguishes_both_build_modes_and_accepts_legacy_normal_builds(self):
        repo = Path(helper.__file__).resolve().parents[1]
        patches = repo / 'scripts/drive_tools'
        for current_gate in [False, True]:
            for requested_gate in [False, True]:
                with self.subTest(current_gate=current_gate, requested_gate=requested_gate), tempfile.TemporaryDirectory() as temporary:
                    output = Path(temporary) / 'packaged'
                    for name in ['fuse2fs', 'COPYING', 'source/' + helper.ARCHIVE,
                                 'source/fuse2fs-noreplace.patch', 'source/fuse2fs-test-gate.patch', 'source/rebuild.sh']:
                        target = output / name
                        target.parent.mkdir(parents=True, exist_ok=True)
                        target.write_bytes(b'owned cache fixture')
                    fingerprint = {
                        'version': helper.VERSION, 'source_sha256': helper.SHA256,
                        'patch_sha256': hashlib.sha256((patches / 'fuse2fs-noreplace.patch').read_bytes()).hexdigest(),
                    }
                    if current_gate:
                        fingerprint['test_io_gate_sha256'] = hashlib.sha256((patches / 'fuse2fs-test-gate.patch').read_bytes()).hexdigest()
                    (output / 'build.json').write_text(json.dumps(fingerprint))
                    with patch.object(helper.subprocess, 'run', side_effect=RuntimeError('uncached build')) as run:
                        if current_gate == requested_gate:
                            helper.build(output, Path(temporary) / 'cache', test_io_gate=requested_gate)
                            run.assert_not_called()
                        else:
                            with self.assertRaisesRegex(RuntimeError, 'uncached build'):
                                helper.build(output, Path(temporary) / 'cache', test_io_gate=requested_gate)
                            self.assertEqual(run.call_count, 1)

    def test_test_build_refuses_the_normal_packaging_directories_before_running_commands(self):
        repo = Path(helper.__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary, patch.object(helper.subprocess, 'run') as run:
            for profile in ['debug', 'release']:
                with self.assertRaisesRegex(ValueError, 'must not replace'):
                    helper.build(repo / 'target' / profile / 'drive-tools', Path(temporary), test_io_gate=True)
            run.assert_not_called()

    def test_symlinked_target_directory_cannot_bypass_the_test_output_guard(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(helper.subprocess, 'run') as run:
            repo = Path(temporary) / 'repo'
            patches = repo / 'scripts/drive_tools'
            patches.mkdir(parents=True)
            (patches / 'fuse2fs-noreplace.patch').write_text('owned fingerprint fixture')
            actual_target = Path(temporary) / 'separate-target'
            actual_target.mkdir()
            (repo / 'target').symlink_to(actual_target, target_is_directory=True)
            with patch.object(helper, '__file__', str(repo / 'scripts/build_drive_tools.py')):
                with self.assertRaisesRegex(ValueError, 'must not replace'):
                    helper.build(actual_target / 'debug/drive-tools', Path(temporary) / 'cache', test_io_gate=True)
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
