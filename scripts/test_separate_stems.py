#!/usr/bin/env python3
"""Descriptor and library-I/O checks for the stem wrapper, without model downloads."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import wave

HERE = Path(__file__).resolve().parent
PYTHON = Path.home() / '.local/share/little-screen-clip/nobg-venv/bin/python'
spec = importlib.util.spec_from_file_location('separate_stems', HERE / 'separate_stems.py')
worker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(worker)


class StemDescriptorTests(unittest.TestCase):
    @unittest.skipUnless(PYTHON.exists(), 'managed Demucs venv is not installed')
    def test_prepared_descriptor_survives_path_replacement_and_keeps_output_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            original = root / 'prepared.wav'
            subprocess.run(['ffmpeg', '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                            'sine=frequency=440:duration=0.1:sample_rate=8000', '-c:a', 'pcm_f32le', str(original)], check=True)
            output = root / 'outputs'
            output.mkdir()
            with original.open('rb') as audio:
                directory = os.open(output, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    original.rename(root / 'original.wav')
                    original.write_bytes(b'replacement must not be decoded')
                    result = subprocess.run([str(PYTHON), '-I', str(HERE / 'stem_worker_fixture.py'),
                        str(PYTHON), str(HERE / 'separate_stems.py'), f'/proc/self/fd/{audio.fileno()}',
                        f'/proc/self/fd/{directory}'], pass_fds=(audio.fileno(), directory), capture_output=True, text=True, timeout=60)
                finally:
                    os.close(directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads(result.stdout)
            self.assertEqual(manifest, {name:name + '.wav' for name in ['vocals','drums','bass','other']})
            for path in manifest.values():
                with wave.open(str(output / path), 'rb') as wav:
                    self.assertEqual(wav.getnchannels(), 2)
                    self.assertEqual(wav.getframerate(), 44100)
                    self.assertEqual(wav.getnframes(), 4410)
            self.assertEqual(original.read_bytes(), b'replacement must not be decoded')

    def test_interpreter_reexec_retains_isolation_and_descriptor_arguments(self):
        with patch.object(worker.os.path, 'exists', return_value=True), \
             patch.object(worker.sys, 'executable', '/different/python'), \
             patch.object(worker.sys, 'argv', ['separate_stems.py', '/proc/self/fd/21', '/proc/self/fd/22']), \
             patch.object(worker.os, 'execv', side_effect=RuntimeError('exec reached')) as execute:
            with self.assertRaisesRegex(RuntimeError, 'exec reached'):
                worker.main()
        self.assertEqual(execute.call_args.args[1][1:], ['-I', 'separate_stems.py', '/proc/self/fd/21', '/proc/self/fd/22'])


if __name__ == '__main__':
    unittest.main()
