"""The component runner must never execute an unverified GUI/application artifact."""
import argparse
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import search_retrieval_benchmark as runner


class HarnessSelectionTests(unittest.TestCase):
    def test_explicit_non_test_executable_is_rejected_without_running_it(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / 'application'
            sentinel = root / 'must-not-run'
            binary.write_text(f'#!/bin/sh\ntouch "{sentinel}"\n')
            binary.chmod(0o700)
            with self.assertRaisesRegex(RuntimeError, 'non-test binary'):
                runner.resolve_binary(argparse.Namespace(binary=binary), root)
            self.assertFalse(sentinel.exists())

    def test_newer_gui_artifact_does_not_hide_the_older_verified_harness(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            deps = root / 'target/release/deps'
            deps.mkdir(parents=True)
            for name in ['clip_workbench-tests', 'clip_workbench-application']:
                path = deps / name
                path.write_bytes(b'fixture')
                path.chmod(0o700)
            with patch.object(runner, 'is_test_harness', side_effect=lambda path: path.name.endswith('-tests')):
                selected = runner.resolve_binary(argparse.Namespace(binary=None, profile='release'), root)
            self.assertEqual(selected.name, 'clip_workbench-tests')


if __name__ == '__main__':
    unittest.main()
